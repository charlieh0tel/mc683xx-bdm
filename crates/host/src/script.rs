//! A pod that answers from a script, for tests.

use std::collections::VecDeque;

use mc683xx_bdm_proto::usb::PACKET;
use mc683xx_bdm_proto::usb::Request;

use crate::pod::Pod;
use crate::transport::Transport;
use crate::transport::TransportError;

/// Records each request and answers from a script.
#[derive(Debug, Default)]
pub(crate) struct Script {
    /// The requests, as sent.
    pub(crate) requests: Vec<Vec<u8>>,
    /// The replies still to give, in order.
    replies: VecDeque<Vec<u8>>,
}

impl Transport for Script {
    fn exchange(&mut self, request: &[u8], reply: &mut [u8; PACKET]) -> Result<usize, TransportError> {
        self.requests.push(request.to_vec());
        let answer = self.replies.pop_front().expect("script ended early");
        reply[..answer.len()].copy_from_slice(&answer);
        Ok(answer.len())
    }
}

/// Records each request and answers it by what it asks.
pub(crate) struct Answers<F> {
    /// The requests, as sent.
    pub(crate) requests: Vec<Vec<u8>>,
    /// The reply to each request.
    answer: F,
}

impl<F: FnMut(Request<'_>) -> Vec<u8>> Transport for Answers<F> {
    fn exchange(&mut self, request: &[u8], reply: &mut [u8; PACKET]) -> Result<usize, TransportError> {
        self.requests.push(request.to_vec());
        let answer = (self.answer)(parsed(request));
        reply[..answer.len()].copy_from_slice(&answer);
        Ok(answer.len())
    }
}

/// A pod that answers each request with `answer`'s reply to it.
pub(crate) fn answering<F: FnMut(Request<'_>) -> Vec<u8>>(answer: F) -> Pod<Answers<F>> {
    Pod::new(Answers { requests: Vec::new(), answer })
}

/// A pod that gives `replies`, in order.
pub(crate) fn pod(replies: &[&[u8]]) -> Pod<Script> {
    Pod::new(Script { requests: Vec::new(), replies: replies.iter().map(|r| r.to_vec()).collect() })
}

/// The request a packet carries.
pub(crate) fn parsed(packet: &[u8]) -> Request<'_> {
    Request::parse(packet).unwrap()
}
