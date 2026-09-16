//! Cooperative reader for the fixed bincode schema. Async field readers retain
//! their position between bounded polls; no whole serialized object is buffered.

use super::memory::{MemoryBudget, Reservation};
use super::transport::DATA_BYTES;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[derive(Clone)]
pub(super) struct Reader(Arc<Mutex<Input>>);

struct Input {
    chunk: Vec<u8>,
    offset: usize,
    received: u64,
    consumed: u64,
    declared: u64,
    credits: usize,
    reservation: Option<Reservation>,
}

pub(super) enum Step<T> {
    NeedInput,
    Yield,
    Ready(T, Reservation),
}

pub(super) struct Decoder<T> {
    reader: Reader,
    parser: Option<Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send>>>,
}

impl<T> Decoder<T> {
    pub fn new(
        declared: u64,
        limit: u64,
        budget: &MemoryBudget,
        parse: impl FnOnce(Reader) -> Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send>>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(declared <= limit, "object exceeds transfer limit");
        // One incoming chunk and bounded string/scalar scratch. Decoded vector
        // allocations are charged separately, before reserving their storage.
        let reader = Reader(Arc::new(Mutex::new(Input {
            chunk: Vec::new(),
            offset: 0,
            received: 0,
            consumed: 0,
            declared,
            credits: 0,
            reservation: Some(budget.reserve((DATA_BYTES + 8192) as u64)?),
        })));
        let parser = Some(parse(reader.clone()));
        Ok(Self { reader, parser })
    }

    pub fn feed(&mut self, bytes: Vec<u8>) -> anyhow::Result<()> {
        anyhow::ensure!(self.parser.is_some(), "decoder finished");
        let mut input = self.reader.0.lock().expect("decoder lock");
        anyhow::ensure!(
            input.offset == input.chunk.len(),
            "unconsumed decoder input"
        );
        anyhow::ensure!(
            !bytes.is_empty() && bytes.len() <= DATA_BYTES,
            "invalid decoder chunk"
        );
        let received = input.received.checked_add(bytes.len() as u64);
        anyhow::ensure!(
            received.is_some_and(|n| n <= input.declared),
            "object exceeds declared size"
        );
        input.received = received.unwrap();
        input.chunk = bytes;
        input.offset = 0;
        Ok(())
    }

    pub fn step(&mut self) -> anyhow::Result<Step<T>> {
        self.reader.0.lock().expect("decoder lock").credits = 1024;
        let parser = self
            .parser
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("decoder finished"))?;
        let outcome = parser
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()));
        match outcome {
            Poll::Ready(result) => {
                self.parser = None;
                let value = result?;
                let mut input = self.reader.0.lock().expect("decoder lock");
                anyhow::ensure!(
                    input.consumed == input.declared,
                    "truncated or trailing object data"
                );
                input.chunk = Vec::new();
                Ok(Step::Ready(value, input.reservation.take().unwrap()))
            }
            Poll::Pending => {
                let input = self.reader.0.lock().expect("decoder lock");
                Ok(if input.credits == 0 {
                    Step::Yield
                } else {
                    Step::NeedInput
                })
            }
        }
    }
}

impl Reader {
    pub fn charged(&self) -> u64 {
        self.0
            .lock()
            .expect("decoder lock")
            .reservation
            .as_ref()
            .unwrap()
            .bytes()
    }

    pub fn take_since(&self, previous: u64) -> anyhow::Result<Reservation> {
        let mut input = self.0.lock().expect("decoder lock");
        let reservation = input.reservation.as_mut().unwrap();
        let bytes = reservation
            .bytes()
            .checked_sub(previous)
            .ok_or_else(|| anyhow::anyhow!("invalid allocation scope"))?;
        reservation.split(bytes)
    }
    pub async fn checkpoint(&self) {
        poll_fn(|_| {
            let mut input = self.0.lock().expect("decoder lock");
            if input.credits == 0 {
                Poll::Pending
            } else {
                input.credits -= 1;
                Poll::Ready(())
            }
        })
        .await
    }

    pub fn charge(&self, bytes: usize) -> anyhow::Result<()> {
        self.0
            .lock()
            .expect("decoder lock")
            .reservation
            .as_mut()
            .unwrap()
            .grow(bytes as u64)
    }

    async fn read(&self, output: &mut [u8]) -> anyhow::Result<()> {
        self.checkpoint().await;
        let mut written = 0;
        poll_fn(|_| {
            let mut input = self.0.lock().expect("decoder lock");
            let count = (input.chunk.len() - input.offset).min(output.len() - written);
            output[written..written + count]
                .copy_from_slice(&input.chunk[input.offset..input.offset + count]);
            written += count;
            input.offset += count;
            input.consumed += count as u64;
            if written == output.len() {
                Poll::Ready(Ok(()))
            } else if input.received == input.declared {
                Poll::Ready(Err(anyhow::anyhow!("truncated object field")))
            } else {
                Poll::Pending
            }
        })
        .await
    }

    pub async fn u8(&self) -> anyhow::Result<u8> {
        let mut bytes = [0];
        self.read(&mut bytes).await?;
        Ok(bytes[0])
    }
    pub async fn u32(&self) -> anyhow::Result<u32> {
        let mut bytes = [0; 4];
        self.read(&mut bytes).await?;
        Ok(u32::from_le_bytes(bytes))
    }
    pub async fn u64(&self) -> anyhow::Result<u64> {
        let mut bytes = [0; 8];
        self.read(&mut bytes).await?;
        Ok(u64::from_le_bytes(bytes))
    }
    pub async fn usize(&self) -> anyhow::Result<usize> {
        Ok(usize::try_from(self.u64().await?)?)
    }
    pub async fn boolean(&self) -> anyhow::Result<bool> {
        match self.u8().await? {
            0 => Ok(false),
            1 => Ok(true),
            _ => anyhow::bail!("invalid boolean or option tag"),
        }
    }

    async fn length(&self, minimum: usize) -> anyhow::Result<usize> {
        let count = self.usize().await?;
        let input = self.0.lock().expect("decoder lock");
        anyhow::ensure!(
            count
                .checked_mul(minimum)
                .is_some_and(|n| n as u64 <= input.declared - input.consumed),
            "collection exceeds object size"
        );
        Ok(count)
    }

    pub async fn vector<T, F, Fut>(&self, minimum: usize, mut parse: F) -> anyhow::Result<Vec<T>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        let count = self.length(minimum).await?;
        self.charge(
            count
                .checked_mul(std::mem::size_of::<T>())
                .ok_or_else(|| anyhow::anyhow!("collection size overflow"))?,
        )?;
        let mut values = Vec::new();
        values.try_reserve_exact(count)?;
        for _ in 0..count {
            self.checkpoint().await;
            values.push(parse().await?);
        }
        Ok(values)
    }

    pub async fn bytes(&self) -> anyhow::Result<Vec<u8>> {
        let count = self.length(1).await?;
        self.charge(count)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(count)?;
        let mut buffer = [0; 4096];
        while bytes.len() < count {
            let n = (count - bytes.len()).min(buffer.len());
            self.read(&mut buffer[..n]).await?;
            bytes.extend_from_slice(&buffer[..n]);
        }
        Ok(bytes)
    }

    pub async fn string(&self) -> anyhow::Result<String> {
        let count = self.length(1).await?;
        self.charge(count)?;
        let mut text = String::new();
        text.try_reserve_exact(count)?;
        let mut buffer = [0; 4096];
        let mut left = count;
        let mut carry = 0;
        while left > 0 {
            let n = left.min(buffer.len() - carry);
            self.read(&mut buffer[carry..carry + n]).await?;
            left -= n;
            let end = carry + n;
            match std::str::from_utf8(&buffer[..end]) {
                Ok(value) => {
                    text.push_str(value);
                    carry = 0;
                }
                Err(error) => {
                    anyhow::ensure!(
                        error.error_len().is_none() && left > 0,
                        "invalid UTF-8 string"
                    );
                    let valid = error.valid_up_to();
                    text.push_str(std::str::from_utf8(&buffer[..valid])?);
                    buffer.copy_within(valid..end, 0);
                    carry = end - valid;
                }
            }
        }
        Ok(text)
    }
}
