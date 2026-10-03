//! One asynchronous protocol-v6 Open. The catalog, pages and indexes stay
//! private until every page's End and cooperative validation have completed.
use super::ClientStep;
use super::decode::{Decoder, Step};
use super::hierarchy::{Assembly, Finished, PAGE_ENTRIES, Page, PageDecoder, PageSpec};
use super::memory::{MemoryBudget, Reservation};
use super::metadata::{MetadataDecoder, MetadataStep, ValidatedMetadata};
use super::session::RemoteSession;
use super::transport::{Body, Command, ObjectId, Packet, Receive, Receiver, acknowledgement};
use crate::session::{LoadResult, Session};
use std::sync::Arc;

enum Decoding {
    Header(MetadataDecoder),
    Page { scopes: bool, decoder: PageDecoder },
    Finish(Decoder<Finished>),
}
enum Decoded {
    Header(Box<ValidatedMetadata>),
    Page {
        scopes: bool,
        page: Page,
        reservation: Reservation,
    },
}

/// Drives a bounded Open response and acknowledges only consumed chunks.
pub struct OpenTransfer<Tag = u64> {
    request: u64,
    tag: Tag,
    generation: u64,
    limit: u64,
    budget: MemoryBudget,
    session: u64,
    receiver: Option<Receiver>,
    decoder: Option<Decoding>,
    decoded: Option<Decoded>,
    assembly: Option<Box<Assembly>>,
    pending_ack: Option<Packet>,
    finished: bool,
    failed: bool,
    next_scope: u32,
    next_var: u32,
}
impl<Tag: Copy> OpenTransfer<Tag> {
    pub fn new(
        request: u64,
        tag: Tag,
        generation: u64,
        limit: u64,
        budget: MemoryBudget,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(request != 0, "invalid Open request identity");
        Ok(Self {
            request,
            tag,
            generation,
            limit,
            budget,
            session: 0,
            receiver: None,
            decoder: None,
            decoded: None,
            assembly: None,
            pending_ack: None,
            finished: false,
            failed: false,
            next_scope: 0,
            next_var: 0,
        })
    }
    pub fn command(&self) -> Packet {
        Packet {
            session: 0,
            request: self.request,
            sequence: 0,
            body: Body::Command(Command::Open {
                max_object_bytes: self.limit,
            }),
        }
    }
    pub fn accept(&mut self, packet: Packet) -> anyhow::Result<ClientStep<Tag>> {
        let result = self.accept_inner(packet);
        self.poison_on_error(result)
    }
    fn accept_inner(&mut self, packet: Packet) -> anyhow::Result<ClientStep<Tag>> {
        anyhow::ensure!(!self.failed && !self.finished, "Open transfer finished");
        anyhow::ensure!(
            self.pending_ack.is_none(),
            "response before decode finished"
        );
        if self.receiver.is_none() {
            self.receiver = Some(Receiver::new(
                packet.session,
                self.request,
                vec![ObjectId::Metadata],
                self.limit,
            )?);
            self.session = packet.session;
        }
        let ack = acknowledgement(&packet);
        match self.receiver.as_mut().unwrap().accept(packet)? {
            Receive::Begin {
                object: ObjectId::Metadata,
                decoded_bytes,
            } => {
                anyhow::ensure!(self.assembly.is_none(), "duplicate Open catalog");
                self.decoder = Some(Decoding::Header(MetadataDecoder::new(
                    decoded_bytes,
                    self.limit,
                    &self.budget,
                )?));
            }
            Receive::Begin {
                object,
                decoded_bytes,
            } => {
                let assembly = self
                    .assembly
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("page before catalog"))?;
                let (scopes, page, total) = match object {
                    ObjectId::Scopes(page) if self.next_var == 0 && page == self.next_scope => {
                        (true, page, assembly.header.scopes)
                    }
                    ObjectId::Variables(page)
                        if self.next_scope == assembly.header.scope_pages()
                            && page == self.next_var =>
                    {
                        (false, page, assembly.header.vars)
                    }
                    _ => anyhow::bail!("unexpected hierarchy page"),
                };
                let start = page
                    .checked_mul(PAGE_ENTRIES as u32)
                    .ok_or_else(|| anyhow::anyhow!("page range overflow"))?;
                let count = (total - start).min(PAGE_ENTRIES as u32) as usize;
                self.decoder = Some(Decoding::Page {
                    scopes,
                    decoder: PageDecoder::new(
                        decoded_bytes,
                        self.limit,
                        &self.budget,
                        PageSpec {
                            scopes,
                            start,
                            count,
                            total_scopes: assembly.header.scopes,
                            total_vars: assembly.header.vars,
                            total_signals: assembly.header.info.signal_count,
                        },
                    )?,
                });
            }
            Receive::Data(bytes) => {
                match self
                    .decoder
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("missing Open decoder"))?
                {
                    Decoding::Header(d) => d.feed(bytes)?,
                    Decoding::Page { decoder, .. } => decoder.feed(bytes)?,
                    Decoding::Finish(_) => anyhow::bail!("data after final page"),
                }
                self.pending_ack = Some(ack);
                return self.step_inner();
            }
            Receive::Complete(_) => {
                match self
                    .decoded
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("Open object validation incomplete"))?
                {
                    Decoded::Header(decoded) => {
                        let assembly = Assembly::new(decoded.header, decoded.reservation)?;
                        self.receiver.as_mut().unwrap().expect_pages(
                            assembly.header.scope_pages(),
                            assembly.header.var_pages(),
                        )?;
                        self.assembly = Some(Box::new(assembly));
                    }
                    Decoded::Page {
                        scopes,
                        page,
                        reservation,
                    } => {
                        self.assembly
                            .as_mut()
                            .unwrap()
                            .push(scopes, page, reservation)?;
                        if scopes {
                            self.next_scope += 1
                        } else {
                            self.next_var += 1
                        }
                    }
                }
                if self.receiver.as_ref().unwrap().is_complete() {
                    self.decoder = Some(Decoding::Finish(
                        self.assembly
                            .take()
                            .unwrap()
                            .finish(self.limit, &self.budget)?,
                    ));
                    self.pending_ack = Some(ack);
                    return self.step_inner();
                }
            }
            Receive::Failed { message, .. } => {
                self.decoder = None;
                self.decoded = None;
                self.assembly = None;
                self.finished = true;
                self.receiver.as_mut().unwrap().finish_open_error();
                return Ok(ClientStep::Complete {
                    ack,
                    result: LoadResult::Opened {
                        tag: self.tag,
                        generation: self.generation,
                        result: Err(anyhow::anyhow!(message)),
                    },
                });
            }
        }
        Ok(ClientStep::Ack(ack))
    }
    pub fn step(&mut self) -> anyhow::Result<ClientStep<Tag>> {
        let result = self.step_inner();
        self.poison_on_error(result)
    }
    fn step_inner(&mut self) -> anyhow::Result<ClientStep<Tag>> {
        anyhow::ensure!(
            !self.failed && self.pending_ack.is_some(),
            "no pending Open decode"
        );
        match self
            .decoder
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("missing Open decoder"))?
        {
            Decoding::Header(d) => match d.step()? {
                MetadataStep::NeedInput => {}
                MetadataStep::Yield => return Ok(ClientStep::Yield),
                MetadataStep::Decoded(decoded) => {
                    self.decoded = Some(Decoded::Header(decoded));
                    self.decoder = None;
                }
            },
            Decoding::Page { scopes, decoder } => match decoder.step()? {
                Step::NeedInput => {}
                Step::Yield => return Ok(ClientStep::Yield),
                Step::Ready(page, reservation) => {
                    self.decoded = Some(Decoded::Page {
                        scopes: *scopes,
                        page,
                        reservation,
                    });
                    self.decoder = None;
                }
            },
            Decoding::Finish(d) => match d.step()? {
                Step::NeedInput => anyhow::bail!("incomplete hierarchy indexes"),
                Step::Yield => return Ok(ClientStep::Yield),
                Step::Ready((metadata, sizes, ownership), scratch) => {
                    drop(scratch);
                    self.decoder = None;
                    self.finished = true;
                    let session =
                        RemoteSession::from_parts(self.session, metadata, sizes, ownership)?;
                    return Ok(ClientStep::Complete {
                        ack: self.pending_ack.take().unwrap(),
                        result: LoadResult::Opened {
                            tag: self.tag,
                            generation: self.generation,
                            result: Ok(Arc::new(session) as Arc<dyn Session>),
                        },
                    });
                }
            },
        }
        Ok(ClientStep::Ack(self.pending_ack.take().unwrap()))
    }
    fn poison_on_error<T>(&mut self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        if result.is_err() {
            self.failed = true;
            self.decoder = None;
            self.decoded = None;
            self.assembly = None;
            self.pending_ack = None;
        }
        result
    }
    pub fn is_complete(&self) -> bool {
        self.finished && !self.failed
    }
    pub fn finish(self) -> anyhow::Result<()> {
        anyhow::ensure!(self.is_complete(), "incomplete Open transfer");
        self.receiver.unwrap().finish()
    }
}
