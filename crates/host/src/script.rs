//! A pod that answers from a script, for tests.  Scripted replies are
//! a reply's status and body: the script adds the header, the request's
//! tag echoed and FREEZE's edge count, except for Hello.

use std::collections::VecDeque;
use std::time::Duration;

use mc683xx_bdm_proto::usb::HELLO_REQUEST;
use mc683xx_bdm_proto::usb::PACKET;
use mc683xx_bdm_proto::usb::Request;

use crate::pod::Pod;
use crate::transport::Transport;
use crate::transport::TransportError;

/// Replies sent and not yet received.
#[derive(Debug, Default)]
pub(crate) struct Outbox(VecDeque<Vec<u8>>);

impl Outbox {
    /// Sends `message`, the reply to `request`, with its header unless
    /// `request` is Hello.
    fn send(&mut self, request: &[u8], freeze_edges: u8, message: Vec<u8>) {
        if request == HELLO_REQUEST {
            self.0.push_back(message);
        } else {
            self.0.push_back([request[0], freeze_edges].into_iter().chain(message).collect());
        }
    }

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
    /// The replies still to give, in order, each with FREEZE's edge count.
    replies: VecDeque<(u8, Vec<u8>)>,
    outbox: Outbox,
}

impl Transport for Script {
    fn send(&mut self, request: &[u8]) -> Result<(), TransportError> {
        self.requests.push(request.to_vec());
        let (freeze_edges, message) = self.replies.pop_front().expect("script ended early");
        self.outbox.send(request, freeze_edges, message);
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
        let message = (self.answer)(parsed(request));
        self.outbox.send(request, 0, message);
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

/// A pod that gives `replies`, in order, FREEZE's edge count 0.
pub(crate) fn pod(replies: &[&[u8]]) -> Pod<Script> {
    let replies: Vec<(u8, &[u8])> = replies.iter().map(|reply| (0, *reply)).collect();
    pod_with_edges(&replies)
}

/// A pod that gives `replies`, in order, each with FREEZE's edge count.
pub(crate) fn pod_with_edges(replies: &[(u8, &[u8])]) -> Pod<Script> {
    Pod::new(Script {
        requests: Vec::new(),
        replies: replies.iter().map(|(edges, reply)| (*edges, reply.to_vec())).collect(),
        outbox: Outbox::default(),
    })
}

/// The request a tagged packet carries.
pub(crate) fn parsed(packet: &[u8]) -> Request<'_> {
    Request::parse(&packet[1..]).unwrap()
}
