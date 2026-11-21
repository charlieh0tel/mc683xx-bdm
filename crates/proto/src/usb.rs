//! Host-to-pod messages: one request per bulk OUT packet, one reply per
//! bulk IN packet, little-endian.  A request is its tag, then its code
//! and body; a reply is the tag echoed, FREEZE's edge count and the
//! status, then a body that depends on the request.  Hello alone is
//! untagged, the same in every protocol version.

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

/// Bytes in the chip's unique ID: the RP2040's flash unique ID, the
/// RP2350's OTP chip ID (pico-sdk's `pico_unique_board_id`).
pub const UNIQUE_ID_BYTES: usize = 8;
/// Characters in the pod's USB serial number: its unique ID in hex.
pub const SERIAL_NUMBER_CHARS: usize = 2 * UNIQUE_ID_BYTES;

/// The pod's USB serial number, written into `text`: `id` in uppercase
/// hex, first byte first, as pico-sdk's
/// `pico_get_unique_board_id_string` and picotool show it.
pub fn serial_number<'a>(id: &[u8; UNIQUE_ID_BYTES], text: &'a mut [u8; SERIAL_NUMBER_CHARS]) -> &'a str {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    for ([high, low], byte) in text.as_chunks_mut::<2>().0.iter_mut().zip(id) {
        *high = DIGITS[usize::from(byte >> 4)];
        *low = DIGITS[usize::from(byte & 0x0F)];
    }
    core::str::from_utf8(text).unwrap_or_default()
}

/// This protocol's version, in the Hello reply: raised on any change
/// to a request or reply (docs/usb-protocol.md).
pub const PROTOCOL_VERSION: u32 = 10;

/// Bytes a request's or reply's tag takes.
const TAG_BYTES: usize = 1;
/// A reply's header: its tag, FREEZE's edge count and its status.
const REPLY_HEADER_BYTES: usize = TAG_BYTES + 2;
/// Bytes per 17-bit word in a Transfer reply.
const REPLY_WORD_BYTES: usize = 4;
/// Words in one Transfer: its reply, a header and 4 bytes a word, fills
/// at most a packet.
pub const MAX_TRANSFER_WORDS: usize = (PACKET - REPLY_HEADER_BYTES) / REPLY_WORD_BYTES;
/// Bytes in one memory read or write: a write's tag, code, address,
/// size and data fit a packet, and it is a whole number of longs.
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
/// Hello's code, which is its whole request (`HELLO_REQUEST`).
const HELLO: u8 = 0x0E;
const RESET_RUN: u8 = 0x0F;
const RESYNC: u8 = 0x10;
const VERSION: u8 = 0x11;
const HOLD: u8 = 0x12;
const BERR_PULSE: u8 = 0x13;

/// Pin level bits in a Pins reply: set for high.
const FREEZE: u8 = 1 << 0;
const RESET: u8 = 1 << 1;
const DS: u8 = 1 << 2;
const BERR: u8 = 1 << 3;

/// Bytes in a 32-bit field.
const LONG: usize = 4;

/// Hello, the one untagged request: the same in every protocol version,
/// so a host and pod of different versions still tell each other theirs.
pub const HELLO_REQUEST: [u8; 1] = [HELLO];
/// Bytes in Hello's reply: status Ok and the version, untagged.
const HELLO_REPLY_BYTES: usize = 1 + LONG;

/// The kind byte of an ATEMP field (`Pins`, `Halt`, `ResetHalt` replies).
const ATEMP_NONE: u8 = 0;
const ATEMP_FRESH: u8 = 1;
const ATEMP_STALE: u8 = 2;
const ATEMP_UNREAD: u8 = 3;
/// An ATEMP field: its kind byte, then the value.
const ATEMP_BYTES: usize = 1 + LONG;
/// A Pins reply's body: levels, VDD, ATEMP.
const PINS_BYTES: usize = 3 + ATEMP_BYTES;

/// What the host asks of the pod, Hello aside ([`HELLO_REQUEST`]).
/// Everything from `ReadRegister` on needs the target in background
/// mode, and so do Go and Call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request<'a> {
    /// Raw serial words, each answered with the 17 bits shifted in: for
    /// bring-up, and for commands the pod does not sequence itself.
    Transfer(TransferWords<'a>),
    /// The target's pin levels and VDD.
    Pins,
    /// Reset the target into background mode; the reply is ATEMP.
    ResetHalt,
    /// Reset the target with BKPT high: it runs, background mode off.
    ResetRun,
    /// Bring a halted target's serial link back into frame; the reply
    /// is ATEMP, stale.
    Resync,
    /// The firmware's version, ASCII: crate version and git commit.
    Version,
    /// Drive BKPT low and leave it, clocking nothing: the CPU halts and
    /// comes out of any reset meanwhile in background mode.
    Hold,
    /// Pulse BERR to end a bus cycle a halted CPU's BDM access hung in;
    /// the reply says whether one ended.
    BerrPulse,
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

/// A tagged packet's tag and what follows it; `None` for an empty one.
#[must_use]
pub fn split_tag(packet: &[u8]) -> Option<(u8, &[u8])> {
    packet.split_first().map(|(&tag, rest)| (tag, rest))
}

impl<'a> Request<'a> {
    /// Reads a request from what follows a bulk OUT packet's tag
    /// ([`split_tag`]): its code and body.
    ///
    /// # Errors
    /// The message is not a request.
    pub fn parse(message: &'a [u8]) -> Result<Self, ParseError> {
        let (&code, body) = message.split_first().ok_or(ParseError::Empty)?;
        let length = ParseError::Length { code, length: message.len() };
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
            RESET_RUN => empty(Self::ResetRun),
            RESYNC => empty(Self::Resync),
            VERSION => empty(Self::Version),
            HOLD => empty(Self::Hold),
            BERR_PULSE => empty(Self::BerrPulse),
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
            other => Err(ParseError::Unknown(other)),
        }
    }

    /// Writes the request, tagged `tag`, into `packet`, returning its
    /// length.
    ///
    /// # Errors
    /// A Transfer of no words or more than [`MAX_TRANSFER_WORDS`], or a
    /// memory request whose length is not whole accesses or is more
    /// than [`MAX_MEMORY_BYTES`].
    pub fn encode(&self, tag: u8, packet: &mut [u8; PACKET]) -> Result<usize, ParseError> {
        let mut out = Writer { packet, length: 0 };
        out.put(&[tag]);
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
            Self::ResetRun => {
                out.put(&[RESET_RUN]);
            }
            Self::Resync => {
                out.put(&[RESYNC]);
            }
            Self::Version => {
                out.put(&[VERSION]);
            }
            Self::Hold => {
                out.put(&[HOLD]);
            }
            Self::BerrPulse => {
                out.put(&[BERR_PULSE]);
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

/// The size a byte names.
fn size_from(byte: u8) -> Option<Size> {
    Size::from_field(u16::from(byte))
}

/// A memory request's length: whole accesses, at least one, at most
/// [`MAX_MEMORY_BYTES`].
fn memory_length_ok(size: Size, length: usize) -> bool {
    length > 0 && length <= MAX_MEMORY_BYTES && length.is_multiple_of(byte_count(size))
}

/// Writes a Transfer request for `words`, tagged `tag`, into `packet`,
/// returning its length.
///
/// # Errors
/// No words, or more than one transfer takes.
pub fn encode_transfer(tag: u8, words: &[u16], packet: &mut [u8; PACKET]) -> Result<usize, ParseError> {
    if words.is_empty() || words.len() > MAX_TRANSFER_WORDS {
        return Err(ParseError::Length { code: TRANSFER, length: 1 + 2 * words.len() });
    }
    let mut out = Writer { packet, length: 0 };
    out.put(&[tag, TRANSFER]);
    for word in words {
        out.put(&word.to_le_bytes());
    }
    Ok(out.length)
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
    /// Halt or Hold refused: the pod's own last reset was `ResetRun`, so
    /// BKPT would be a breakpoint exception in the running program.
    #[error("background mode is off since the pod's reset to run")]
    BdmDisabled = 8,
    /// RESET stayed low after the pod's pulse: the target holds it.
    #[error("the target holds RESET low")]
    ResetHeld = 9,
}

impl Status {
    const ALL: [Self; 10] = [
        Self::Ok,
        Self::BadRequest,
        Self::NoTarget,
        Self::NoFreeze,
        Self::BusError,
        Self::Illegal,
        Self::NotReady,
        Self::Unexpected,
        Self::BdmDisabled,
        Self::ResetHeld,
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
            session::Error::NoFreeze => Self::NoFreeze,
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

/// What every reply but Hello's starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplyHeader {
    /// The request's tag, echoed: 0 for a request too short to carry
    /// one, which a host never sends.
    pub tag: u8,
    /// FREEZE's edge count as the reply is written, its low 8 bits: a
    /// count that moved between two replies means the CPU left
    /// background mode in between, perhaps to enter it again.
    pub freeze_edges: u8,
}

/// Writes a reply's header and `status`.
fn reply(header: ReplyHeader, status: Status, packet: &mut [u8; PACKET]) -> Writer<'_> {
    let mut out = Writer { packet, length: 0 };
    out.put(&[header.tag, header.freeze_edges, status as u8]);
    out
}

/// Writes a reply that is only a status, returning its length.
pub fn encode_status(header: ReplyHeader, status: Status, packet: &mut [u8; PACKET]) -> usize {
    reply(header, status, packet).length
}

/// Writes a Transfer's reply, returning its length.  `responses` is at
/// most [`MAX_TRANSFER_WORDS`] long.
pub fn encode_transfer_reply(header: ReplyHeader, responses: &[u32], packet: &mut [u8; PACKET]) -> usize {
    let mut out = reply(header, Status::Ok, packet);
    for response in responses {
        out.put(&response.to_le_bytes());
    }
    out.length
}

/// Writes a Pins reply, returning its length.
pub fn encode_pins(header: ReplyHeader, pins: &Pins, packet: &mut [u8; PACKET]) -> usize {
    let levels = [(pins.freeze, FREEZE), (pins.reset, RESET), (pins.ds, DS), (pins.berr, BERR)]
        .into_iter()
        .filter(|(high, _)| *high)
        .fold(0, |bits, (_, bit)| bits | bit);
    let mut out = reply(header, Status::Ok, packet);
    out.put(&[levels]).put(&pins.vdd_mv.to_le_bytes()).put(&pins.atemp.to_bytes());
    out.length
}

/// Writes a `Halt`, `ResetHalt` or `Resync` reply, returning its length.
pub fn encode_atemp(header: ReplyHeader, atemp: Atemp, packet: &mut [u8; PACKET]) -> usize {
    let mut out = reply(header, Status::Ok, packet);
    out.put(&atemp.to_bytes());
    out.length
}

/// Writes a reply carrying one 32-bit value (a register, the DSCLK rate
/// set), returning its length.
pub fn encode_value(header: ReplyHeader, value: u32, packet: &mut [u8; PACKET]) -> usize {
    let mut out = reply(header, Status::Ok, packet);
    out.put(&value.to_le_bytes());
    out.length
}

/// Writes a memory read's reply, returning its length.  `data` is at
/// most [`MAX_MEMORY_BYTES`] long.
pub fn encode_data(header: ReplyHeader, data: &[u8], packet: &mut [u8; PACKET]) -> usize {
    let mut out = reply(header, Status::Ok, packet);
    out.put(data);
    out.length
}

/// Writes Hello's reply, untagged: status Ok and [`PROTOCOL_VERSION`].
pub fn encode_hello_reply(packet: &mut [u8; PACKET]) -> usize {
    let mut out = Writer { packet, length: 0 };
    out.put(&[Status::Ok as u8]).put(&PROTOCOL_VERSION.to_le_bytes());
    out.length
}

/// The version in a Hello reply; `None` for any other packet, a
/// tagged reply included (its first byte, the tag, is never 0 from a
/// host's request).
#[must_use]
pub fn parse_hello_reply(packet: &[u8]) -> Option<u32> {
    let [status, version @ ..] = exact::<HELLO_REPLY_BYTES>(packet)?;
    (status == Status::Ok as u8).then(|| long(version))
}

/// A tagged reply's header, and what follows it, for [`parse_reply`].
///
/// # Errors
/// Too short for a header.
pub fn parse_reply_header(packet: &[u8]) -> Result<(ReplyHeader, &[u8]), ReplyError> {
    match packet {
        [tag, freeze_edges, rest @ ..] if !rest.is_empty() => {
            Ok((ReplyHeader { tag: *tag, freeze_edges: *freeze_edges }, rest))
        }
        _ => Err(ReplyError::Malformed),
    }
}

/// A reply's result, from what follows its header
/// ([`parse_reply_header`]): its status, then the body.
///
/// # Errors
/// The pod reported a failure, or the reply is malformed.
pub fn parse_reply(message: &[u8]) -> Result<&[u8], ReplyError> {
    let (&status, body) = message.split_first().ok_or(ReplyError::Malformed)?;

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
    use super::HELLO_REQUEST;
    use super::MAX_MEMORY_BYTES;
    use super::MAX_TRANSFER_WORDS;
    use super::PACKET;
    use super::PROTOCOL_VERSION;
    use super::ParseError;
    use super::Pins;
    use super::Register;
    use super::ReplyError;
    use super::ReplyHeader;
    use super::Request;
    use super::SERIAL_NUMBER_CHARS;
    use super::Size;
    use super::Status;
    use super::SystemRegister;
    use super::UNIQUE_ID_BYTES;
    use super::byte_count;
    use super::encode_atemp;
    use super::encode_data;
    use super::encode_hello_reply;
    use super::encode_pins;
    use super::encode_status;
    use super::encode_transfer;
    use super::encode_transfer_reply;
    use super::encode_value;
    use super::parse_atemp;
    use super::parse_hello_reply;
    use super::parse_pins;
    use super::parse_reply;
    use super::parse_reply_header;
    use super::parse_transfer_reply;
    use super::parse_value;
    use super::serial_number;
    use super::split_tag;

    /// A reply header with no meaning beyond the test's.
    const HEADER: ReplyHeader = ReplyHeader { tag: 0x5A, freeze_edges: 3 };

    fn round_trip(request: Request<'_>) {
        let mut packet = [0; PACKET];
        let length = request.encode(0xC3, &mut packet).unwrap();
        let (tag, message) = split_tag(&packet[..length]).unwrap();
        assert_eq!(tag, 0xC3);
        assert_eq!(Request::parse(message), Ok(request));
    }

    /// A reply's result, its header checked against [`HEADER`].
    fn result(packet: &[u8]) -> Result<&[u8], ReplyError> {
        let (header, message) = parse_reply_header(packet).unwrap();
        assert_eq!(header, HEADER);
        parse_reply(message)
    }

    #[test]
    fn hello_is_untagged_and_the_same_in_every_version() {
        assert_eq!(HELLO_REQUEST, [0x0E]);
        let mut packet = [0; PACKET];
        let length = encode_hello_reply(&mut packet);
        assert_eq!(packet[..length], [0, PROTOCOL_VERSION as u8, 0, 0, 0]);
        assert_eq!(parse_hello_reply(&packet[..length]), Some(PROTOCOL_VERSION));
        // A tagged reply is never taken for it: its tag is not 0.
        let length = encode_value(ReplyHeader { tag: 1, freeze_edges: 0 }, 9, &mut packet);
        assert_eq!(parse_hello_reply(&packet[..length]), None);
        assert_eq!(parse_hello_reply(&[0, 9, 0, 0]), None);
        assert_eq!(parse_hello_reply(&[1]), None);
    }

    #[test]
    fn replies_carry_the_tag_and_freeze_edges() {
        let mut packet = [0; PACKET];
        let length = encode_status(HEADER, Status::NoFreeze, &mut packet);
        assert_eq!(packet[..length], [0x5A, 3, Status::NoFreeze as u8]);
        assert_eq!(parse_reply_header(&[0x5A, 3]), Err(ReplyError::Malformed));
        assert_eq!(split_tag(&[]), None);
    }

    #[test]
    fn requests_round_trip() {
        round_trip(Request::Pins);
        round_trip(Request::ResetHalt);
        round_trip(Request::ResetRun);
        round_trip(Request::Resync);
        round_trip(Request::Version);
        round_trip(Request::Hold);
        round_trip(Request::BerrPulse);
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
    }

    #[test]
    fn memory_requests_at_the_limit_fit() {
        let data = [0xA5; MAX_MEMORY_BYTES];
        round_trip(Request::WriteMemory { address: Address(u32::MAX), size: Size::Long, data: &data });
        round_trip(Request::ReadMemory { address: Address(0), size: Size::Byte, length: MAX_MEMORY_BYTES });
        assert!(MAX_MEMORY_BYTES.is_multiple_of(byte_count(Size::Long)));
        const { assert!(2 + 4 + 1 + MAX_MEMORY_BYTES <= PACKET) };
        const { assert!(3 + MAX_MEMORY_BYTES <= PACKET) };
    }

    #[test]
    fn memory_lengths_out_of_range() {
        let mut packet = [0; PACKET];
        for (size, length) in
            [(Size::Word, 3), (Size::Long, 6), (Size::Byte, 0), (Size::Byte, MAX_MEMORY_BYTES + 1)]
        {
            let request = Request::ReadMemory { address: Address(0), size, length };
            assert_eq!(request.encode(1, &mut packet), Err(ParseError::Field(0x0B)), "{size:?} {length}");
        }
        let request = Request::WriteMemory { address: Address(0), size: Size::Word, data: &[1, 2, 3] };
        assert_eq!(request.encode(1, &mut packet), Err(ParseError::Field(0x0C)));
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
        let length = encode_transfer(7, &words, &mut packet).unwrap();
        assert_eq!(&packet[..length], [7, 0x01, 0x88, 0x25, 0, 0, 0xFF, 0xFF]);
        let Ok(Request::Transfer(parsed)) = Request::parse(&packet[1..length]) else {
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
        assert!(encode_transfer(1, &most, &mut packet).is_ok());
        let too_many = [0; MAX_TRANSFER_WORDS + 1];
        assert!(encode_transfer(1, &too_many, &mut packet).is_err());
        assert!(encode_transfer(1, &[], &mut packet).is_err());
        // A reply to the most words still fits a packet.
        const { assert!(3 + 4 * MAX_TRANSFER_WORDS <= PACKET) };
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
        // Hello is no tagged request.
        assert_eq!(Request::parse(&[0x0E]), Err(ParseError::Unknown(0x0E)));
    }

    #[test]
    fn transfer_reply() {
        let mut packet = [0; PACKET];
        let length = encode_transfer_reply(HEADER, &[0x1_0000, 0x0_1234], &mut packet);
        let body = result(&packet[..length]).unwrap();
        assert_eq!(parse_transfer_reply(body).unwrap().collect::<Vec<_>>(), [0x1_0000, 0x0_1234]);
        assert!(parse_transfer_reply(&[0, 0, 0]).is_err());
    }

    #[test]
    fn pins_reply() {
        let mut packet = [0; PACKET];
        for (freeze, atemp) in [(true, Atemp::Fresh(1)), (true, Atemp::Stale(0)), (false, Atemp::None)] {
            let pins = Pins { freeze, reset: true, ds: false, berr: true, vdd_mv: 5012, atemp };
            let length = encode_pins(HEADER, &pins, &mut packet);
            assert_eq!(parse_pins(result(&packet[..length]).unwrap()), Ok(pins));
        }
        // Each level in its own bit.
        for (berr, levels) in [(true, DS | BERR), (false, DS)] {
            let pins = Pins { freeze: false, reset: false, ds: true, berr, vdd_mv: 0, atemp: Atemp::None };
            let length = encode_pins(HEADER, &pins, &mut packet);
            assert_eq!(packet[3], levels);
            assert_eq!(parse_pins(result(&packet[..length]).unwrap()), Ok(pins));
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
            let length = encode_atemp(HEADER, atemp, &mut packet);
            assert_eq!(parse_atemp(result(&packet[..length]).unwrap()), Ok(atemp));
        }
        let length = encode_atemp(HEADER, Atemp::None, &mut packet);
        assert_eq!(parse_atemp(result(&packet[..length]).unwrap()), Err(ReplyError::Malformed));
        assert_eq!(parse_atemp(&[ATEMP_UNREAD, 1, 0, 0, 0]), Err(ReplyError::Malformed));
        assert_eq!(parse_atemp(&[4, 0, 0, 0, 0]), Err(ReplyError::Malformed));
        assert_eq!(parse_atemp(&[ATEMP_FRESH, 0, 0, 0]), Err(ReplyError::Malformed));
    }

    #[test]
    fn value_and_data_replies() {
        let mut packet = [0; PACKET];
        let length = encode_value(HEADER, 0x0000_0001, &mut packet);
        assert_eq!(parse_value(result(&packet[..length]).unwrap()), Ok(1));
        assert_eq!(parse_value(&[0, 0]), Err(ReplyError::Malformed));
        let length = encode_data(HEADER, &[9, 8, 7], &mut packet);
        assert_eq!(result(&packet[..length]), Ok(&[9, 8, 7][..]));
    }

    #[test]
    fn failures() {
        let mut packet = [0; PACKET];
        for status in Status::ALL.into_iter().skip(1) {
            let length = encode_status(HEADER, status, &mut packet);
            assert_eq!(result(&packet[..length]), Err(ReplyError::Failed(status)));
        }
        assert_eq!(Status::ALL.map(|status| status as u8), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);

        assert_eq!(parse_reply(&[0xEE]), Err(ReplyError::Malformed));
        assert_eq!(parse_reply(&[]), Err(ReplyError::Malformed));
    }

    #[test]
    fn serial_numbers_are_uppercase_hex_first_byte_first() {
        let mut text = [0; SERIAL_NUMBER_CHARS];
        assert_eq!(
            serial_number(&[0xE6, 0x61, 0x38, 0x52, 0x83, 0x3B, 0x2A, 0x2F], &mut text),
            "E6613852833B2A2F"
        );
        assert_eq!(serial_number(&[0x00; UNIQUE_ID_BYTES], &mut text), "0000000000000000");
        assert_eq!(serial_number(&[0xFF; UNIQUE_ID_BYTES], &mut text), "FFFFFFFFFFFFFFFF");
    }
}
