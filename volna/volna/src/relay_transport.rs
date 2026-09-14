//! Webview courier state around the binary RPC driver. JavaScript carries byte
//! arrays and decimal courier tokens; it never interprets the query protocol.
use std::task::{Context, Poll, Waker};
use vtr_query::{
    Budget, Error, Result,
    rpc_session::{Incarnation, Outbound, RpcDriver, RpcOpenFuture},
};

pub(crate) struct RelayTransport {
    driver: RpcDriver,
    incarnation: Incarnation,
    sending: Option<Outbound>,
    sent: u64,
    received: u64,
    writable: Option<Waker>,
}
impl RelayTransport {
    pub fn fail_incarnation(&mut self, incarnation: Incarnation) -> bool {
        if incarnation != self.incarnation {
            return false;
        }
        self.fail();
        true
    }
    pub fn new(incarnation: Incarnation, budget: Budget) -> Result<(Self, RpcOpenFuture)> {
        let (driver, opening) = RpcDriver::new(incarnation, budget)?;
        Ok((
            Self {
                driver,
                incarnation,
                sending: None,
                sent: 0,
                received: 0,
                writable: None,
            },
            opening,
        ))
    }
    /// The host copies `packet` synchronously, then waits for rpcWritten before
    /// polling again. Keep its admitted storage alive until that acknowledgement.
    pub fn poll_send(&mut self, cx: &mut Context<'_>) -> Poll<Result<Option<u64>>> {
        if self.sending.is_some() {
            self.writable = Some(cx.waker().clone());
            return Poll::Pending;
        }
        match std::task::ready!(self.driver.poll_outbound(cx)) {
            Ok(Some(packet)) => {
                let Some(token) = self.sent.checked_add(1) else {
                    self.fail();
                    return Poll::Ready(Err(Error::Invalid("relay send counter exhausted")));
                };
                self.sent = token;
                self.sending = Some(packet);
                self.writable = Some(cx.waker().clone());
                Poll::Ready(Ok(Some(token)))
            }
            result => Poll::Ready(result.map(|_| None)),
        }
    }
    pub fn packet(&self) -> Option<&[u8]> {
        self.sending.as_ref().map(Outbound::bytes)
    }
    pub fn written(&mut self, incarnation: Incarnation, token: u64) -> Result<bool> {
        if incarnation != self.incarnation {
            return Ok(false);
        }
        if self.sending.is_none() || token != self.sent {
            self.fail();
            return Err(Error::Invalid("unexpected relay write acknowledgement"));
        }
        self.sending = None;
        if let Some(waker) = self.writable.take() {
            waker.wake();
        }
        Ok(true)
    }
    /// A true result allows rpcConsumed to release the host's reply credit,
    /// including replies discarded because the Rust session has closed.
    pub fn receive(&mut self, incarnation: Incarnation, token: u64, bytes: &[u8]) -> Result<bool> {
        if incarnation != self.incarnation {
            return Ok(false);
        }
        if self.received.checked_add(1) != Some(token) {
            self.fail();
            return Err(Error::Invalid("unexpected relay reply sequence"));
        }
        if let Err(error) = self.driver.receive(incarnation, bytes) {
            self.fail();
            return Err(error);
        }
        self.received = token;
        Ok(true)
    }
    pub fn fail(&mut self) {
        self.sending = None;
        self.driver.transport_failed();
        if let Some(waker) = self.writable.take() {
            waker.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::Wake,
    };
    struct Count(AtomicUsize);
    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[test]
    fn one_packet_credit_retains_admission_and_acknowledgement_wakes_sender() {
        let budget = Budget::new(16 << 20);
        let incarnation = Incarnation([1; 16]);
        let (mut relay, opening) = RelayTransport::new(incarnation, budget.clone()).unwrap();
        let wake = Arc::new(Count(AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(relay.poll_send(&mut cx), Poll::Ready(Ok(Some(1)))));
        assert!(relay.packet().is_some());
        // An acknowledgement can arrive before a second poll. It must still
        // wake the host so newly queued handshake/control traffic can run.
        assert!(!relay.written(Incarnation([2; 16]), 1).unwrap());
        assert!(relay.packet().is_some());
        assert!(relay.written(incarnation, 1).unwrap());
        assert_eq!(wake.0.load(Ordering::SeqCst), 1);
        assert!(relay.packet().is_none());
        assert!(relay.written(incarnation, 1).is_err());
        drop(opening);
        drop(relay);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn stale_replies_are_ignored_and_sequence_gaps_fail_the_session() {
        let incarnation = Incarnation([1; 16]);
        let (mut relay, _opening) =
            RelayTransport::new(incarnation, Budget::new(16 << 20)).unwrap();
        assert!(!relay.fail_incarnation(Incarnation([2; 16])));
        assert!(!relay.receive(Incarnation([2; 16]), 99, b"invalid").unwrap());
        assert!(relay.receive(incarnation, 2, b"invalid").is_err());
        assert!(matches!(
            relay.poll_send(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(None))
        ));
    }
}
