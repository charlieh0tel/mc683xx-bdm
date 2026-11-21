//! A pod that answers from a script, for tests.

use std::collections::VecDeque;
use std::time::Duration;

use mc683xx_bdm_proto::usb::PACKET;
use mc683xx_bdm_proto::usb::Request;

use crate::pod::Pod;
use crate::transport::Transport;
use crate::transport::TransportError;

/// Replies sent and not yet received.
#[derive(Debug, Default)]
pub(crate) struct Outbox(VecDeque<Vec<u8>>);

impl Outbox {
    /// The next reply into `reply`, or `NoReply`.
    fn receive(&mut self, reply: &mut [u8; PACKET], timeout: Duration) -> Result<usize, TransportError> {
        let answer = self.0.pop_front().ok_or(TransportError::NoReply(timeout))?;
        reply[..answer.len()].copy_from_slice(&answer);
        Ok(answer.len())
    }
}

/// Records each request and answers from a script.
#[derive(Debug, Default)]
pub(crate) struct Script {
    /// The requests, as sent.
    pub(crate) requests: Vec<Vec<u8>>,
    /// The replies still to give, in order.
    replies: VecDeque<Vec<u8>>,
    outbox: Outbox,
}

impl Transport for Script {
    fn send(&mut self, request: &[u8]) -> Result<(), TransportError> {
        self.requests.push(request.to_vec());
        self.outbox.0.push_back(self.replies.pop_front().expect("script ended early"));
        Ok(())
    }

    fn receive(&mut self, reply: &mut [u8; PACKET], timeout: Duration) -> Result<usize, TransportError> {
        self.outbox.receive(reply, timeout)
    }
}

/// Records each request and answers it by what it asks.
pub(crate) struct Answers<F> {
    /// The requests, as sent.
    pub(crate) requests: Vec<Vec<u8>>,
    /// The reply to each request.
    answer: F,
    outbox: Outbox,
}

impl<F: FnMut(Request<'_>) -> Vec<u8>> Transport for Answers<F> {
    fn send(&mut self, request: &[u8]) -> Result<(), TransportError> {
        self.requests.push(request.to_vec());
        self.outbox.0.push_back((self.answer)(parsed(request)));
        Ok(())
    }

    fn receive(&mut self, reply: &mut [u8; PACKET], timeout: Duration) -> Result<usize, TransportError> {
        self.outbox.receive(reply, timeout)
    }
}

/// A pod that answers each request with `answer`'s reply to it.
pub(crate) fn answering<F: FnMut(Request<'_>) -> Vec<u8>>(answer: F) -> Pod<Answers<F>> {
    Pod::new(Answers { requests: Vec::new(), answer, outbox: Outbox::default() })
}

/// A pod that gives `replies`, in order.
pub(crate) fn pod(replies: &[&[u8]]) -> Pod<Script> {
    Pod::new(Script {
        requests: Vec::new(),
        replies: replies.iter().map(|r| r.to_vec()).collect(),
        outbox: Outbox::default(),
    })
}

/// The request a packet carries.
pub(crate) fn parsed(packet: &[u8]) -> Request<'_> {
    Request::parse(packet).unwrap()
}
