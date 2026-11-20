//! The pod's requests as calls: each encodes a request, exchanges it
//! over the transport, and decodes the reply.  Memory and raw transfers
//! longer than a packet go as several requests.

use mc683xx_target::address::Address;
use mc683xx_target::function_code::FunctionCode;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;
use pod_proto::bdm::byte_count;
use pod_proto::usb::Atemp;
use pod_proto::usb::MAX_MEMORY_BYTES;
use pod_proto::usb::MAX_TRANSFER_WORDS;
use pod_proto::usb::PACKET;
use pod_proto::usb::PROTOCOL_VERSION;
use pod_proto::usb::ParseError;
use pod_proto::usb::Pins;
use pod_proto::usb::ReplyError;
use pod_proto::usb::Request;
use pod_proto::usb::encode_transfer;
use pod_proto::usb::parse_atemp;
use pod_proto::usb::parse_pins;
use pod_proto::usb::parse_reply;
use pod_proto::usb::parse_transfer_reply;
use pod_proto::usb::parse_value;

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
    /// A call the pod cannot make yet.
    #[error("{0} is not supported yet")]
    Unsupported(&'static str),
}

/// The function code `bits` gives, if it fits FC\[2:0\].
#[must_use]
pub fn function_code(bits: u32) -> Option<FunctionCode> {
    let code = FunctionCode::new(bits);
    (u32::from(code.bits()) == bits).then_some(code)
}

/// The pod, over a transport.
#[derive(Debug)]
pub struct Pod<T> {
    transport: T,
}

impl<T: Transport> Pod<T> {
    /// The pod at the end of `transport`.
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }

    /// The transport, to give it back.
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// The pod's protocol version.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn hello(&mut self) -> Result<u32, Error> {
        self.value(&Request::Hello)
    }

    /// Says Hello, and checks the pod speaks this host's protocol.
    ///
    /// # Errors
    /// As [`Error`]; [`Error::Protocol`] for another version.
    pub fn check_protocol(&mut self) -> Result<(), Error> {
        let version = self.hello()?;
        if version == PROTOCOL_VERSION { Ok(()) } else { Err(Error::Protocol(version)) }
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
        self.value(&Request::SetClock { dsclk_hz })
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
    /// through SFC, as many requests as it takes.
    ///
    /// # Errors
    /// As [`Error`]; a length that is not whole accesses.
    pub fn read_memory(&mut self, address: Address, size: Size, data: &mut [u8]) -> Result<(), Error> {
        check_length(size, data.len())?;
        for (offset, chunk) in chunks(data.len()).zip(data.chunks_mut(MAX_MEMORY_BYTES)) {
            let request = Request::ReadMemory { address: at(address, offset), size, length: chunk.len() };
            let mut reply = [0; PACKET];
            let body = self.request(&request, &mut reply)?;
            if body.len() != chunk.len() {
                return Err(ReplyError::Malformed.into());
            }
            chunk.copy_from_slice(body);
        }
        Ok(())
    }

    /// Writes `data` at `address` in `size` accesses, through DFC, as
    /// many requests as it takes.
    ///
    /// # Errors
    /// As [`Error`]; a length that is not whole accesses.
    pub fn write_memory(&mut self, address: Address, size: Size, data: &[u8]) -> Result<(), Error> {
        check_length(size, data.len())?;
        for (offset, chunk) in chunks(data.len()).zip(data.chunks(MAX_MEMORY_BYTES)) {
            self.done(&Request::WriteMemory { address: at(address, offset), size, data: chunk })?;
        }
        Ok(())
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
            let mut packet = [0; PACKET];
            let length = encode_transfer(chunk, &mut packet)?;
            let mut reply = [0; PACKET];
            let body = self.exchange(&packet[..length], &mut reply)?;
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
        if body.is_empty() { Ok(()) } else { Err(ReplyError::Malformed.into()) }
    }

    /// Encodes and exchanges `request`; returns the reply's result.
    fn request<'r>(&mut self, request: &Request<'_>, reply: &'r mut [u8; PACKET]) -> Result<&'r [u8], Error> {
        let mut packet = [0; PACKET];
        let length = request.encode(&mut packet)?;
        self.exchange(&packet[..length], reply)
    }

    fn exchange<'r>(&mut self, packet: &[u8], reply: &'r mut [u8; PACKET]) -> Result<&'r [u8], Error> {
        let length = self.transport.exchange(packet, reply)?;
        Ok(parse_reply(&reply[..length])?)
    }
}

/// Each chunk's offset, [`MAX_MEMORY_BYTES`] apart, a whole number of
/// longs, so a chunk never splits an access.
fn chunks(length: usize) -> impl Iterator<Item = usize> {
    (0..length).step_by(MAX_MEMORY_BYTES)
}

fn at(Address(base): Address, offset: usize) -> Address {
    Address(base.wrapping_add(u32::try_from(offset).expect("a block fits the address space")))
}

fn check_length(size: Size, length: usize) -> Result<(), Error> {
    if length.is_multiple_of(byte_count(size)) { Ok(()) } else { Err(Error::Length { length, size }) }
}

#[cfg(test)]
mod tests {
    use mc683xx_target::function_code::FunctionCode;
    use pod_proto::usb::PROTOCOL_VERSION;
    use pod_proto::usb::ReplyError;
    use pod_proto::usb::Request;
    use pod_proto::usb::Status;

    use super::Address;
    use super::Atemp;
    use super::Error;
    use super::Register;
    use super::Size;
    use super::function_code;
    use crate::script::parsed;
    use crate::script::pod;

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
        let mut pod = pod(&[&reply(PROTOCOL_VERSION), &reply(PROTOCOL_VERSION + 1)]);
        pod.check_protocol().unwrap();
        assert!(
            matches!(pod.check_protocol(), Err(Error::Protocol(version)) if version == PROTOCOL_VERSION + 1)
        );
    }

    #[test]
    fn function_codes() {
        assert_eq!(function_code(5), Some(FunctionCode::SUPERVISOR_DATA));
        assert_eq!(function_code(7), Some(FunctionCode::CPU_SPACE));
        assert_eq!(function_code(8), None);
        assert_eq!(function_code(0xD), None);
    }
}
