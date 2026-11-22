//! The pod's requests as calls: each encodes a request, exchanges it
//! over the transport, and decodes the reply.  Memory and raw transfers
//! longer than a packet go as several requests.

use mc683xx_target::Unsupported;
use std::collections::VecDeque;
use std::ops::Range;
use std::time::Duration;
use std::time::Instant;

use mc683xx_bdm_proto::bdm::byte_count;
use mc683xx_bdm_proto::dsclk::MIN_HZ;
use mc683xx_bdm_proto::pod::NOT_READY_RETRIES;
use mc683xx_bdm_proto::pod::NOT_READY_TIMEOUT_US;
use mc683xx_bdm_proto::pod::Workload;

use mc683xx_bdm_proto::usb::Atemp;
use mc683xx_bdm_proto::usb::HELLO_REQUEST;
use mc683xx_bdm_proto::usb::MAX_MEMORY_BYTES;
use mc683xx_bdm_proto::usb::MAX_TRANSFER_WORDS;
use mc683xx_bdm_proto::usb::PACKET;
use mc683xx_bdm_proto::usb::PROTOCOL_VERSION;
use mc683xx_bdm_proto::usb::ParseError;
use mc683xx_bdm_proto::usb::Pins;
use mc683xx_bdm_proto::usb::ReplyError;
use mc683xx_bdm_proto::usb::Request;
use mc683xx_bdm_proto::usb::encode_transfer;
use mc683xx_bdm_proto::usb::parse_atemp;
use mc683xx_bdm_proto::usb::parse_hello_reply;
use mc683xx_bdm_proto::usb::parse_pins;
use mc683xx_bdm_proto::usb::parse_reply;
use mc683xx_bdm_proto::usb::parse_reply_header;

use mc683xx_bdm_proto::usb::parse_transfer_reply;
use mc683xx_bdm_proto::usb::parse_value;
use mc683xx_target::address::Address;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;

use crate::step::StepError;
use crate::transport::Transport;
use crate::transport::TransportError;

/// Why a request did not complete.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The packets did not get through.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// The pod refused or failed the request, or its reply was malformed.
    #[error(transparent)]
    Reply(#[from] ReplyError),
    /// The request cannot be sent: a length or field out of range.
    #[error(transparent)]
    Request(#[from] ParseError),
    /// A memory block's length is not whole accesses.
    #[error("length {length} is not a multiple of {size:?}")]
    Length {
        /// The block's length in bytes.
        length: usize,
        /// The access size.
        size: Size,
    },
    /// The pod speaks another protocol version than this host.
    #[error("the pod speaks protocol {0}, this host {PROTOCOL_VERSION}")]
    Protocol(u32),
    /// A step could not be made, or did not end at its stub.
    #[error("step: {0}")]
    Step(#[from] StepError),
    /// The target left background mode since it was seen halted, as a
    /// reply's FREEZE edge count shows: a reset, say, perhaps back into
    /// background mode.  What was kept of the halt is forgotten; look
    /// or halt again.
    #[error("the target left background mode since it was halted (a reset?); look again")]
    Reentered,
    /// A `Cpu32Target` call this target does not offer.
    #[error(transparent)]
    Unsupported(#[from] Unsupported),
}

/// How long a reply may take beyond its words and not-ready waits
/// (`Workload`): a reset into background mode, about 110 ms (`pod-fw`'s
/// RESET pulse and FREEZE timeout), CALL's 100 ms wait for FREEZE, and
/// USB's own.  All of Hello's time.
const TIMEOUT: Duration = Duration::from_secs(1);
/// Bits in a serial word.
const WORD_BITS: u64 = 17;
/// The pod's own time per word, beyond its bits: generous against the
/// firmware's handling between PIO words.
const WORD_OVERHEAD_US: u64 = 20;
const MICROS_PER_SECOND: u64 = 1_000_000;

/// Memory requests a block keeps sent and unanswered.  The pod answers
/// one at a time, in order, and reads the next request as soon as it
/// has written a reply (docs/usb-protocol.md, Transport), so one
/// request waiting behind the one in progress hides the host's turn
/// between reply and request; a few more cover the host's scheduling
/// and USB frame jitter.  Few, so a failure leaves few stale replies,
/// and few requests run behind it.  The USB transport keeps a read
/// waiting for each.
pub(crate) const IN_FLIGHT: usize = 4;

// Each request outstanding, and each stale reply a failed block leaves
// ahead of the next block's (up to `IN_FLIGHT`, the failed request's own
// after a timeout), has a tag of its own among the 255.
const _: () = assert!(2 * IN_FLIGHT <= u8::MAX as usize);

/// How long the pod may take over a request of `workload` at DSCLK
/// `dsclk_hz`: [`TIMEOUT`], its words, and each wait at the first of
/// its limits, time or NOPs, a word over for the check.
fn timeout(workload: Workload, dsclk_hz: u32) -> Duration {
    let word_us = WORD_BITS * MICROS_PER_SECOND / u64::from(dsclk_hz.max(1)) + WORD_OVERHEAD_US;
    let wait_us = u64::from(NOT_READY_TIMEOUT_US).min((u64::from(NOT_READY_RETRIES) + 1) * word_us) + word_us;
    TIMEOUT + Duration::from_micros(u64::from(workload.words) * word_us + u64::from(workload.waits) * wait_us)
}

/// The pod, over a transport.
#[derive(Debug)]
pub struct Pod<T> {
    transport: T,
    /// The last request's tag, 1-255 once one is sent: 0 tags only the
    /// pod's replies to packets it could not read a tag from.
    tag: u8,
    /// FREEZE's edge count, low 8 bits, from the last reply that carried
    /// one.
    freeze_edges: Option<u8>,
    /// DSCLK's rate as the pod last gave it, in `GetClock`'s reply at
    /// the handshake or `SetClock`'s.  Until then the slowest the pod
    /// sets, for `GetClock`'s own exchange, since the pod keeps a rate
    /// from one host to the next.  Each reply's timeout is worked out
    /// from it.
    dsclk_hz: u32,
}

impl<T: Transport> Pod<T> {
    /// The pod at the end of `transport`.
    pub const fn new(transport: T) -> Self {
        Self { transport, tag: 0, freeze_edges: None, dsclk_hz: MIN_HZ }
    }

    /// The transport, to give it back.
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// FREEZE's edge count, its low 8 bits, as the last reply gave it
    /// (docs/usb-protocol.md, Replies): one that moved between two
    /// replies means the target left background mode in between, perhaps
    /// to enter it again.  `None` before the first reply.
    #[must_use]
    pub const fn freeze_edges(&self) -> Option<u8> {
        self.freeze_edges
    }

    /// The pod's protocol version, from its untagged Hello, whatever its
    /// version: any other reply waiting (a stale one) is discarded.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn hello(&mut self) -> Result<u32, Error> {
        self.transport.send(&HELLO_REQUEST)?;
        let deadline = Instant::now() + TIMEOUT;
        let mut reply = [0; PACKET];
        loop {
            let length = self.receive(&mut reply, deadline, TIMEOUT)?;
            if let Some(version) = parse_hello_reply(&reply[..length]) {
                return Ok(version);
            }
        }
    }

    /// What a host does first on a connection: says Hello, checks the
    /// pod speaks this host's protocol, and asks DSCLK's rate, which
    /// sizes the timeouts from then on.
    ///
    /// # Errors
    /// As [`Error`]; [`Error::Protocol`] for another version.
    pub fn handshake(&mut self) -> Result<(), Error> {
        let version = self.hello()?;
        if version != PROTOCOL_VERSION {
            return Err(Error::Protocol(version));
        }
        self.get_clock()?;
        Ok(())
    }

    /// FREEZE, RESET and DS levels, and VDD.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn pins(&mut self) -> Result<Pins, Error> {
        let mut reply = [0; PACKET];
        let body = self.request(&Request::Pins, &mut reply)?;
        Ok(parse_pins(body)?)
    }

    /// Stops the target in background mode, if it is not already;
    /// returns ATEMP as the pod read it first after entry.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn halt(&mut self) -> Result<Atemp, Error> {
        self.atemp(&Request::Halt)
    }

    /// Resets the target into background mode; returns ATEMP.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn reset_halt(&mut self) -> Result<Atemp, Error> {
        self.atemp(&Request::ResetHalt)
    }

    /// The pod firmware's version: crate version and git commit.
    ///
    /// # Errors
    /// As [`Error`]; the reply is not ASCII.
    pub fn version(&mut self) -> Result<String, Error> {
        let mut reply = [0; PACKET];
        let body = self.request(&Request::Version, &mut reply)?;
        if !body.is_ascii() {
            return Err(ReplyError::Malformed.into());
        }
        Ok(String::from_utf8_lossy(body).into_owned())
    }

    /// Brings a halted target's serial link back into frame; returns
    /// ATEMP, stale.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn resync(&mut self) -> Result<Atemp, Error> {
        self.atemp(&Request::Resync)
    }

    /// Pulses BERR with the target halted; whether a BDM access hung in a
    /// bus cycle ended (the pod also does this itself when a halted
    /// target stays not ready).
    ///
    /// # Errors
    /// As [`Error`]; `NoFreeze` if the target runs.
    pub fn berr_pulse(&mut self) -> Result<bool, Error> {
        let mut reply = [0; PACKET];
        let body = self.request(&Request::BerrPulse, &mut reply)?;
        Ok(parse_value(body)? != 0)
    }

    /// Holds BKPT low, clocking nothing: the target halts, and comes out
    /// of any reset meanwhile in background mode.  The next request
    /// releases it.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn hold(&mut self) -> Result<(), Error> {
        self.done(&Request::Hold)
    }

    /// Resets the target to run, background mode off.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn reset_run(&mut self) -> Result<(), Error> {
        self.done(&Request::ResetRun)
    }

    /// Resumes at RPC.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn go(&mut self) -> Result<(), Error> {
        self.done(&Request::Go)
    }

    /// Stacks RPC and runs from `address`.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn call(&mut self, address: Address) -> Result<(), Error> {
        self.done(&Request::Call(address))
    }

    /// Sets DSCLK's rate; returns the rate set.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn set_clock(&mut self, dsclk_hz: u32) -> Result<u32, Error> {
        let set = self.value(&Request::SetClock { dsclk_hz })?;
        self.dsclk_hz = set;
        Ok(set)
    }

    /// DSCLK's rate in effect, as the pod gives it.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn get_clock(&mut self) -> Result<u32, Error> {
        let rate = self.value(&Request::GetClock)?;
        self.dsclk_hz = rate;
        Ok(rate)
    }

    /// A data or address register.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn read_register(&mut self, register: Register) -> Result<u32, Error> {
        self.value(&Request::ReadRegister(register))
    }

    /// Writes a data or address register.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn write_register(&mut self, register: Register, value: u32) -> Result<(), Error> {
        self.done(&Request::WriteRegister(register, value))
    }

    /// A system register.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn read_system_register(&mut self, register: SystemRegister) -> Result<u32, Error> {
        self.value(&Request::ReadSystemRegister(register))
    }

    /// Writes a system register.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn write_system_register(&mut self, register: SystemRegister, value: u32) -> Result<(), Error> {
        self.done(&Request::WriteSystemRegister(register, value))
    }

    /// Reads `data.len()` bytes from `address` in `size` accesses,
    /// through SFC, as many requests as it takes, up to [`IN_FLIGHT`]
    /// of them outstanding.
    ///
    /// # Errors
    /// As [`Error`]; a length that is not whole accesses.  The first
    /// failure is returned at once and nothing more is sent; `data` is
    /// filled up to the failed chunk.  The requests already sent behind
    /// it still run, and a later exchange discards their replies.
    pub fn read_memory(&mut self, address: Address, size: Size, data: &mut [u8]) -> Result<(), Error> {
        let length = data.len();
        check_length(size, length)?;
        self.pipeline(
            chunk_count(length),
            |n, tag, packet| {
                let range = chunk(length, n);
                Request::ReadMemory { address: at(address, range.start), size, length: range.len() }
                    .encode(tag, packet)
            },
            |n, body| {
                let into = &mut data[chunk(length, n)];
                if body.len() != into.len() {
                    return Err(ReplyError::Malformed.into());
                }
                into.copy_from_slice(body);
                Ok(())
            },
        )
    }

    /// Writes `data` at `address` in `size` accesses, through DFC, as
    /// many requests as it takes, one at a time: unlike a read, a write
    /// behind a failed one would change memory, so none is sent ahead.
    ///
    /// # Errors
    /// As [`Error`]; a length that is not whole accesses.  The first
    /// failure is returned at once; nothing past the failed chunk is
    /// written.
    pub fn write_memory(&mut self, address: Address, size: Size, data: &[u8]) -> Result<(), Error> {
        check_length(size, data.len())?;
        (0..chunk_count(data.len())).try_for_each(|n| {
            let range = chunk(data.len(), n);
            self.done(&Request::WriteMemory { address: at(address, range.start), size, data: &data[range] })
        })
    }

    /// Raw serial words, each answered with the 17 bits shifted in, as
    /// many requests as it takes.  Unguarded: with the target running,
    /// the clocks are BKPT pulses.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn transfer(&mut self, words: &[u16]) -> Result<Vec<u32>, Error> {
        let mut responses = Vec::with_capacity(words.len());
        for chunk in words.chunks(MAX_TRANSFER_WORDS) {
            let mut reply = [0; PACKET];
            let body = self.exchange(|tag, packet| encode_transfer(tag, chunk, packet), &mut reply)?;
            let before = responses.len();
            responses.extend(parse_transfer_reply(body)?);
            if responses.len() - before != chunk.len() {
                return Err(ReplyError::Malformed.into());
            }
        }
        Ok(responses)
    }

    fn value(&mut self, request: &Request<'_>) -> Result<u32, Error> {
        let mut reply = [0; PACKET];
        let body = self.request(request, &mut reply)?;
        Ok(parse_value(body)?)
    }

    fn atemp(&mut self, request: &Request<'_>) -> Result<Atemp, Error> {
        let mut reply = [0; PACKET];
        let body = self.request(request, &mut reply)?;
        Ok(parse_atemp(body)?)
    }

    fn done(&mut self, request: &Request<'_>) -> Result<(), Error> {
        let mut reply = [0; PACKET];
        let body = self.request(request, &mut reply)?;
        empty(body)
    }

    /// Exchanges `count` requests, the `n`th as `encode(n, tag, packet)`
    /// writes it, keeping up to [`IN_FLIGHT`] outstanding, and hands
    /// each reply's result to `take(n, body)`, in order.  The pod starts
    /// a request at the earliest once it has written the reply before,
    /// so each reply's timeout still runs from when the host starts
    /// waiting for it.  The first failure, sending, receiving or
    /// `take`'s, is returned at once and nothing more is sent.
    fn pipeline(
        &mut self,
        count: usize,
        mut encode: impl FnMut(usize, u8, &mut [u8; PACKET]) -> Result<usize, ParseError>,
        mut take: impl FnMut(usize, &[u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut unsent = 0..count;
        let mut outstanding = VecDeque::with_capacity(IN_FLIGHT);
        for n in unsent.by_ref().take(IN_FLIGHT) {
            outstanding.push_back(self.send_request(|tag, packet| encode(n, tag, packet))?);
        }
        for n in 0..count {
            let sent = outstanding.pop_front().expect("a request sent for each reply still to come");
            let mut reply = [0; PACKET];
            take(n, self.receive_reply(sent, &mut reply)?)?;
            if let Some(next) = unsent.next() {
                outstanding.push_back(self.send_request(|tag, packet| encode(next, tag, packet))?);
            }
        }
        Ok(())
    }

    /// Encodes and exchanges `request`; returns the reply's result.
    fn request<'r>(&mut self, request: &Request<'_>, reply: &'r mut [u8; PACKET]) -> Result<&'r [u8], Error> {
        self.exchange(|tag, packet| request.encode(tag, packet), reply)
    }

    /// Sends the request `encode` writes, with the next tag, and returns
    /// its reply's result.
    fn exchange<'r>(
        &mut self,
        encode: impl FnOnce(u8, &mut [u8; PACKET]) -> Result<usize, ParseError>,
        reply: &'r mut [u8; PACKET],
    ) -> Result<&'r [u8], Error> {
        let sent = self.send_request(encode)?;
        self.receive_reply(sent, reply)
    }

    /// Sends the request `encode` writes, with the next tag, without
    /// waiting for its reply.
    fn send_request(
        &mut self,
        encode: impl FnOnce(u8, &mut [u8; PACKET]) -> Result<usize, ParseError>,
    ) -> Result<Sent, Error> {
        self.tag = self.tag % u8::MAX + 1;
        let mut packet = [0; PACKET];
        let length = encode(self.tag, &mut packet)?;
        let request = Request::parse(&packet[1..length])?;
        let timeout = timeout(Workload::of(&request), self.dsclk_hz);
        self.transport.send(&packet[..length])?;
        Ok(Sent { tag: self.tag, timeout })
    }

    /// Waits for the reply to the request `sent`, its timeout running
    /// from now, and returns its result.  Replies with another tag are
    /// stale (left by a request this host gave up on, or sent behind a
    /// failed one) and discarded.
    fn receive_reply<'r>(&mut self, sent: Sent, reply: &'r mut [u8; PACKET]) -> Result<&'r [u8], Error> {
        let deadline = Instant::now() + sent.timeout;
        let length = loop {
            let length = self.receive(reply, deadline, sent.timeout)?;
            if let Ok((header, _)) = parse_reply_header(&reply[..length])
                && header.tag == sent.tag
            {
                self.freeze_edges = Some(header.freeze_edges);
                break length;
            }
        };
        let (_, message) = parse_reply_header(&reply[..length])?;
        Ok(parse_reply(message)?)
    }

    /// The next reply packet, by `deadline`; `timeout` is what it is
    /// reported as.
    fn receive(
        &mut self,
        reply: &mut [u8; PACKET],
        deadline: Instant,
        timeout: Duration,
    ) -> Result<usize, Error> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.transport.receive(reply, remaining) {
            Err(TransportError::NoReply(_)) => Err(TransportError::NoReply(timeout).into()),
            received => Ok(received?),
        }
    }
}

/// A request sent and not yet answered.
#[derive(Debug, Clone, Copy)]
struct Sent {
    /// Its tag, which its reply echoes.
    tag: u8,
    /// How long its reply may take, from when the host starts waiting.
    timeout: Duration,
}

/// The chunks, a request each, a block of `length` bytes goes in.
fn chunk_count(length: usize) -> usize {
    length.div_ceil(MAX_MEMORY_BYTES)
}

/// The `n`th chunk of a block of `length` bytes: chunks start
/// [`MAX_MEMORY_BYTES`] apart, a whole number of longs, so a chunk
/// never splits an access.
fn chunk(length: usize, n: usize) -> Range<usize> {
    let start = n * MAX_MEMORY_BYTES;
    start..length.min(start + MAX_MEMORY_BYTES)
}

/// A reply body that should be empty.
fn empty(body: &[u8]) -> Result<(), Error> {
    if body.is_empty() { Ok(()) } else { Err(ReplyError::Malformed.into()) }
}

fn at(Address(base): Address, offset: usize) -> Address {
    Address(base.wrapping_add(u32::try_from(offset).expect("a block fits the address space")))
}

fn check_length(size: Size, length: usize) -> Result<(), Error> {
    if length.is_multiple_of(byte_count(size)) { Ok(()) } else { Err(Error::Length { length, size }) }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use embassy_futures::block_on;
    use mc683xx_bdm_model::Target;
    use mc683xx_bdm_model::board::ModelBoard;
    use mc683xx_bdm_proto::dsclk::MAX_HZ;
    use mc683xx_bdm_proto::dsclk::MIN_HZ;
    use mc683xx_bdm_proto::pod::NOT_READY_RETRIES;
    use mc683xx_bdm_proto::pod::NOT_READY_TIMEOUT_US;
    use mc683xx_bdm_proto::pod::Workload;
    use mc683xx_bdm_proto::session::Link;
    use mc683xx_bdm_proto::usb::HELLO_REQUEST;
    use mc683xx_bdm_proto::usb::MAX_MEMORY_BYTES;
    use mc683xx_bdm_proto::usb::PACKET;
    use mc683xx_bdm_proto::usb::PROTOCOL_VERSION;
    use mc683xx_bdm_proto::usb::ReplyError;
    use mc683xx_bdm_proto::usb::Request;
    use mc683xx_bdm_proto::usb::Status;
    use mc683xx_target::entry::Entry;

    use super::Address;
    use super::Atemp;
    use super::Error;
    use super::IN_FLIGHT;
    use super::Pod;
    use super::Register;
    use super::Size;
    use super::TIMEOUT;
    use super::timeout;
    use crate::script::parsed;
    use crate::script::pod;
    use crate::script::pod_with_edges;
    use crate::transport::Transport;

    use crate::transport::TransportError;

    /// A pod whose replies come late: those to requests a host gave up
    /// on wait ahead of the next.  Each request is answered with its tag
    /// as a register's value, after `late` requests' worth of delay.
    struct Late {
        /// Replies not yet received.
        waiting: VecDeque<Vec<u8>>,
        /// Requests whose reply has not come yet.
        late: usize,
        /// Replies held back by `late`.
        held: VecDeque<Vec<u8>>,
    }

    impl Transport for Late {
        fn send(&mut self, request: &[u8]) -> Result<(), TransportError> {
            let tag = request[0];
            self.held.push_back(vec![tag, 0, Status::Ok as u8, tag, 0, 0, 0]);
            if self.late > 0 {
                self.late -= 1;
            } else {
                self.waiting.extend(self.held.drain(..));
            }
            Ok(())
        }

        fn receive(&mut self, reply: &mut [u8; PACKET], timeout: Duration) -> Result<usize, TransportError> {
            let next = self.waiting.pop_front().ok_or(TransportError::NoReply(timeout))?;
            reply[..next.len()].copy_from_slice(&next);
            Ok(next.len())
        }
    }

    #[test]
    fn a_reply_left_by_a_request_given_up_on_is_discarded() {
        let mut pod = Pod::new(Late { waiting: VecDeque::new(), late: 1, held: VecDeque::new() });
        assert!(matches!(pod.read_register(Register::D0), Err(Error::Transport(TransportError::NoReply(_)))));
        // The first request's reply comes ahead of the second's, tagged 1.
        assert_eq!(pod.read_register(Register::D1).unwrap(), 2, "the second request's own");
        assert_eq!(pod.read_register(Register::D2).unwrap(), 3);
    }

    #[test]
    fn stale_replies_waiting_before_hello_are_discarded() {
        let stale = vec![vec![7, 0, Status::Ok as u8, 1, 0, 0, 0], vec![0, 0, Status::BadRequest as u8]];
        let mut pod = Pod::new(Late { waiting: stale.into(), late: 0, held: VecDeque::new() });
        // Hello's reply is its own form; the script below has none, so
        // the stale ones are read and discarded until it gives up.
        assert!(matches!(pod.hello(), Err(Error::Transport(TransportError::NoReply(_)))));
        assert_eq!(pod.into_transport().waiting.len(), 0);
    }

    #[test]
    fn tags_run_from_1_to_255_and_round_again() {
        let mut pod = Pod::new(Late { waiting: VecDeque::new(), late: 0, held: VecDeque::new() });
        for expected in (1..=255).chain(1..=2) {
            assert_eq!(pod.read_register(Register::D0).unwrap(), expected);
        }
    }

    /// The pod's time over `request` on a model whose every access
    /// answers "not ready" just short of the pod's limit at `dsclk_hz`,
    /// against the host's timeout for it.
    fn pod_time_and_timeout(request: &Request<'_>, dsclk_hz: u32) -> (Duration, Duration) {
        let mut target = Target::<256>::new(Address(0x1000));
        target.enter_background(Entry::Breakpoint);
        let word_us = 17 * 1_000_000 / dsclk_hz;
        target.memory_busy = NOT_READY_RETRIES.min(NOT_READY_TIMEOUT_US / word_us - 1);
        let mut pod = mc683xx_bdm_proto::pod::Pod::new(ModelBoard::new(target, true));
        let mut packet = [0; PACKET];
        let mut reply = [0; PACKET];
        let length = Request::SetClock { dsclk_hz }.encode(1, &mut packet).unwrap();
        block_on(pod.handle(&packet[..length], &mut reply));
        let length = request.encode(2, &mut packet).unwrap();
        let start = pod.board_mut().clock_us();
        block_on(pod.handle(&packet[..length], &mut reply));
        assert_eq!(reply[2], Status::Ok as u8, "{request:?} at {dsclk_hz} Hz");
        let spent = Duration::from_micros(u64::from(pod.board_mut().clock_us().wrapping_sub(start)));
        (spent, timeout(Workload::of(request), dsclk_hz))
    }

    #[test]
    fn the_timeout_covers_the_slowest_request_at_each_rate() {
        let data = [0; MAX_MEMORY_BYTES];
        for dsclk_hz in [MIN_HZ, 100_000, 1_000_000, MAX_HZ] {
            for request in [
                Request::ReadMemory { address: Address(0x1000), size: Size::Byte, length: MAX_MEMORY_BYTES },
                Request::WriteMemory { address: Address(0x1000), size: Size::Byte, data: &data },
                Request::ReadRegister(Register::D0),
            ] {
                let (spent, timeout) = pod_time_and_timeout(&request, dsclk_hz);
                assert!(spent + TIMEOUT <= timeout, "{request:?} at {dsclk_hz} Hz: {spent:?}, {timeout:?}");
            }
        }
    }

    /// A pod that says Hello and answers `GetClock` with `dsclk_hz`, if
    /// it has one, and nothing else: every other request times out.
    struct Clocked {
        dsclk_hz: Option<u32>,
        /// The reply not yet received.
        waiting: Option<Vec<u8>>,
    }

    impl Transport for Clocked {
        fn send(&mut self, request: &[u8]) -> Result<(), TransportError> {
            self.waiting = if request == HELLO_REQUEST {
                Some([Status::Ok as u8].into_iter().chain(PROTOCOL_VERSION.to_le_bytes()).collect())
            } else if let (Request::GetClock, Some(dsclk_hz)) = (parsed(request), self.dsclk_hz) {
                Some([request[0], 0, Status::Ok as u8].into_iter().chain(dsclk_hz.to_le_bytes()).collect())
            } else {
                None
            };
            Ok(())
        }

        fn receive(&mut self, reply: &mut [u8; PACKET], timeout: Duration) -> Result<usize, TransportError> {
            let next = self.waiting.take().ok_or(TransportError::NoReply(timeout))?;
            reply[..next.len()].copy_from_slice(&next);
            Ok(next.len())
        }
    }

    /// The timeout a request's `result` reports, the pod not having
    /// answered.
    fn reported_timeout(result: Result<impl std::fmt::Debug, Error>) -> Duration {
        match result {
            Err(Error::Transport(TransportError::NoReply(timeout))) => timeout,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_handshake_asks_the_rate_and_times_out_by_it() {
        let read = Request::ReadRegister(Register::D0);
        for dsclk_hz in [MIN_HZ, 1_000_000, MAX_HZ] {
            let mut pod = Pod::new(Clocked { dsclk_hz: Some(dsclk_hz), waiting: None });
            pod.handshake().unwrap();
            assert_eq!(pod.dsclk_hz, dsclk_hz);
            let timeout_reported = reported_timeout(pod.read_register(Register::D0));
            assert_eq!(timeout_reported, timeout(Workload::of(&read), dsclk_hz), "at {dsclk_hz} Hz");
        }
    }

    #[test]
    fn get_clock_is_timed_at_the_slowest() {
        let mut pod = Pod::new(Clocked { dsclk_hz: None, waiting: None });
        let timeout_reported = reported_timeout(pod.handshake());
        assert_eq!(timeout_reported, timeout(Workload::of(&Request::GetClock), MIN_HZ));
        assert_eq!(pod.dsclk_hz, MIN_HZ, "the slowest until the pod says");
    }

    #[test]
    fn the_timeout_follows_the_rate_set() {
        let mut pod = pod(&[&[0, 0x10, 0x27, 0, 0], &[0, 0x00, 0x09, 0x3D, 0], &[0, 0xA0, 0x86, 0x01, 0]]);
        assert_eq!(pod.dsclk_hz, MIN_HZ, "the slowest until the pod says");
        pod.set_clock(1).unwrap();
        assert_eq!(pod.dsclk_hz, 10_000);
        pod.set_clock(4_000_000).unwrap();
        assert_eq!(pod.dsclk_hz, 4_000_000);
        assert_eq!(pod.get_clock().unwrap(), 100_000);
        assert_eq!(pod.dsclk_hz, 100_000, "as the pod gives it");
        let read = Workload::of(&Request::ReadMemory { address: Address(0), size: Size::Byte, length: 56 });
        assert!(timeout(read, MAX_HZ) < timeout(read, MIN_HZ));
        assert!(timeout(read, MIN_HZ) < Duration::from_secs(4), "a lost pod is reported in seconds");
    }

    #[test]
    fn replies_give_freeze_edges() {
        let mut pod = pod_with_edges(&[(9, &[0])]);
        assert_eq!(pod.freeze_edges(), None);
        pod.go().unwrap();
        assert_eq!(pod.freeze_edges(), Some(9));
    }

    #[test]
    fn halt_returns_atemp() {
        let mut pod = pod(&[&[0, 1, 1, 0, 0, 0]]);
        assert_eq!(pod.halt().unwrap(), Atemp::Fresh(1));
        assert_eq!(parsed(&pod.into_transport().requests[0]), Request::Halt);
    }

    #[test]
    fn read_register() {
        let mut pod = pod(&[&[0, 0x78, 0x56, 0x34, 0x12]]);
        assert_eq!(pod.read_register(Register::A7).unwrap(), 0x1234_5678);
        assert_eq!(parsed(&pod.into_transport().requests[0]), Request::ReadRegister(Register::A7));
    }

    #[test]
    fn long_memory_reads_go_in_chunks() {
        let reply = |bytes: std::ops::Range<u8>| -> Vec<u8> { std::iter::once(0).chain(bytes).collect() };
        let mut pod = pod(&[&reply(0..56), &reply(56..112), &reply(112..120)]);
        let mut data = [0; 120];
        pod.read_memory(Address(0x1000), Size::Long, &mut data).unwrap();
        assert!(data.iter().enumerate().all(|(n, b)| usize::from(*b) == n));
        let requests: Vec<_> = pod
            .into_transport()
            .requests
            .iter()
            .map(|r| match parsed(r) {
                Request::ReadMemory { address, size: Size::Long, length } => (address, length),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(requests, [(Address(0x1000), 56), (Address(0x1038), 56), (Address(0x1070), 8)]);
    }

    #[test]
    fn long_memory_writes_go_in_chunks() {
        let mut pod = pod(&[&[0], &[0], &[0]]);
        let data: Vec<u8> = (0..=129).collect();
        pod.write_memory(Address(0x2000), Size::Word, &data).unwrap();
        let requests = pod.into_transport().requests;
        let lengths: Vec<_> = requests
            .iter()
            .map(|r| match parsed(r) {
                Request::WriteMemory { address, data, .. } => (address, data.len()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(lengths, [(Address(0x2000), 56), (Address(0x2038), 56), (Address(0x2070), 18)]);
    }

    /// What the host did, in order, as [`Queued`] saw it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Event {
        /// Sent the request with this tag.
        Sent(u8),
        /// Received the reply with this tag.
        Received(u8),
    }

    /// A pod that takes requests in order, one at a time, answering
    /// each only as the host reads, as the pod's writing a reply before
    /// reading the next request makes it.  A memory read gives each
    /// byte's address, low 8 bits; a register read its tag; the memory
    /// request at `fail_at` fails with `NoFreeze`.
    struct Queued {
        /// Requests sent and not yet answered, in order.
        unanswered: VecDeque<Vec<u8>>,
        /// The host's sends and receives.
        log: Vec<Event>,
        /// The address of the memory request that fails.
        fail_at: Option<Address>,
        /// The addresses of the memory writes run, in order.
        written: Vec<Address>,
    }

    impl Queued {
        fn new(fail_at: Option<Address>) -> Self {
            Self { unanswered: VecDeque::new(), log: Vec::new(), fail_at, written: Vec::new() }
        }

        /// The most requests unanswered at once.
        fn most_outstanding(&self) -> usize {
            let (_, most) = self.log.iter().fold((0_usize, 0), |(now, most), event| match event {
                Event::Sent(_) => (now + 1, most.max(now + 1)),
                Event::Received(_) => (now - 1, most),
            });
            most
        }

        /// The tags of the requests sent, in order.
        fn sent(&self) -> Vec<u8> {
            self.log
                .iter()
                .filter_map(|event| match event {
                    Event::Sent(tag) => Some(*tag),
                    Event::Received(_) => None,
                })
                .collect()
        }
    }

    impl Transport for Queued {
        fn send(&mut self, request: &[u8]) -> Result<(), TransportError> {
            self.log.push(Event::Sent(request[0]));
            self.unanswered.push_back(request.to_vec());
            Ok(())
        }

        fn receive(&mut self, reply: &mut [u8; PACKET], timeout: Duration) -> Result<usize, TransportError> {
            let request = self.unanswered.pop_front().ok_or(TransportError::NoReply(timeout))?;
            let tag = request[0];
            self.log.push(Event::Received(tag));
            let (status, body): (Status, Vec<u8>) = match parsed(&request) {
                Request::ReadMemory { address, .. } | Request::WriteMemory { address, .. }
                    if Some(address) == self.fail_at =>
                {
                    (Status::NoFreeze, Vec::new())
                }
                Request::ReadMemory { address: Address(base), length, .. } => {
                    (Status::Ok, (0..length).map(|n| (base as usize + n) as u8).collect())
                }
                Request::WriteMemory { address, .. } => {
                    self.written.push(address);
                    (Status::Ok, Vec::new())
                }
                Request::ReadRegister(_) => (Status::Ok, u32::from(tag).to_le_bytes().to_vec()),
                other => panic!("{other:?}"),
            };
            let message: Vec<u8> = [tag, 0, status as u8].into_iter().chain(body).collect();
            reply[..message.len()].copy_from_slice(&message);
            Ok(message.len())
        }
    }

    /// The log of a block of `chunks` requests all answered: `IN_FLIGHT`
    /// sent, then each reply followed by the next request, if any.
    fn pipelined(chunks: u8) -> Vec<Event> {
        let in_flight = u8::try_from(IN_FLIGHT).unwrap();
        (1..=in_flight.min(chunks))
            .map(Event::Sent)
            .chain((1..=chunks).flat_map(|n| {
                std::iter::once(Event::Received(n))
                    .chain((n + in_flight <= chunks).then_some(Event::Sent(n + in_flight)))
            }))
            .collect()
    }

    #[test]
    fn memory_reads_keep_requests_in_flight() {
        let mut pod = Pod::new(Queued::new(None));
        let mut data = vec![0; 10 * MAX_MEMORY_BYTES - 4];
        pod.read_memory(Address(0x1000), Size::Long, &mut data).unwrap();
        assert!(data.iter().enumerate().all(|(n, b)| *b == (0x1000 + n) as u8));
        let transport = pod.into_transport();
        assert_eq!(transport.log, pipelined(10));
        assert_eq!(transport.most_outstanding(), IN_FLIGHT);
    }

    #[test]
    fn memory_writes_go_one_at_a_time() {
        let mut pod = Pod::new(Queued::new(None));
        pod.write_memory(Address(0x2000), Size::Word, &[0; 6 * MAX_MEMORY_BYTES]).unwrap();
        let transport = pod.into_transport();
        let expected: Vec<_> = (1..=6).flat_map(|n| [Event::Sent(n), Event::Received(n)]).collect();
        assert_eq!(transport.log, expected);
        assert_eq!(transport.most_outstanding(), 1);
        let expected: Vec<_> = (0..6).map(|n| Address(0x2000 + n * 56)).collect();
        assert_eq!(transport.written, expected);
    }

    #[test]
    fn a_block_shorter_than_in_flight_sends_each_chunk_at_once() {
        let mut pod = Pod::new(Queued::new(None));
        let mut data = [0; 2 * MAX_MEMORY_BYTES];
        pod.read_memory(Address(0), Size::Byte, &mut data).unwrap();
        let log = pod.into_transport().log;
        assert_eq!(log, [Event::Sent(1), Event::Sent(2), Event::Received(1), Event::Received(2)]);
    }

    #[test]
    fn a_failure_mid_block_stops_sending_and_later_exchanges_discard_its_stale_replies() {
        // The third chunk fails.
        let mut pod = Pod::new(Queued::new(Some(Address(0x1000 + 2 * 56))));
        let mut data = vec![0; 10 * MAX_MEMORY_BYTES];
        assert!(matches!(
            pod.read_memory(Address(0x1000), Size::Byte, &mut data),
            Err(Error::Reply(ReplyError::Failed(Status::NoFreeze)))
        ));
        let failed_at = 3;
        let sent_before = failed_at + u8::try_from(IN_FLIGHT).unwrap() - 1;
        assert_eq!(pod.transport.sent(), (1..=sent_before).collect::<Vec<_>>(), "none after the failure");
        assert_eq!(pod.transport.unanswered.len(), IN_FLIGHT - 1, "stale, behind the failed one");
        assert_eq!(pod.read_register(Register::D0).unwrap(), u32::from(sent_before + 1), "its own reply");
        let after: Vec<_> = pod.transport.log.iter().rev().take(IN_FLIGHT + 1).rev().copied().collect();
        let expected: Vec<_> = (failed_at + 1..=sent_before)
            .map(Event::Received)
            .chain([Event::Received(sent_before + 1)])
            .collect();
        assert_eq!(after[1..], expected, "the stale replies, then its own");
        assert_eq!(after[0], Event::Sent(sent_before + 1));
    }

    #[test]
    fn a_failed_write_writes_nothing_past_it() {
        // The second chunk fails.
        let mut pod = Pod::new(Queued::new(Some(Address(0x2000 + 56))));
        assert!(matches!(
            pod.write_memory(Address(0x2000), Size::Byte, &[0; 8 * MAX_MEMORY_BYTES]),
            Err(Error::Reply(ReplyError::Failed(Status::NoFreeze)))
        ));
        assert_eq!(pod.transport.sent(), [1, 2], "none after the failure");
        assert_eq!(pod.into_transport().written, [Address(0x2000)]);
    }

    #[test]
    fn memory_length_must_be_whole_accesses() {
        let mut pod = pod(&[]);
        let mut data = [0; 6];
        assert!(matches!(
            pod.read_memory(Address(0), Size::Long, &mut data),
            Err(Error::Length { length: 6, size: Size::Long })
        ));
    }

    #[test]
    fn short_memory_reply_is_malformed() {
        let mut pod = pod(&[&[0, 1, 2]]);
        let mut data = [0; 4];
        assert!(matches!(
            pod.read_memory(Address(0), Size::Byte, &mut data),
            Err(Error::Reply(ReplyError::Malformed))
        ));
    }

    #[test]
    fn pod_failures_come_back() {
        let mut pod = pod(&[&[Status::NoFreeze as u8]]);
        assert!(matches!(pod.go(), Err(Error::Reply(ReplyError::Failed(Status::NoFreeze)))));
    }

    #[test]
    fn long_transfers_go_in_chunks() {
        let reply = |n: usize| -> Vec<u8> {
            std::iter::once(0).chain((0..n).flat_map(|w| u32::try_from(w).unwrap().to_le_bytes())).collect()
        };
        let mut pod = pod(&[&reply(15), &reply(5)]);
        let responses = pod.transfer(&[0; 20]).unwrap();
        assert_eq!(responses.len(), 20);
        assert_eq!(pod.into_transport().requests.len(), 2);
    }

    #[test]
    fn another_protocol_is_refused() {
        let reply = |version: u32| -> Vec<u8> { std::iter::once(0).chain(version.to_le_bytes()).collect() };
        let mut pod = pod(&[&reply(PROTOCOL_VERSION), &reply(MIN_HZ), &reply(PROTOCOL_VERSION + 1)]);
        pod.handshake().unwrap();
        assert!(matches!(pod.handshake(), Err(Error::Protocol(version)) if version == PROTOCOL_VERSION + 1));
        let requests = pod.into_transport().requests;
        assert_eq!(requests.len(), 3, "no GetClock after another version's Hello");
        assert_eq!(parsed(&requests[1]), Request::GetClock);
    }
}
