//! BDM serial words: commands encoded, responses decoded (CPU32RM 7.2.7,
//! 7.2.8).  A word is 17 bits: bit 16 is status (from the CPU) or
//! control (from the host, always clear), bits 15-0 data.

use mc683xx_target::address::Address;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;

/// Opcodes, CPU32RM 7.2.8.4-7.2.8.15.  RSREG's figure there prints
/// 0x2480, the same as WSREG's: a misprint (PLAN.md, Protocol summary).
const RDREG: u16 = 0x2180;
const WDREG: u16 = 0x2080;
const RSREG: u16 = 0x2580;
const WSREG: u16 = 0x2480;
const READ: u16 = 0x1900;
const WRITE: u16 = 0x1800;
const DUMP: u16 = 0x1D00;
const FILL: u16 = 0x1C00;
const GO: u16 = 0x0C00;
const CALL: u16 = 0x0800;
const RST: u16 = 0x0400;
/// NOP, also what the host clocks out to collect a result.
pub const NOP: u16 = 0x0000;

/// Where the size field sits in an opcode (CPU32RM 7.2.8.1).
const SIZE_SHIFT: u32 = 6;

/// Bit 16 of a word from the CPU: set for a status message.
const STATUS: u32 = 1 << 16;
/// The 16 data bits of a word.
const DATA: u32 = 0xFFFF;
/// A byte operand's bits, carried in a word's low byte (CPU32RM 7.2.8.9).
const BYTE: u32 = 0xFF;
/// The data of a "command complete" (status clear) and of an "illegal
/// command" (status set) message (CPU32RM Table 7-3).
pub const COMPLETE: u16 = 0xFFFF;
const ILLEGAL: u16 = 0xFFFF;
/// The data of the status messages "not ready" and "bus error".
const NOT_READY: u16 = 0x0000;
const BUS_ERROR: u16 = 0x0001;

/// The most words a command takes: WRITE long's opcode, two address
/// words and two data words.
const MAX_WORDS: usize = 5;

/// A BDM command and its operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// RDREG/RAREG.
    ReadRegister(Register),
    /// WDREG/WAREG.
    WriteRegister(Register, u32),
    /// RSREG.
    ReadSystemRegister(SystemRegister),
    /// WSREG.
    WriteSystemRegister(SystemRegister, u32),
    /// READ: one access at an address, through SFC.
    Read(Size, Address),
    /// WRITE: one access at an address, through DFC.
    Write(Size, Address, u32),
    /// DUMP: the next access after a READ or DUMP.
    Dump(Size),
    /// FILL: the next access after a WRITE or FILL.
    Fill(Size, u32),
    /// GO: resume at RPC, leaving background mode.
    Go,
    /// CALL: stack RPC and run from an address, leaving background mode.
    Call(Address),
    /// RST: assert RESET for 512 clocks; the CPU itself is not reset.
    ResetPeripherals,
    /// NOP.
    Nop,
}

/// What follows a command's words (CPU32RM 7.2.8's command sequences).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// A result of one word (byte or word reads) or two (long).
    Data {
        /// 1 or 2.
        words: usize,
    },
    /// "Command complete" once the command is done.
    Complete,
    /// Nothing: the CPU has left background mode, and DSCLK is BKPT
    /// again, so any further clock would request a breakpoint.
    Exit,
}

/// A command's words, in the order they go out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Words {
    /// The words; only the first `len` are used.
    words: [u16; MAX_WORDS],
    len: usize,
}

impl Words {
    /// The words to send.
    #[must_use]
    pub fn as_slice(&self) -> &[u16] {
        &self.words[..self.len]
    }

    fn new(head: &[u16]) -> Self {
        let mut words = [0; MAX_WORDS];
        words[..head.len()].copy_from_slice(head);
        Self { words, len: head.len() }
    }

    fn with_long(self, value: u32) -> Self {
        self.with(&split(value))
    }

    fn with_sized(self, size: Size, value: u32) -> Self {
        match size {
            Size::Byte => self.with(&[(value & BYTE) as u16]),
            Size::Word => self.with(&[(value & DATA) as u16]),
            Size::Long => self.with_long(value),
        }
    }

    fn with(self, tail: &[u16]) -> Self {
        let mut words = self.words;
        words[self.len..self.len + tail.len()].copy_from_slice(tail);
        Self { words, len: self.len + tail.len() }
    }
}

/// A long as two words, most significant first, as every 32-bit operand
/// and result goes.
const fn split(value: u32) -> [u16; 2] {
    [(value >> 16) as u16, value as u16]
}

/// An opcode with its size field.
const fn sized(opcode: u16, size: Size) -> u16 {
    opcode | (size.field() << SIZE_SHIFT)
}

/// Bytes in an access of `size`, as a length.
#[must_use]
pub const fn byte_count(size: Size) -> usize {
    // 1, 2 or 4: no target's usize loses it.
    size.bytes() as usize
}

impl Command {
    /// The words that carry this command.
    #[must_use]
    pub fn encode(&self) -> Words {
        match *self {
            Self::ReadRegister(register) => Words::new(&[RDREG | register.code()]),
            Self::WriteRegister(register, value) => Words::new(&[WDREG | register.code()]).with_long(value),
            Self::ReadSystemRegister(register) => Words::new(&[RSREG | register.code()]),
            Self::WriteSystemRegister(register, value) => {
                Words::new(&[WSREG | register.code()]).with_long(value)
            }
            Self::Read(size, Address(address)) => Words::new(&[sized(READ, size)]).with_long(address),
            Self::Write(size, Address(address), value) => {
                Words::new(&[sized(WRITE, size)]).with_long(address).with_sized(size, value)
            }
            Self::Dump(size) => Words::new(&[sized(DUMP, size)]),
            Self::Fill(size, value) => Words::new(&[sized(FILL, size)]).with_sized(size, value),
            Self::Go => Words::new(&[GO]),
            Self::Call(Address(address)) => Words::new(&[CALL]).with_long(address),
            Self::ResetPeripherals => Words::new(&[RST]),
            Self::Nop => Words::new(&[NOP]),
        }
    }

    /// What the CPU sends back once the command's words are in.
    #[must_use]
    pub const fn reply(&self) -> Reply {
        match self {
            Self::ReadRegister(_) | Self::ReadSystemRegister(_) => Reply::Data { words: 2 },
            Self::Read(size, _) | Self::Dump(size) => Reply::Data { words: result_words(*size) },
            Self::WriteRegister(..)
            | Self::WriteSystemRegister(..)
            | Self::Write(..)
            | Self::Fill(..)
            | Self::ResetPeripherals
            | Self::Nop => Reply::Complete,
            Self::Go | Self::Call(_) => Reply::Exit,
        }
    }
}

/// Words in a memory result: bytes come back in a word's low byte
/// (CPU32RM 7.2.8.8).
const fn result_words(size: Size) -> usize {
    match size {
        Size::Byte | Size::Word => 1,
        Size::Long => 2,
    }
}

/// A word from the CPU (CPU32RM Table 7-3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    /// Status clear: a result word, or "command complete" ([`COMPLETE`]);
    /// which one depends on the command.
    Data(u16),
    /// Not ready; clock a NOP and look again.
    NotReady,
    /// The bus cycle ended in a bus or address error; data invalid.
    BusError,
    /// The previous word was not a command the CPU knows.
    Illegal,
    /// Status set with data the manual does not define.
    Unknown(u16),
}

impl Response {
    /// Decodes the 17 bits shifted in from DSO.
    #[must_use]
    pub const fn decode(word: u32) -> Self {
        let data = (word & DATA) as u16;
        if word & STATUS == 0 {
            return Self::Data(data);
        }
        match data {
            NOT_READY => Self::NotReady,
            BUS_ERROR => Self::BusError,
            ILLEGAL => Self::Illegal,
            other => Self::Unknown(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Address;
    use super::Command;
    use super::Register;
    use super::Reply;
    use super::Response;
    use super::Size;
    use super::SystemRegister;

    fn words(command: Command) -> Vec<u16> {
        command.encode().as_slice().to_vec()
    }

    #[test]
    fn register_commands() {
        assert_eq!(words(Command::ReadRegister(Register::D0)), [0x2180]);
        assert_eq!(words(Command::ReadRegister(Register::A7)), [0x218F]);
        assert_eq!(words(Command::WriteRegister(Register::A0, 0x1234_5678)), [0x2088, 0x1234, 0x5678]);
        assert_eq!(words(Command::ReadSystemRegister(SystemRegister::Atemp)), [0x2588]);
        assert_eq!(words(Command::WriteSystemRegister(SystemRegister::Sfc, 5)), [0x248E, 0x0000, 0x0005]);
    }

    #[test]
    fn memory_commands_carry_size_address_and_data() {
        let address = Address(0x00FF_FA00);
        assert_eq!(words(Command::Read(Size::Byte, address)), [0x1900, 0x00FF, 0xFA00]);
        assert_eq!(words(Command::Read(Size::Word, address)), [0x1940, 0x00FF, 0xFA00]);
        assert_eq!(words(Command::Read(Size::Long, address)), [0x1980, 0x00FF, 0xFA00]);
        assert_eq!(words(Command::Write(Size::Byte, address, 0xABCD)), [0x1800, 0x00FF, 0xFA00, 0x00CD]);
        assert_eq!(words(Command::Write(Size::Word, address, 0x1_ABCD)), [0x1840, 0x00FF, 0xFA00, 0xABCD]);
        assert_eq!(
            words(Command::Write(Size::Long, address, 0xDEAD_BEEF)),
            [0x1880, 0x00FF, 0xFA00, 0xDEAD, 0xBEEF]
        );
        assert_eq!(words(Command::Dump(Size::Word)), [0x1D40]);
        assert_eq!(words(Command::Fill(Size::Long, 0x0102_0304)), [0x1C80, 0x0102, 0x0304]);
        assert_eq!(words(Command::Fill(Size::Byte, 0x1FF)), [0x1C00, 0x00FF]);
    }

    #[test]
    fn control_commands() {
        assert_eq!(words(Command::Go), [0x0C00]);
        assert_eq!(words(Command::Call(Address(0x0004_0000))), [0x0800, 0x0004, 0x0000]);
        assert_eq!(words(Command::ResetPeripherals), [0x0400]);
        assert_eq!(words(Command::Nop), [0x0000]);
    }

    #[test]
    fn replies() {
        assert_eq!(Command::ReadRegister(Register::D0).reply(), Reply::Data { words: 2 });
        assert_eq!(Command::Read(Size::Byte, Address(0)).reply(), Reply::Data { words: 1 });
        assert_eq!(Command::Dump(Size::Long).reply(), Reply::Data { words: 2 });
        assert_eq!(Command::Fill(Size::Word, 0).reply(), Reply::Complete);
        assert_eq!(Command::Nop.reply(), Reply::Complete);
        assert_eq!(Command::Go.reply(), Reply::Exit);
        assert_eq!(Command::Call(Address(0)).reply(), Reply::Exit);
    }

    #[test]
    fn responses() {
        assert_eq!(Response::decode(0x0_1234), Response::Data(0x1234));
        assert_eq!(Response::decode(0x0_FFFF), Response::Data(0xFFFF));
        assert_eq!(Response::decode(0x1_0000), Response::NotReady);
        assert_eq!(Response::decode(0x1_0001), Response::BusError);
        assert_eq!(Response::decode(0x1_FFFF), Response::Illegal);
        assert_eq!(Response::decode(0x1_0002), Response::Unknown(2));
    }
}
