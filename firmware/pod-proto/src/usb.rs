//! Host-to-pod messages: one request per bulk OUT packet, one reply per
//! bulk IN packet, little-endian.  A request starts with its code, a
//! reply with its status; what follows depends on the request.

use crate::session;

/// A full-speed bulk packet.
pub const PACKET: usize = 64;

/// Bytes a reply's status takes.
const STATUS_BYTES: usize = 1;
/// Bytes per 17-bit word in a Transfer reply.
const REPLY_WORD_BYTES: usize = 4;
/// Words in one Transfer: its reply, a status and 4 bytes a word,
/// fills at most a packet.
pub const MAX_TRANSFER_WORDS: usize = (PACKET - STATUS_BYTES) / REPLY_WORD_BYTES;

/// Request codes.
const TRANSFER: u8 = 0x01;
const PINS: u8 = 0x02;
const RESET_HALT: u8 = 0x03;
const HALT: u8 = 0x04;
const GO: u8 = 0x05;
const SET_CLOCK: u8 = 0x06;

/// Pin level bits in a Pins reply: set for high.
const FREEZE: u8 = 1 << 0;
const RESET: u8 = 1 << 1;
const DS: u8 = 1 << 2;

/// What the host asks of the pod.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request<'a> {
    /// Raw serial words, each answered with the 17 bits shifted in: for
    /// bring-up, and for commands the pod does not sequence itself.
    Transfer(TransferWords<'a>),
    /// The target's pin levels and VDD.
    Pins,
    /// Reset the target into background mode.
    ResetHalt,
    /// Stop the running target in background mode.
    Halt,
    /// GO: resume at RPC.
    Go,
    /// Set DSCLK's rate; the reply gives the rate set.
    SetClock {
        /// The rate asked for.
        dsclk_hz: u32,
    },
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

/// A request the pod cannot take.
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
}

/// A Transfer of more than [`MAX_TRANSFER_WORDS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0} words; a transfer takes at most {MAX_TRANSFER_WORDS}")]
pub struct TooManyWords(pub usize);

impl<'a> Request<'a> {
    /// Reads a request from a bulk OUT packet.
    ///
    /// # Errors
    /// The packet is not a request.
    pub fn parse(packet: &'a [u8]) -> Result<Self, ParseError> {
        let (&code, body) = packet.split_first().ok_or(ParseError::Empty)?;
        let length = ParseError::Length { code, length: packet.len() };
        let no_body = |request| if body.is_empty() { Ok(request) } else { Err(length) };
        match code {
            TRANSFER => {
                if body.is_empty() || !body.len().is_multiple_of(2) || body.len() / 2 > MAX_TRANSFER_WORDS {
                    return Err(length);
                }
                Ok(Self::Transfer(TransferWords(body)))
            }
            PINS => no_body(Self::Pins),
            RESET_HALT => no_body(Self::ResetHalt),
            HALT => no_body(Self::Halt),
            GO => no_body(Self::Go),
            SET_CLOCK => {
                let bytes: [u8; 4] = body.try_into().map_err(|_| length)?;
                Ok(Self::SetClock { dsclk_hz: u32::from_le_bytes(bytes) })
            }
            other => Err(ParseError::Unknown(other)),
        }
    }

    /// Writes the request into `packet`, returning its length.
    pub fn encode(&self, packet: &mut [u8; PACKET]) -> usize {
        match *self {
            Self::Transfer(TransferWords(words)) => {
                packet[0] = TRANSFER;
                packet[1..=words.len()].copy_from_slice(words);
                1 + words.len()
            }
            Self::Pins => code_only(packet, PINS),
            Self::ResetHalt => code_only(packet, RESET_HALT),
            Self::Halt => code_only(packet, HALT),
            Self::Go => code_only(packet, GO),
            Self::SetClock { dsclk_hz } => {
                packet[0] = SET_CLOCK;
                packet[1..5].copy_from_slice(&dsclk_hz.to_le_bytes());
                5
            }
        }
    }
}

fn code_only(packet: &mut [u8; PACKET], code: u8) -> usize {
    packet[0] = code;
    1
}

/// Writes a Transfer request for `words` into `packet`, returning its
/// length.
///
/// # Errors
/// More words than one transfer takes.
pub fn encode_transfer(words: &[u16], packet: &mut [u8; PACKET]) -> Result<usize, TooManyWords> {
    if words.is_empty() || words.len() > MAX_TRANSFER_WORDS {
        return Err(TooManyWords(words.len()));
    }
    packet[0] = TRANSFER;
    for (word, pair) in words.iter().zip(packet[1..].as_chunks_mut::<2>().0) {
        pair.copy_from_slice(&word.to_le_bytes());
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
    /// FREEZE did not assert.
    #[error("target did not enter background mode")]
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
    /// Target VDD, millivolts.
    pub vdd_mv: u16,
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
        out.copy_from_slice(&response.to_le_bytes());
    }
    STATUS_BYTES + REPLY_WORD_BYTES * responses.len()
}

/// Writes a Pins reply, returning its length.
pub fn encode_pins(pins: &Pins, packet: &mut [u8; PACKET]) -> usize {
    let levels = [(pins.freeze, FREEZE), (pins.reset, RESET), (pins.ds, DS)]
        .into_iter()
        .filter(|(high, _)| *high)
        .fold(0, |bits, (_, bit)| bits | bit);
    packet[0] = Status::Ok as u8;
    packet[1] = levels;
    packet[2..4].copy_from_slice(&pins.vdd_mv.to_le_bytes());
    4
}

/// Writes a `SetClock` reply, returning its length.
pub fn encode_clock(dsclk_hz: u32, packet: &mut [u8; PACKET]) -> usize {
    packet[0] = Status::Ok as u8;
    packet[1..5].copy_from_slice(&dsclk_hz.to_le_bytes());
    5
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
    let [levels, low, high] = body.try_into().map_err(|_| ReplyError::Malformed)?;
    Ok(Pins {
        freeze: levels & FREEZE != 0,
        reset: levels & RESET != 0,
        ds: levels & DS != 0,
        vdd_mv: u16::from_le_bytes([low, high]),
    })
}

/// A `SetClock` reply's rate, from [`parse_reply`]'s result.
///
/// # Errors
/// The wrong length.
pub fn parse_clock(body: &[u8]) -> Result<u32, ReplyError> {
    let bytes: [u8; 4] = body.try_into().map_err(|_| ReplyError::Malformed)?;
    Ok(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::MAX_TRANSFER_WORDS;
    use super::PACKET;
    use super::ParseError;
    use super::Pins;
    use super::ReplyError;
    use super::Request;
    use super::Status;
    use super::TooManyWords;
    use super::encode_clock;
    use super::encode_pins;
    use super::encode_status;
    use super::encode_transfer;
    use super::encode_transfer_reply;
    use super::parse_clock;
    use super::parse_pins;
    use super::parse_reply;
    use super::parse_transfer_reply;

    fn round_trip(request: Request<'_>) {
        let mut packet = [0; PACKET];
        let length = request.encode(&mut packet);
        assert_eq!(Request::parse(&packet[..length]), Ok(request));
    }

    #[test]
    fn requests_round_trip() {
        round_trip(Request::Pins);
        round_trip(Request::ResetHalt);
        round_trip(Request::Halt);
        round_trip(Request::Go);
        round_trip(Request::SetClock { dsclk_hz: 1_000_000 });
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
        assert_eq!(encode_transfer(&too_many, &mut packet), Err(TooManyWords(MAX_TRANSFER_WORDS + 1)));
        assert_eq!(encode_transfer(&[], &mut packet), Err(TooManyWords(0)));
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
        let pins = Pins { freeze: true, reset: true, ds: false, vdd_mv: 5012 };
        let mut packet = [0; PACKET];
        let length = encode_pins(&pins, &mut packet);
        assert_eq!(parse_pins(parse_reply(&packet[..length]).unwrap()), Ok(pins));
        assert_eq!(parse_pins(&[0, 0]), Err(ReplyError::Malformed));
    }

    #[test]
    fn clock_reply() {
        let mut packet = [0; PACKET];
        let length = encode_clock(999_999, &mut packet);
        assert_eq!(parse_clock(parse_reply(&packet[..length]).unwrap()), Ok(999_999));
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
