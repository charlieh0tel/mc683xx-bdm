//! Host-to-pod messages: one request per bulk OUT packet, one reply per
//! bulk IN packet, little-endian.  A request starts with its code, a
//! reply with its status; what follows depends on the request.

use mc683xx_target::address::Address;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;

use crate::bdm::byte_count;
use crate::session;

/// pid.codes' VID, with its test PID below, for private use until the
/// pod has its own (PLAN.md, Open questions).
pub const VID: u16 = 0x1209;
/// pid.codes' test PID.
pub const PID: u16 = 0x0001;

/// A full-speed bulk packet.
pub const PACKET: usize = 64;

/// This protocol's version, in the Hello reply: raised on any change
/// to a request or reply (docs/usb-protocol.md).
pub const PROTOCOL_VERSION: u32 = 4;

/// Bytes a reply's status takes.
const STATUS_BYTES: usize = 1;
/// Bytes per 17-bit word in a Transfer reply.
const REPLY_WORD_BYTES: usize = 4;
/// Words in one Transfer: its reply, a status and 4 bytes a word,
/// fills at most a packet.
pub const MAX_TRANSFER_WORDS: usize = (PACKET - STATUS_BYTES) / REPLY_WORD_BYTES;
/// Bytes in one memory read or write: a write's code, address, size and
/// data fit a packet, and it is a whole number of longs.
pub const MAX_MEMORY_BYTES: usize = 56;

/// Request codes.
const TRANSFER: u8 = 0x01;
const PINS: u8 = 0x02;
const RESET_HALT: u8 = 0x03;
const HALT: u8 = 0x04;
const GO: u8 = 0x05;
const SET_CLOCK: u8 = 0x06;
const READ_REGISTER: u8 = 0x07;
const WRITE_REGISTER: u8 = 0x08;
const READ_SYSTEM_REGISTER: u8 = 0x09;
const WRITE_SYSTEM_REGISTER: u8 = 0x0A;
const READ_MEMORY: u8 = 0x0B;
const WRITE_MEMORY: u8 = 0x0C;
const CALL: u8 = 0x0D;
const HELLO: u8 = 0x0E;

/// Pin level bits in a Pins reply: set for high.
const FREEZE: u8 = 1 << 0;
const RESET: u8 = 1 << 1;
const DS: u8 = 1 << 2;
const BERR: u8 = 1 << 3;

/// Bytes in a 32-bit field.
const LONG: usize = 4;

/// The kind byte of an ATEMP field (`Pins`, `Halt`, `ResetHalt` replies).
const ATEMP_NONE: u8 = 0;
const ATEMP_FRESH: u8 = 1;
const ATEMP_STALE: u8 = 2;
const ATEMP_UNREAD: u8 = 3;
/// An ATEMP field: its kind byte, then the value.
const ATEMP_BYTES: usize = 1 + LONG;
/// A Pins reply's body: levels, VDD, ATEMP.
const PINS_BYTES: usize = 3 + ATEMP_BYTES;

/// What the host asks of the pod.  Everything from `ReadRegister` on
/// needs the target in background mode, and so do Go and Call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request<'a> {
    /// Raw serial words, each answered with the 17 bits shifted in: for
    /// bring-up, and for commands the pod does not sequence itself.
    Transfer(TransferWords<'a>),
    /// The target's pin levels and VDD.
    Pins,
    /// Reset the target into background mode; the reply is ATEMP.
    ResetHalt,
    /// Stop the running target in background mode; the reply is ATEMP.
    Halt,
    /// GO: resume at RPC.
    Go,
    /// Set DSCLK's rate; the reply gives the rate set.
    SetClock {
        /// The rate asked for.
        dsclk_hz: u32,
    },
    /// A data or address register; the reply is its value.
    ReadRegister(Register),
    /// Write a data or address register.
    WriteRegister(Register, u32),
    /// A system register; the reply is its value.
    ReadSystemRegister(SystemRegister),
    /// Write a system register.
    WriteSystemRegister(SystemRegister, u32),
    /// Read memory through SFC, READ then DUMP; the reply is the bytes.
    ReadMemory {
        /// The first byte's address.
        address: Address,
        /// The access size.
        size: Size,
        /// Bytes, a multiple of `size` and at most [`MAX_MEMORY_BYTES`].
        length: usize,
    },
    /// Write memory through DFC, WRITE then FILL.
    WriteMemory {
        /// The first byte's address.
        address: Address,
        /// The access size.
        size: Size,
        /// The bytes, a multiple of `size` and at most
        /// [`MAX_MEMORY_BYTES`].
        data: &'a [u8],
    },
    /// CALL: stack RPC and run from an address.
    Call(Address),
    /// The protocol version, [`PROTOCOL_VERSION`]; first, to check the
    /// pod speaks this protocol.
    Hello,
}

/// A Transfer's words, as they lie in the packet: two bytes each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferWords<'a>(&'a [u8]);

impl TransferWords<'_> {
    /// The words, in order.
    pub fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        self.0.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair))
    }

    /// How many words.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len() / 2
    }

    /// No words.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A request the pod cannot take, or the host cannot send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// An empty packet.
    #[error("empty request")]
    Empty,
    /// A code the pod does not know.
    #[error("unknown request {0:#04x}")]
    Unknown(u8),
    /// A request of the wrong length for its code.
    #[error("request {code:#04x} is {length} bytes")]
    Length {
        /// The request's code.
        code: u8,
        /// The packet's length.
        length: usize,
    },
    /// A register, size or memory length out of range.
    #[error("request {0:#04x} has a field out of range")]
    Field(u8),
}

impl<'a> Request<'a> {
    /// Reads a request from a bulk OUT packet.
    ///
    /// # Errors
    /// The packet is not a request.
    pub fn parse(packet: &'a [u8]) -> Result<Self, ParseError> {
        let (&code, body) = packet.split_first().ok_or(ParseError::Empty)?;
        let length = ParseError::Length { code, length: packet.len() };
        let field = ParseError::Field(code);
        let empty = |request| if body.is_empty() { Ok(request) } else { Err(length) };
        match code {
            TRANSFER => {
                if body.is_empty() || !body.len().is_multiple_of(2) || body.len() / 2 > MAX_TRANSFER_WORDS {
                    return Err(length);
                }
                Ok(Self::Transfer(TransferWords(body)))
            }
            PINS => empty(Self::Pins),
            RESET_HALT => empty(Self::ResetHalt),
            HALT => empty(Self::Halt),
            GO => empty(Self::Go),
            SET_CLOCK => Ok(Self::SetClock { dsclk_hz: long(exact::<LONG>(body).ok_or(length)?) }),
            READ_REGISTER => {
                let [register] = exact::<1>(body).ok_or(length)?;
                Ok(Self::ReadRegister(register_from(register).ok_or(field)?))
            }
            WRITE_REGISTER => {
                let [register, value @ ..] = exact::<5>(body).ok_or(length)?;
                Ok(Self::WriteRegister(register_from(register).ok_or(field)?, long(value)))
            }
            READ_SYSTEM_REGISTER => {
                let [register] = exact::<1>(body).ok_or(length)?;
                Ok(Self::ReadSystemRegister(system_register_from(register).ok_or(field)?))
            }
            WRITE_SYSTEM_REGISTER => {
                let [register, value @ ..] = exact::<5>(body).ok_or(length)?;
                Ok(Self::WriteSystemRegister(system_register_from(register).ok_or(field)?, long(value)))
            }
            READ_MEMORY => {
                let [a, b, c, d, size, bytes] = exact::<6>(body).ok_or(length)?;
                let size = size_from(size).ok_or(field)?;
                let length = usize::from(bytes);
                memory_length_ok(size, length).then_some(()).ok_or(field)?;
                Ok(Self::ReadMemory { address: Address(long([a, b, c, d])), size, length })
            }
            WRITE_MEMORY => {
                let (head, data) = body.split_at_checked(LONG + 1).ok_or(length)?;
                let [a, b, c, d, size] = exact::<5>(head).ok_or(length)?;
                let size = size_from(size).ok_or(field)?;
                memory_length_ok(size, data.len()).then_some(()).ok_or(field)?;
                Ok(Self::WriteMemory { address: Address(long([a, b, c, d])), size, data })
            }
            CALL => Ok(Self::Call(Address(long(exact::<LONG>(body).ok_or(length)?)))),
            HELLO => empty(Self::Hello),
            other => Err(ParseError::Unknown(other)),
        }
    }

    /// Writes the request into `packet`, returning its length.
    ///
    /// # Errors
    /// A Transfer of no words or more than [`MAX_TRANSFER_WORDS`], or a
    /// memory request whose length is not whole accesses or is more
    /// than [`MAX_MEMORY_BYTES`].
    pub fn encode(&self, packet: &mut [u8; PACKET]) -> Result<usize, ParseError> {
        let mut out = Writer { packet, length: 0 };
        match *self {
            Self::Transfer(TransferWords(words)) => {
                if words.is_empty() || words.len() / 2 > MAX_TRANSFER_WORDS {
                    return Err(ParseError::Length { code: TRANSFER, length: 1 + words.len() });
                }
                out.put(&[TRANSFER]).put(words);
            }
            Self::Pins => {
                out.put(&[PINS]);
            }
            Self::ResetHalt => {
                out.put(&[RESET_HALT]);
            }
            Self::Halt => {
                out.put(&[HALT]);
            }
            Self::Go => {
                out.put(&[GO]);
            }
            Self::SetClock { dsclk_hz } => {
                out.put(&[SET_CLOCK]).put(&dsclk_hz.to_le_bytes());
            }
            Self::ReadRegister(register) => {
                out.put(&[READ_REGISTER, code_byte(register.code())]);
            }
            Self::WriteRegister(register, value) => {
                out.put(&[WRITE_REGISTER, code_byte(register.code())]).put(&value.to_le_bytes());
            }
            Self::ReadSystemRegister(register) => {
                out.put(&[READ_SYSTEM_REGISTER, code_byte(register.code())]);
            }
            Self::WriteSystemRegister(register, value) => {
                out.put(&[WRITE_SYSTEM_REGISTER, code_byte(register.code())]).put(&value.to_le_bytes());
            }
            Self::ReadMemory { address: Address(address), size, length } => {
                let bytes = u8::try_from(length).ok().filter(|_| memory_length_ok(size, length));
                let bytes = bytes.ok_or(ParseError::Field(READ_MEMORY))?;
                out.put(&[READ_MEMORY]).put(&address.to_le_bytes()).put(&[code_byte(size.field()), bytes]);
            }
            Self::WriteMemory { address: Address(address), size, data } => {
                if !memory_length_ok(size, data.len()) {
                    return Err(ParseError::Field(WRITE_MEMORY));
                }
                out.put(&[WRITE_MEMORY])
                    .put(&address.to_le_bytes())
                    .put(&[code_byte(size.field())])
                    .put(data);
            }
            Self::Call(Address(address)) => {
                out.put(&[CALL]).put(&address.to_le_bytes());
            }
            Self::Hello => {
                out.put(&[HELLO]);
            }
        }
        Ok(out.length)
    }
}

/// Appends to a packet.
struct Writer<'p> {
    packet: &'p mut [u8; PACKET],
    length: usize,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) -> &mut Self {
        self.packet[self.length..self.length + bytes.len()].copy_from_slice(bytes);
        self.length += bytes.len();
        self
    }
}

/// `body` as an array, if it is exactly `N` bytes.
fn exact<const N: usize>(body: &[u8]) -> Option<[u8; N]> {
    body.try_into().ok()
}

const fn long(bytes: [u8; LONG]) -> u32 {
    u32::from_le_bytes(bytes)
}

/// A register or select code, or a size field, as its one byte: all are
/// under 16.
fn code_byte(code: u16) -> u8 {
    u8::try_from(code).expect("register and size codes fit a byte")
}

fn register_from(byte: u8) -> Option<Register> {
    Register::from_code(u16::from(byte))
}

fn system_register_from(byte: u8) -> Option<SystemRegister> {
    SystemRegister::from_code(u16::from(byte))
}

/// The size a byte names; `from_field` alone would read 4 as a byte.
fn size_from(byte: u8) -> Option<Size> {
    Size::from_field(u16::from(byte)).filter(|size| size.field() == u16::from(byte))
}

/// A memory request's length: whole accesses, at least one, at most
/// [`MAX_MEMORY_BYTES`].
fn memory_length_ok(size: Size, length: usize) -> bool {
    length > 0 && length <= MAX_MEMORY_BYTES && length.is_multiple_of(byte_count(size))
}

/// Writes a Transfer request for `words` into `packet`, returning its
/// length.
///
/// # Errors
/// No words, or more than one transfer takes.
pub fn encode_transfer(words: &[u16], packet: &mut [u8; PACKET]) -> Result<usize, ParseError> {
    if words.is_empty() || words.len() > MAX_TRANSFER_WORDS {
        return Err(ParseError::Length { code: TRANSFER, length: 1 + 2 * words.len() });
    }
    packet[0] = TRANSFER;
    for (word, pair) in words.iter().zip(packet[1..].as_chunks_mut::<2>().0) {
        *pair = word.to_le_bytes();
    }
    Ok(1 + 2 * words.len())
}

/// How a request went: a reply's first byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[repr(u8)]
pub enum Status {
    /// Done; the rest of the reply is the request's result.
    #[error("ok")]
    Ok = 0,
    /// The pod could not parse the request.
    #[error("bad request")]
    BadRequest = 1,
    /// No target VDD.
    #[error("no target power")]
    NoTarget = 2,
    /// FREEZE did not assert, or is not asserted for a request that
    /// needs background mode.
    #[error("target not in background mode")]
    NoFreeze = 3,
    /// A bus or address error on the target.
    #[error("target bus error")]
    BusError = 4,
    /// The target refused a command.
    #[error("illegal command")]
    Illegal = 5,
    /// The target stayed not ready.
    #[error("target not ready")]
    NotReady = 6,
    /// A response the command's sequence does not allow.
    #[error("unexpected response from target")]
    Unexpected = 7,
}

impl Status {
    const ALL: [Self; 8] = [
        Self::Ok,
        Self::BadRequest,
        Self::NoTarget,
        Self::NoFreeze,
        Self::BusError,
        Self::Illegal,
        Self::NotReady,
        Self::Unexpected,
    ];

    fn from_byte(byte: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|status| *status as u8 == byte)
    }
}

impl<L> From<session::Error<L>> for Status {
    fn from(error: session::Error<L>) -> Self {
        match error {
            session::Error::BusError => Self::BusError,
            session::Error::Illegal => Self::Illegal,
            session::Error::NotReady(_) => Self::NotReady,
            session::Error::Length { .. } => Self::BadRequest,
            session::Error::Link(_) | session::Error::Unexpected(_) => Self::Unexpected,
        }
    }
}

/// A reply the host cannot read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReplyError {
    /// The pod reported a failure.
    #[error(transparent)]
    Failed(Status),
    /// The reply is not one the request allows.
    #[error("malformed reply")]
    Malformed,
}

/// The target's pins, as the pod reads them through the translators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pins {
    /// FREEZE high: the CPU is in background mode.
    pub freeze: bool,
    /// RESET high: not in reset.
    pub reset: bool,
    /// DS high: no data strobe.
    pub ds: bool,
    /// BERR high: no bus error.
    pub berr: bool,
    /// Target VDD, millivolts.
    pub vdd_mv: u16,
    /// ATEMP for the CPU's current stay in background mode.
    pub atemp: Atemp,
}

/// ATEMP as the pod read it for the CPU's current stay in background
/// mode.  ATEMP holds the entry source only until the first command
/// after entry (CPU32RM 7.2.3), so the pod reads it before any other
/// command once FREEZE rises, and keeps it until the CPU leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Atemp {
    /// The CPU is not in background mode.
    None,
    /// Read first after entry: the entry source (CPU32RM Table 7-2).
    Fresh(u32),
    /// Read when the pod had not seen the entry (it started with the
    /// CPU already halted), so commands may have come first: not an
    /// entry source.
    Stale(u32),
    /// In background mode, but the read after entry failed: the
    /// failure was that request's status, and the pod does not retry
    /// it until the next entry.
    Unread,
}

impl Atemp {
    /// Its kind byte and value.
    const fn to_bytes(self) -> [u8; ATEMP_BYTES] {
        let (kind, value) = match self {
            Self::None => (ATEMP_NONE, 0),
            Self::Fresh(value) => (ATEMP_FRESH, value),
            Self::Stale(value) => (ATEMP_STALE, value),
            Self::Unread => (ATEMP_UNREAD, 0),
        };
        let [b0, b1, b2, b3] = value.to_le_bytes();
        [kind, b0, b1, b2, b3]
    }

    const fn from_bytes(bytes: [u8; ATEMP_BYTES]) -> Option<Self> {
        let [kind, b0, b1, b2, b3] = bytes;
        let value = u32::from_le_bytes([b0, b1, b2, b3]);
        match kind {
            ATEMP_NONE if value == 0 => Some(Self::None),
            ATEMP_FRESH => Some(Self::Fresh(value)),
            ATEMP_STALE => Some(Self::Stale(value)),
            ATEMP_UNREAD if value == 0 => Some(Self::Unread),
            _ => None,
        }
    }
}

/// Writes a reply that is only a status, returning its length.
pub fn encode_status(status: Status, packet: &mut [u8; PACKET]) -> usize {
    packet[0] = status as u8;
    STATUS_BYTES
}

/// Writes a Transfer's reply, returning its length.  `responses` is at
/// most [`MAX_TRANSFER_WORDS`] long.
pub fn encode_transfer_reply(responses: &[u32], packet: &mut [u8; PACKET]) -> usize {
    packet[0] = Status::Ok as u8;
    for (response, out) in responses.iter().zip(packet[STATUS_BYTES..].as_chunks_mut::<REPLY_WORD_BYTES>().0)
    {
        *out = response.to_le_bytes();
    }
    STATUS_BYTES + REPLY_WORD_BYTES * responses.len()
}

/// Writes a Pins reply, returning its length.
pub fn encode_pins(pins: &Pins, packet: &mut [u8; PACKET]) -> usize {
    let levels = [(pins.freeze, FREEZE), (pins.reset, RESET), (pins.ds, DS), (pins.berr, BERR)]
        .into_iter()
        .filter(|(high, _)| *high)
        .fold(0, |bits, (_, bit)| bits | bit);
    packet[0] = Status::Ok as u8;
    packet[1] = levels;
    packet[2..4].copy_from_slice(&pins.vdd_mv.to_le_bytes());
    packet[4..STATUS_BYTES + PINS_BYTES].copy_from_slice(&pins.atemp.to_bytes());
    STATUS_BYTES + PINS_BYTES
}

/// Writes a `Halt` or `ResetHalt` reply, returning its length.
pub fn encode_atemp(atemp: Atemp, packet: &mut [u8; PACKET]) -> usize {
    packet[0] = Status::Ok as u8;
    packet[STATUS_BYTES..STATUS_BYTES + ATEMP_BYTES].copy_from_slice(&atemp.to_bytes());
    STATUS_BYTES + ATEMP_BYTES
}

/// Writes a reply carrying one 32-bit value (a register, ATEMP, the
/// DSCLK rate set), returning its length.
pub fn encode_value(value: u32, packet: &mut [u8; PACKET]) -> usize {
    packet[0] = Status::Ok as u8;
    packet[1..=LONG].copy_from_slice(&value.to_le_bytes());
    STATUS_BYTES + LONG
}

/// Writes a memory read's reply, returning its length.  `data` is at
/// most [`MAX_MEMORY_BYTES`] long.
pub fn encode_data(data: &[u8], packet: &mut [u8; PACKET]) -> usize {
    packet[0] = Status::Ok as u8;
    packet[STATUS_BYTES..STATUS_BYTES + data.len()].copy_from_slice(data);
    STATUS_BYTES + data.len()
}

/// A reply's result, after its status.
///
/// # Errors
/// The pod reported a failure, or the reply is malformed.
pub fn parse_reply(packet: &[u8]) -> Result<&[u8], ReplyError> {
    let (&status, body) = packet.split_first().ok_or(ReplyError::Malformed)?;
    match Status::from_byte(status) {
        Some(Status::Ok) => Ok(body),
        Some(failed) => Err(ReplyError::Failed(failed)),
        None => Err(ReplyError::Malformed),
    }
}

/// A Transfer reply's 17-bit responses, from [`parse_reply`]'s result.
///
/// # Errors
/// A length that is not whole words.
pub fn parse_transfer_reply(body: &[u8]) -> Result<impl Iterator<Item = u32> + '_, ReplyError> {
    if !body.len().is_multiple_of(REPLY_WORD_BYTES) {
        return Err(ReplyError::Malformed);
    }
    Ok(body.as_chunks::<REPLY_WORD_BYTES>().0.iter().map(|b| u32::from_le_bytes(*b)))
}

/// A Pins reply, from [`parse_reply`]'s result.
///
/// # Errors
/// The wrong length.
pub fn parse_pins(body: &[u8]) -> Result<Pins, ReplyError> {
    let [levels, low, high, a0, a1, a2, a3, a4] = exact::<PINS_BYTES>(body).ok_or(ReplyError::Malformed)?;
    let freeze = levels & FREEZE != 0;
    let atemp = Atemp::from_bytes([a0, a1, a2, a3, a4]).ok_or(ReplyError::Malformed)?;
    if freeze == (atemp == Atemp::None) {
        return Err(ReplyError::Malformed);
    }
    Ok(Pins {
        freeze,
        reset: levels & RESET != 0,
        ds: levels & DS != 0,
        berr: levels & BERR != 0,
        vdd_mv: u16::from_le_bytes([low, high]),
        atemp,
    })
}

/// A `Halt` or `ResetHalt` reply's ATEMP, from [`parse_reply`]'s result:
/// never [`Atemp::None`].
///
/// # Errors
/// The wrong length or kind.
pub fn parse_atemp(body: &[u8]) -> Result<Atemp, ReplyError> {
    match exact::<ATEMP_BYTES>(body).and_then(Atemp::from_bytes) {
        Some(Atemp::None) | None => Err(ReplyError::Malformed),
        Some(atemp) => Ok(atemp),
    }
}

/// A one-value reply's value, from [`parse_reply`]'s result.
///
/// # Errors
/// The wrong length.
pub fn parse_value(body: &[u8]) -> Result<u32, ReplyError> {
    Ok(long(exact::<LONG>(body).ok_or(ReplyError::Malformed)?))
}

#[cfg(test)]
mod tests {
    use super::ATEMP_FRESH;
    use super::ATEMP_NONE;
    use super::ATEMP_UNREAD;
    use super::Address;
    use super::Atemp;
    use super::BERR;
    use super::DS;
    use super::FREEZE;
    use super::MAX_MEMORY_BYTES;
    use super::MAX_TRANSFER_WORDS;
    use super::PACKET;
    use super::ParseError;
    use super::Pins;
    use super::Register;
    use super::ReplyError;
    use super::Request;
    use super::Size;
    use super::Status;
    use super::SystemRegister;
    use super::byte_count;
    use super::encode_atemp;
    use super::encode_data;
    use super::encode_pins;
    use super::encode_status;
    use super::encode_transfer;
    use super::encode_transfer_reply;
    use super::encode_value;
    use super::parse_atemp;
    use super::parse_pins;
    use super::parse_reply;
    use super::parse_transfer_reply;
    use super::parse_value;

    fn round_trip(request: Request<'_>) {
        let mut packet = [0; PACKET];
        let length = request.encode(&mut packet).unwrap();
        assert_eq!(Request::parse(&packet[..length]), Ok(request));
    }

    #[test]
    fn requests_round_trip() {
        round_trip(Request::Pins);
        round_trip(Request::ResetHalt);
        round_trip(Request::Halt);
        round_trip(Request::Go);
        round_trip(Request::SetClock { dsclk_hz: 1_000_000 });
        round_trip(Request::ReadRegister(Register::A7));
        round_trip(Request::WriteRegister(Register::D3, 0xDEAD_BEEF));
        round_trip(Request::ReadSystemRegister(SystemRegister::Atemp));
        round_trip(Request::WriteSystemRegister(SystemRegister::Sfc, 5));
        round_trip(Request::ReadMemory { address: Address(0x00FF_FA00), size: Size::Word, length: 8 });
        round_trip(Request::WriteMemory {
            address: Address(0x0010_0000),
            size: Size::Long,
            data: &[1, 2, 3, 4],
        });
        round_trip(Request::Call(Address(0x0004_0000)));
        round_trip(Request::Hello);
    }

    #[test]
    fn memory_requests_at_the_limit_fit() {
        let data = [0xA5; MAX_MEMORY_BYTES];
        round_trip(Request::WriteMemory { address: Address(u32::MAX), size: Size::Long, data: &data });
        round_trip(Request::ReadMemory { address: Address(0), size: Size::Byte, length: MAX_MEMORY_BYTES });
        assert!(MAX_MEMORY_BYTES.is_multiple_of(byte_count(Size::Long)));
        const { assert!(1 + 4 + 1 + MAX_MEMORY_BYTES <= PACKET) };
    }

    #[test]
    fn memory_lengths_out_of_range() {
        let mut packet = [0; PACKET];
        for (size, length) in
            [(Size::Word, 3), (Size::Long, 6), (Size::Byte, 0), (Size::Byte, MAX_MEMORY_BYTES + 1)]
        {
            let request = Request::ReadMemory { address: Address(0), size, length };
            assert_eq!(request.encode(&mut packet), Err(ParseError::Field(0x0B)), "{size:?} {length}");
        }
        let request = Request::WriteMemory { address: Address(0), size: Size::Word, data: &[1, 2, 3] };
        assert_eq!(request.encode(&mut packet), Err(ParseError::Field(0x0C)));
        // Parsed: odd word length, the reserved size, and sizes past the
        // field's two bits.
        assert_eq!(Request::parse(&[0x0B, 0, 0, 0, 0, 1, 3]), Err(ParseError::Field(0x0B)));
        for size in [3, 4, 6, 0xFF] {
            assert_eq!(
                Request::parse(&[0x0B, 0, 0, 0, 0, size, 4]),
                Err(ParseError::Field(0x0B)),
                "size {size}"
            );
        }
    }

    #[test]
    fn register_codes_out_of_range() {
        assert_eq!(Request::parse(&[0x07, 16]), Err(ParseError::Field(0x07)));
        assert_eq!(Request::parse(&[0x09, 2]), Err(ParseError::Field(0x09)));
        assert_eq!(Request::parse(&[0x0A, 7, 0, 0, 0, 0]), Err(ParseError::Field(0x0A)));
    }

    #[test]
    fn transfer_round_trips() {
        let words = [0x2588, 0x0000, 0xFFFF];
        let mut packet = [0; PACKET];
        let length = encode_transfer(&words, &mut packet).unwrap();
        assert_eq!(&packet[..length], [0x01, 0x88, 0x25, 0, 0, 0xFF, 0xFF]);
        let Ok(Request::Transfer(parsed)) = Request::parse(&packet[..length]) else {
            panic!("not a transfer");
        };
        assert_eq!(parsed.iter().collect::<Vec<_>>(), words);
        assert_eq!(parsed.len(), 3);
        round_trip(Request::Transfer(parsed));
    }

    #[test]
    fn transfer_limits() {
        let mut packet = [0; PACKET];
        let most = [0; MAX_TRANSFER_WORDS];
        assert!(encode_transfer(&most, &mut packet).is_ok());
        let too_many = [0; MAX_TRANSFER_WORDS + 1];
        assert!(encode_transfer(&too_many, &mut packet).is_err());
        assert!(encode_transfer(&[], &mut packet).is_err());
        // A reply to the most words still fits a packet.
        const { assert!(4 * MAX_TRANSFER_WORDS < PACKET) };
    }

    #[test]
    fn bad_requests() {
        assert_eq!(Request::parse(&[]), Err(ParseError::Empty));
        assert_eq!(Request::parse(&[0x7F]), Err(ParseError::Unknown(0x7F)));
        assert_eq!(Request::parse(&[0x02, 0]), Err(ParseError::Length { code: 2, length: 2 }));
        assert_eq!(Request::parse(&[0x01]), Err(ParseError::Length { code: 1, length: 1 }));
        assert_eq!(Request::parse(&[0x01, 0, 0, 0]), Err(ParseError::Length { code: 1, length: 4 }));
        assert_eq!(Request::parse(&[0x06, 1, 2, 3]), Err(ParseError::Length { code: 6, length: 4 }));
        assert_eq!(Request::parse(&[0x0C, 0, 0, 0]), Err(ParseError::Length { code: 0x0C, length: 4 }));
        assert_eq!(Request::parse(&[0x0D, 0, 0]), Err(ParseError::Length { code: 0x0D, length: 3 }));
    }

    #[test]
    fn transfer_reply() {
        let mut packet = [0; PACKET];
        let length = encode_transfer_reply(&[0x1_0000, 0x0_1234], &mut packet);
        let body = parse_reply(&packet[..length]).unwrap();
        assert_eq!(parse_transfer_reply(body).unwrap().collect::<Vec<_>>(), [0x1_0000, 0x0_1234]);
        assert!(parse_transfer_reply(&[0, 0, 0]).is_err());
    }

    #[test]
    fn pins_reply() {
        let mut packet = [0; PACKET];
        for (freeze, atemp) in [(true, Atemp::Fresh(1)), (true, Atemp::Stale(0)), (false, Atemp::None)] {
            let pins = Pins { freeze, reset: true, ds: false, berr: true, vdd_mv: 5012, atemp };
            let length = encode_pins(&pins, &mut packet);
            assert_eq!(parse_pins(parse_reply(&packet[..length]).unwrap()), Ok(pins));
        }
        // Each level in its own bit.
        for (berr, levels) in [(true, DS | BERR), (false, DS)] {
            let pins = Pins { freeze: false, reset: false, ds: true, berr, vdd_mv: 0, atemp: Atemp::None };
            let length = encode_pins(&pins, &mut packet);
            assert_eq!(packet[1], levels);
            assert_eq!(parse_pins(parse_reply(&packet[..length]).unwrap()), Ok(pins));
        }
        assert_eq!(parse_pins(&[0, 0]), Err(ReplyError::Malformed));
        // ATEMP only in background mode, and always then.
        assert_eq!(parse_pins(&[0, 0x88, 0x13, ATEMP_FRESH, 0, 0, 0, 0]), Err(ReplyError::Malformed));
        assert_eq!(parse_pins(&[FREEZE, 0x88, 0x13, ATEMP_NONE, 0, 0, 0, 0]), Err(ReplyError::Malformed));
    }

    #[test]
    fn atemp_reply() {
        let mut packet = [0; PACKET];
        for atemp in [Atemp::Fresh(0x1234_FFFF), Atemp::Stale(0xA7E3_A7E3), Atemp::Unread] {
            let length = encode_atemp(atemp, &mut packet);
            assert_eq!(parse_atemp(parse_reply(&packet[..length]).unwrap()), Ok(atemp));
        }
        let length = encode_atemp(Atemp::None, &mut packet);
        assert_eq!(parse_atemp(parse_reply(&packet[..length]).unwrap()), Err(ReplyError::Malformed));
        assert_eq!(parse_atemp(&[ATEMP_UNREAD, 1, 0, 0, 0]), Err(ReplyError::Malformed));
        assert_eq!(parse_atemp(&[4, 0, 0, 0, 0]), Err(ReplyError::Malformed));
        assert_eq!(parse_atemp(&[ATEMP_FRESH, 0, 0, 0]), Err(ReplyError::Malformed));
    }

    #[test]
    fn value_and_data_replies() {
        let mut packet = [0; PACKET];
        let length = encode_value(0x0000_0001, &mut packet);
        assert_eq!(parse_value(parse_reply(&packet[..length]).unwrap()), Ok(1));
        assert_eq!(parse_value(&[0, 0]), Err(ReplyError::Malformed));
        let length = encode_data(&[9, 8, 7], &mut packet);
        assert_eq!(parse_reply(&packet[..length]), Ok(&[9, 8, 7][..]));
    }

    #[test]
    fn failures() {
        let mut packet = [0; PACKET];
        for status in Status::ALL.into_iter().skip(1) {
            let length = encode_status(status, &mut packet);
            assert_eq!(parse_reply(&packet[..length]), Err(ReplyError::Failed(status)));
        }
        assert_eq!(parse_reply(&[0xEE]), Err(ReplyError::Malformed));
        assert_eq!(parse_reply(&[]), Err(ReplyError::Malformed));
    }
}
