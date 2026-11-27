//! A CPU32 in background mode, modeled at the serial word: what it
//! shifts out on DSO for each word shifted in on DSI, from CPU32RM
//! 7.2.7 and the command sequences of 7.2.8.  Registers and a small RAM
//! stand in for the rest of the chip.
//!
//! It implements `mc683xx_bdm_proto::session::Link`, so the pod's command
//! sequences run against it on the host, and it records what a real
//! CPU would not forgive: clocks while it runs, words lost while it is
//! busy, DUMP or FILL with no READ or WRITE before, and GO or CALL
//! without the pod's DSI release.
//!
//! Opcodes are decoded here from CPU32RM's bit fields, not with
//! `mc683xx_bdm_proto::bdm`, so that an encoding mistake cannot hide on both
//! sides.
//!
//! Once resumed it runs the few instructions `program` models, from
//! its RAM, until a BGND puts it back in background mode or it meets
//! one it does not model: enough to step by T1 trace (CPU32RM 6.2.10)
//! through a stub, as a pod does.

#![no_std]

pub mod board;
mod program;

use core::convert::Infallible;
use core::ops::Range;

use mc683xx_bdm_proto::bdm::byte_count;
use mc683xx_bdm_proto::session::Link;
use mc683xx_target::Address;
use mc683xx_target::Entry;
use mc683xx_target::Register;
use mc683xx_target::Size;
use mc683xx_target::SystemRegister;

/// Words the CPU shifts out (CPU32RM Table 7-3).
const NOT_READY: u32 = 0x1_0000;
const BUS_ERROR: u32 = 0x1_0001;
const ILLEGAL: u32 = 0x1_FFFF;
const COMPLETE: u32 = 0x0_FFFF;

/// NOP, which the host clocks while it waits.
const NOP: u16 = 0x0000;

/// Opcode fields (CPU32RM 7.2.8.1): the register commands keep 4 bits
/// of register, the memory commands 2 bits of size at bit 6.
const REGISTER_MASK: u16 = 0xFFF0;
const SIZED_MASK: u16 = 0xFF3F;
const SIZE_SHIFT: u16 = 6;
const LOW_NIBBLE: u16 = 0x000F;

/// SR's supervisor bit (CPU32RM 2.3.3): A7 is SSP while it is set,
/// USP while it is clear.
const SR_S: u32 = 0x2000;

/// Bytes in a long, the stack step for CALL's return address.
const LONG_BYTES: u32 = 4;
/// The low word of a long.
const LOW_WORD: u32 = 0xFFFF;

/// The ATEMP a command leaves behind once it has used it for scratch
/// (CPU32RM 7.2.3: most commands do).
const ATEMP_SCRATCH: u32 = 0xA7E3_A7E3;

/// Something a real CPU32 would not have taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    /// A word clocked while the CPU runs: each clock is a BKPT pulse.
    ClockedWhileRunning(u16),
    /// A word other than NOP sent while the CPU ignores its input (busy,
    /// or shifting out the first word of a long result): lost.
    WordIgnored(u16),
    /// GO or CALL sent without the DSI release, so DSI still drives pin
    /// 8 when it becomes IFETCH.
    ExitNotSignaled(u16),
    /// The DSI release on a word that does not leave background mode, so
    /// the CPU loses every word after.
    ExitSignaledEarly(u16),
    /// DUMP with no READ or DUMP before it: undefined (CPU32RM 7.2.8.10).
    DumpWithoutRead,
    /// FILL with no WRITE or FILL before it: undefined (CPU32RM 7.2.8.11).
    FillWithoutWrite,
    /// BERR asserted while the CPU runs: its program takes a bus error.
    BerrWhileRunning,
}

/// A decoded command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    ReadRegister(Register),
    WriteRegister(Register),
    ReadSystemRegister(SystemRegister),
    WriteSystemRegister(SystemRegister),
    Read(Size),
    Write(Size),
    Dump(Size),
    Fill(Size),
    Go,
    Call,
    ResetPeripherals,
    Nop,
}

impl Op {
    fn decode(word: u16) -> Option<Self> {
        let low = word & LOW_NIBBLE;
        let size = Size::from_field((word >> SIZE_SHIFT) & Size::FIELD_MASK);
        match (word & REGISTER_MASK, word & SIZED_MASK) {
            (0x2180, _) => Register::from_code(low).map(Self::ReadRegister),
            (0x2080, _) => Register::from_code(low).map(Self::WriteRegister),
            (0x2580, _) => SystemRegister::from_code(low).map(Self::ReadSystemRegister),
            (0x2480, _) => SystemRegister::from_code(low).map(Self::WriteSystemRegister),
            (_, 0x1900) => size.map(Self::Read),
            (_, 0x1800) => size.map(Self::Write),
            (_, 0x1D00) => size.map(Self::Dump),
            (_, 0x1C00) => size.map(Self::Fill),
            _ => match word {
                0x0C00 => Some(Self::Go),
                0x0800 => Some(Self::Call),
                0x0400 => Some(Self::ResetPeripherals),
                NOP => Some(Self::Nop),
                _ => None,
            },
        }
    }

    /// Operand words after the opcode (CPU32RM 7.2.8.4-7.2.8.15).
    const fn operands(self) -> usize {
        match self {
            Self::WriteRegister(_) | Self::WriteSystemRegister(_) | Self::Read(_) | Self::Call => 2,
            Self::Write(Size::Long) => 4,
            Self::Write(_) => 3,
            Self::Fill(Size::Long) => 2,
            Self::Fill(_) => 1,
            _ => 0,
        }
    }
}

/// What the CPU does with the word shifted in alongside an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    /// Ignored ("XXX" in CPU32RM's sequences) while the CPU works or
    /// shifts out a long's first word: anything but a NOP is lost.
    Ignore,
    /// Ignored with an error going out: the host is abandoning the
    /// command, so whatever it sent is moot.
    Discard,
    /// The next command.
    Command,
}

/// The block command last done, for DUMP and FILL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    None,
    Read,
    Write,
}

/// A CPU32 with `RAM` bytes of memory at `ram_base`; every other
/// address ends in a bus error.
#[derive(Debug)]
pub struct Target<const RAM: usize> {
    /// D0-D7, then A0-A7, in register-code order.  A7 is the active
    /// stack pointer, SSP or USP as SR's S bit says.
    registers: [u32; 16],
    /// By select code; codes with no register stay 0.  Of SSP and
    /// USP, the one A7 is not keeps its value here: USP while S is
    /// set, SSP while it is clear.
    system: [u32; 16],
    /// An interrupt requested and not yet taken: its level and vector.
    interrupt: Option<(u8, u8)>,
    ram: [u8; RAM],
    ram_base: u32,
    /// Exchanges each memory access answers "not ready" before its
    /// result: a slow bus.
    pub memory_busy: u32,
    /// Running, not in background mode.
    running: bool,
    /// "Not ready" exchanges still to come.
    busy: u32,
    /// Outputs queued after `busy`, at most a long result's two.
    queue: [(u32, Input); 2],
    queued: usize,
    /// The command whose operands are coming in, and those so far.
    collecting: Option<(Op, [u16; 4], usize)>,
    block: Block,
    /// The next DUMP's or FILL's address.
    block_address: u32,
    first_violation: Option<Violation>,
    violations: usize,
    /// RST commands seen.
    peripheral_resets: usize,
    /// The last command was a CALL whose stacking failed: the CPU stayed
    /// in background mode after the pod's DSI release.
    call_failed: bool,
    /// The link's clock as a `Link`: [`WORD_US`] a word.
    clock_us: u32,
}

/// Microseconds a word takes on the model's own link: 17 bits at 1 MHz.
const WORD_US: u32 = 17;

impl<const RAM: usize> Target<RAM> {
    /// A running CPU with zeroed registers and memory.
    #[must_use]
    pub const fn new(ram_base: Address) -> Self {
        Self {
            registers: [0; 16],
            system: [0; 16],
            interrupt: None,
            ram: [0; RAM],
            ram_base: ram_base.0,
            memory_busy: 0,
            running: true,
            busy: 0,
            queue: [(0, Input::Ignore); 2],
            queued: 0,
            collecting: None,
            block: Block::None,
            block_address: 0,
            first_violation: None,
            violations: 0,
            peripheral_resets: 0,
            call_failed: false,
            clock_us: 0,
        }
    }

    /// Stops the CPU in background mode, as FREEZE asserts.
    pub const fn enter_background(&mut self, entry: Entry) {
        self.running = false;
        self.system[SystemRegister::Atemp.code() as usize] = entry.atemp();
        self.busy = 0;
        self.queued = 0;
        self.collecting = None;
        self.block = Block::None;
    }

    /// Comes out of a reset running, as with BKPT high at RESET's rise.
    pub const fn reset_running(&mut self) {
        self.running = true;
        self.busy = 0;
        self.queued = 0;
        self.collecting = None;
        self.block = Block::None;
    }

    /// BERR asserted: an access still answering "not ready" (`memory_busy`)
    /// ends in a bus error.  The model has done the access already, so a
    /// write's value stays; a real one would not be written.
    pub fn bus_error(&mut self) {
        if self.running {
            self.violate(Violation::BerrWhileRunning);
            return;
        }
        if self.busy > 0 && self.queued > 0 {
            self.busy = 0;
            self.queue[0] = (BUS_ERROR, Input::Discard);
            self.queued = 1;
        }
    }

    /// FREEZE: in background mode.
    #[must_use]
    pub const fn frozen(&self) -> bool {
        !self.running
    }

    /// The first violation, and how many there were.
    #[must_use]
    pub const fn violations(&self) -> (Option<Violation>, usize) {
        (self.first_violation, self.violations)
    }

    /// RST commands taken.
    #[must_use]
    pub const fn peripheral_resets(&self) -> usize {
        self.peripheral_resets
    }

    /// A data or address register.
    #[must_use]
    pub const fn register(&self, register: Register) -> u32 {
        self.registers[register.code() as usize]
    }

    /// Sets a data or address register.
    pub const fn set_register(&mut self, register: Register, value: u32) {
        self.registers[register.code() as usize] = value;
    }

    /// A system register.  SSP or USP, whichever is active, is A7.
    #[must_use]
    pub const fn system_register(&self, register: SystemRegister) -> u32 {
        if self.is_active_stack(register) {
            return self.register(Register::A7);
        }
        self.system[register.code() as usize]
    }

    /// Sets a system register.  SSP or USP, whichever is active, is A7;
    /// an SR that changes S swaps A7 with the other stack pointer.
    pub const fn set_system_register(&mut self, register: SystemRegister, value: u32) {
        if self.is_active_stack(register) {
            self.set_register(Register::A7, value);
            return;
        }
        if matches!(register, SystemRegister::Sr) && (value ^ self.sr()) & SR_S != 0 {
            let (from, to) = if value & SR_S != 0 {
                (SystemRegister::Usp, SystemRegister::Ssp)
            } else {
                (SystemRegister::Ssp, SystemRegister::Usp)
            };
            self.system[from.code() as usize] = self.register(Register::A7);
            self.set_register(Register::A7, self.system[to.code() as usize]);
        }
        self.system[register.code() as usize] = value;
    }

    /// SR.
    const fn sr(&self) -> u32 {
        self.system[SystemRegister::Sr.code() as usize]
    }

    /// Whether `register` is the stack pointer A7 is now.
    const fn is_active_stack(&self, register: SystemRegister) -> bool {
        match register {
            SystemRegister::Ssp => self.sr() & SR_S != 0,
            SystemRegister::Usp => self.sr() & SR_S == 0,
            _ => false,
        }
    }

    /// Requests an interrupt at `level` (1-7) with `vector`, taken at
    /// the next instruction boundary SR's mask allows, as a device's
    /// vectored interrupt (CPU32RM 6.2.12).  Level 7 is not masked.
    pub const fn request_interrupt(&mut self, level: u8, vector: u8) {
        self.interrupt = Some((level, vector));
    }

    /// The model's RAM.
    pub const fn ram_mut(&mut self) -> &mut [u8; RAM] {
        &mut self.ram
    }

    /// One serial word: returns what goes out on DSO while `input` comes
    /// in on DSI.  `exit` is the pod's DSI release after the word.
    pub fn exchange(&mut self, input: u16, exit: bool) -> u32 {
        if self.running {
            self.violate(Violation::ClockedWhileRunning(input));
            return NOT_READY;
        }
        let (output, take) = self.next_output();
        let leaves = match take {
            Some(Input::Ignore) => {
                if input != NOP {
                    self.violate(Violation::WordIgnored(input));
                }
                false
            }
            Some(Input::Discard) => false,
            Some(Input::Command) => self.command(input),
            None => self.operand(input),
        };
        if leaves && !exit {
            self.violate(Violation::ExitNotSignaled(input));
        }
        if exit && !self.exit_allowed(leaves) {
            self.violate(Violation::ExitSignaledEarly(input));
        }
        output
    }

    /// The output for this exchange and what the input is for; `None`
    /// for an operand.
    fn next_output(&mut self) -> (u32, Option<Input>) {
        if self.busy > 0 {
            self.busy -= 1;
            return (NOT_READY, Some(Input::Ignore));
        }
        if self.queued > 0 {
            let item = self.queue[0];
            self.queue[0] = self.queue[1];
            self.queued -= 1;
            return (item.0, Some(item.1));
        }
        if self.collecting.is_some() {
            return (NOT_READY, None);
        }
        (NOT_READY, Some(Input::Command))
    }

    /// The release may come with the word that leaves, or with CALL's
    /// last word when stacking failed and the CPU stayed.
    const fn exit_allowed(&self, leaves: bool) -> bool {
        leaves || self.call_failed
    }

    fn command(&mut self, word: u16) -> bool {
        self.call_failed = false;
        let Some(op) = Op::decode(word) else {
            self.reply(&[(ILLEGAL, Input::Discard)]);
            return false;
        };
        if op.operands() == 0 {
            return self.execute(op, &[]);
        }
        self.collecting = Some((op, [0; 4], 0));
        false
    }

    fn operand(&mut self, word: u16) -> bool {
        let Some((op, mut words, have)) = self.collecting else {
            return false;
        };
        words[have] = word;
        if have + 1 < op.operands() {
            self.collecting = Some((op, words, have + 1));
            return false;
        }
        self.collecting = None;
        self.execute(op, &words[..op.operands()])
    }

    /// Carries out a complete command; true if the CPU leaves background
    /// mode.
    fn execute(&mut self, op: Op, operands: &[u16]) -> bool {
        let long = |n: usize| u32::from(operands[n]) << 16 | u32::from(operands[n + 1]);
        let sized = |size: Size, n: usize| match size {
            Size::Long => long(n),
            Size::Byte | Size::Word => u32::from(operands[n]),
        };
        let leaves = match op {
            Op::ReadRegister(register) => {
                self.reply_long(self.register(register));
                false
            }
            Op::WriteRegister(register) => {
                self.set_register(register, long(0));
                self.reply(&[(COMPLETE, Input::Command)]);
                false
            }
            Op::ReadSystemRegister(register) => {
                self.reply_long(self.system_register(register));
                false
            }
            Op::WriteSystemRegister(register) => {
                self.set_system_register(register, long(0));
                self.reply(&[(COMPLETE, Input::Command)]);
                false
            }
            Op::Read(size) => {
                self.block = Block::Read;
                self.read(size, long(0));
                false
            }
            Op::Dump(size) => {
                if self.block != Block::Read {
                    self.violate(Violation::DumpWithoutRead);
                }
                self.read(size, self.block_address);
                false
            }
            Op::Write(size) => {
                self.block = Block::Write;
                self.write(size, long(0), sized(size, 2));
                false
            }
            Op::Fill(size) => {
                if self.block != Block::Write {
                    self.violate(Violation::FillWithoutWrite);
                }
                self.write(size, self.block_address, sized(size, 0));
                false
            }
            Op::Go => {
                self.running = true;
                true
            }
            Op::Call => self.call(long(0)),
            Op::ResetPeripherals => {
                self.peripheral_resets += 1;
                self.reply(&[(COMPLETE, Input::Command)]);
                false
            }
            Op::Nop => {
                self.reply(&[(COMPLETE, Input::Command)]);
                false
            }
        };
        // NOP pads between DUMPs or FILLs "without corrupting the address
        // pointer" (CPU32RM 7.2.8.10, 7.2.8.11); anything else ends a block.
        if !matches!(op, Op::Read(_) | Op::Dump(_) | Op::Write(_) | Op::Fill(_) | Op::Nop) {
            self.block = Block::None;
        }
        if !matches!(op, Op::Nop | Op::ReadSystemRegister(_)) {
            self.set_system_register(SystemRegister::Atemp, ATEMP_SCRATCH);
        }
        leaves
    }

    /// CALL: RPC stacked at SP - 4, then run from `address`; a bus error
    /// while stacking keeps the CPU in background mode (CPU32RM
    /// 7.2.8.13).
    fn call(&mut self, address: u32) -> bool {
        let sp = self.register(Register::A7).wrapping_sub(LONG_BYTES);
        let rpc = self.system_register(SystemRegister::Rpc);
        if self.store(Size::Long, sp, rpc).is_err() {
            self.call_failed = true;
            self.reply(&[(BUS_ERROR, Input::Discard)]);
            return false;
        }
        self.set_register(Register::A7, sp);
        self.set_system_register(SystemRegister::Rpc, address);
        self.running = true;
        true
    }

    fn read(&mut self, size: Size, address: u32) {
        self.busy = self.memory_busy;
        match self.load(size, address) {
            Ok(value) => {
                self.block_address = address.wrapping_add(size.bytes());
                match size {
                    Size::Long => self.reply_long(value),
                    Size::Byte | Size::Word => self.reply(&[(value, Input::Command)]),
                }
            }
            Err(()) => self.reply(&[(BUS_ERROR, Input::Discard)]),
        }
    }

    fn write(&mut self, size: Size, address: u32, value: u32) {
        self.busy = self.memory_busy;
        match self.store(size, address, value) {
            Ok(()) => {
                self.block_address = address.wrapping_add(size.bytes());
                self.reply(&[(COMPLETE, Input::Command)]);
            }
            Err(()) => self.reply(&[(BUS_ERROR, Input::Discard)]),
        }
    }

    /// The RAM bytes an access covers; a bus error outside RAM, an
    /// address error for an odd word or long.
    fn span(&self, size: Size, address: u32) -> Result<Range<usize>, ()> {
        if size != Size::Byte && !address.is_multiple_of(2) {
            return Err(());
        }
        let offset = address.checked_sub(self.ram_base).ok_or(())?;
        let start = usize::try_from(offset).map_err(|_| ())?;
        let end = start.checked_add(byte_count(size)).ok_or(())?;
        if end > RAM {
            return Err(());
        }
        Ok(start..end)
    }

    fn load(&self, size: Size, address: u32) -> Result<u32, ()> {
        let span = self.span(size, address)?;
        Ok(self.ram[span].iter().fold(0, |value, byte| value << 8 | u32::from(*byte)))
    }

    fn store(&mut self, size: Size, address: u32, value: u32) -> Result<(), ()> {
        let span = self.span(size, address)?;
        let bytes = value.to_be_bytes();
        let width = span.len();
        self.ram[span].copy_from_slice(&bytes[4 - width..]);
        Ok(())
    }

    /// A long result: the input with the first word is ignored, the one
    /// with the second is the next command (CPU32RM 7.2.8.4).
    fn reply_long(&mut self, value: u32) {
        self.reply(&[(value >> 16, Input::Ignore), (value & LOW_WORD, Input::Command)]);
    }

    fn reply(&mut self, items: &[(u32, Input)]) {
        self.queue[..items.len()].copy_from_slice(items);
        self.queued = items.len();
    }

    fn violate(&mut self, violation: Violation) {
        self.first_violation.get_or_insert(violation);
        self.violations += 1;
    }
}

impl<const RAM: usize> Link for Target<RAM> {
    type Error = Infallible;

    async fn transfer(&mut self, word: u16) -> Result<u32, Infallible> {
        self.clock_us = self.clock_us.wrapping_add(WORD_US);
        Ok(self.exchange(word, false))
    }

    async fn transfer_exit(&mut self, word: u16) -> Result<u32, Infallible> {
        self.clock_us = self.clock_us.wrapping_add(WORD_US);
        Ok(self.exchange(word, true))
    }

    fn clock_us(&self) -> u32 {
        self.clock_us
    }

    fn freeze_high(&self) -> bool {
        self.frozen()
    }
}
