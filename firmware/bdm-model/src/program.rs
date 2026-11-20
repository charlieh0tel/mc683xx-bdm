//! The instructions the model runs once resumed, and the exceptions
//! they and an interrupt raise (CPU32RM 4, 6): NOP, MOVEQ, BRA.S, TRAP,
//! ILLEGAL and BGND, with T1 and T0 tracing.  Another instruction, a
//! fetch, stacking or vector read outside RAM, or a run past
//! [`RUN_LIMIT`] instructions is a program the model cannot follow: it
//! is left running, as a CPU running code no one watches.

use mc683xx_target::entry::Entry;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;

use crate::SR_S;
use crate::Target;

/// Instructions one run executes before the model stops following it.
const RUN_LIMIT: u32 = 1000;

/// SR's trace bits (CPU32RM 2.3.3): T1 traces every instruction, T0
/// changes of flow.
const SR_T1: u32 = 0x8000;
const SR_T0: u32 = 0x4000;
/// SR's interrupt mask, and where it sits.
const SR_MASK: u32 = 0x0700;
const SR_MASK_SHIFT: u32 = 8;
/// The condition codes MOVEQ sets (CPU32RM 4, MOVEQ): N and Z set by
/// the result, V and C cleared, X kept.
const CCR_N: u32 = 0x8;
const CCR_Z: u32 = 0x4;
const CCR_V: u32 = 0x2;
const CCR_C: u32 = 0x1;

/// The interrupt level the mask never blocks (CPU32RM 6.2.12).
const NONMASKABLE: u8 = 7;

/// Exception vectors (CPU32RM Table 6-1).
const ILLEGAL_VECTOR: u32 = 4;
const TRACE_VECTOR: u32 = 9;
const TRAP_VECTOR: u32 = 32;

/// Stack frame formats (CPU32RM 6.4): four words, and six with the
/// faulting or traced instruction's address.
const FORMAT_FOUR_WORD: u16 = 0x0;
const FORMAT_SIX_WORD: u16 = 0x2;
/// The format's place in the format/vector word.
const FORMAT_SHIFT: u32 = 12;

/// Bytes in a vector, a long.
const VECTOR_BYTES: u32 = 4;
/// Bytes in an instruction word.
const WORD_BYTES: u32 = 2;

/// Opcodes (CPU32RM 4).
const NOP: u16 = 0x4E71;
const BGND: u16 = 0x4AFA;
const ILLEGAL: u16 = 0x4AFC;
const TRAP: u16 = 0x4E40;
const TRAP_MASK: u16 = 0xFFF0;
const MOVEQ: u16 = 0x7000;
const MOVEQ_MASK: u16 = 0xF100;
const BRA_SHORT: u16 = 0x6000;
const BRA_MASK: u16 = 0xFF00;
/// BRA's 8-bit displacements that announce a 16- or 32-bit one.
const BRA_WORD: u8 = 0x00;
const BRA_LONG: u8 = 0xFF;
/// The register field of MOVEQ and the vector field of TRAP.
const REGISTER_SHIFT: u16 = 9;
const REGISTER_FIELD: u16 = 0x7;
const TRAP_FIELD: u16 = 0xF;

/// A program the model cannot follow further.
#[derive(Debug)]
struct Lost;

/// How an instruction or exception left the CPU.
enum Flow {
    /// At the next instruction.
    Next,
    /// Back in background mode.
    Background,
}

impl<const RAM: usize> Target<RAM> {
    /// Runs the program from RPC, as after GO or CALL, until a BGND
    /// enters background mode or the model loses it.
    pub(crate) fn run(&mut self) {
        if !self.running {
            return;
        }
        for _ in 0..RUN_LIMIT {
            match self.next() {
                Ok(Flow::Next) => {}
                Ok(Flow::Background) => {
                    self.enter_background(Entry::Background);
                    return;
                }
                Err(Lost) => return,
            }
        }
    }

    /// One instruction boundary: a pending interrupt the mask allows,
    /// or the instruction at the PC and its trace.
    fn next(&mut self) -> Result<Flow, Lost> {
        if let Some((level, vector)) = self.interrupt
            && (level == NONMASKABLE || u32::from(level) > (self.sr() & SR_MASK) >> SR_MASK_SHIFT)
        {
            self.interrupt = None;
            let pc = self.pc();
            self.exception(u32::from(vector), pc, None)?;
            let sr = (self.sr() & !SR_MASK) | (u32::from(level) << SR_MASK_SHIFT);
            self.set_system_register(SystemRegister::Sr, sr);
            return Ok(Flow::Next);
        }
        let start = self.pc();
        let tracing = self.sr() & (SR_T1 | SR_T0);
        let opcode = self.fetch(start)?;
        let next = start.wrapping_add(WORD_BYTES);
        let mut changed_flow = false;
        match opcode {
            NOP => self.set_pc(next),
            BGND => {
                // RPC after the BGND, PCC on it (CPU32RM 4, BGND;
                // 7.2.5.3); a trace pending on it is not taken
                self.set_pc(next);
                self.set_system_register(SystemRegister::Pcc, start);
                return Ok(Flow::Background);
            }
            ILLEGAL => {
                // not executed, so not traced (CPU32RM 6.2.10)
                self.exception(ILLEGAL_VECTOR, start, None)?;
                return Ok(Flow::Next);
            }
            _ if opcode & TRAP_MASK == TRAP => {
                self.exception(TRAP_VECTOR + u32::from(opcode & TRAP_FIELD), next, None)?;
                changed_flow = true;
            }
            _ if opcode & MOVEQ_MASK == MOVEQ => {
                let value = i32::from(opcode as u8 as i8) as u32;
                let register = (opcode >> REGISTER_SHIFT) & REGISTER_FIELD;
                self.registers[usize::from(register)] = value;
                let mut sr = self.sr() & !(CCR_N | CCR_Z | CCR_V | CCR_C);
                if value == 0 {
                    sr |= CCR_Z;
                }
                if (value as i32) < 0 {
                    sr |= CCR_N;
                }
                self.set_system_register(SystemRegister::Sr, sr);
                self.set_pc(next);
            }
            _ if opcode & BRA_MASK == BRA_SHORT && !matches!(opcode as u8, BRA_WORD | BRA_LONG) => {
                let displacement = i32::from(opcode as u8 as i8) as u32;
                self.set_pc(next.wrapping_add(displacement));
                changed_flow = true;
            }
            _ => return Err(Lost),
        }
        if tracing & SR_T1 != 0 || (tracing & SR_T0 != 0 && changed_flow) {
            let pc = self.pc();
            self.exception(TRACE_VECTOR, pc, Some(start))?;
        }
        Ok(Flow::Next)
    }

    /// Exception processing (CPU32RM 6.2): supervisor mode, tracing
    /// off, the frame stacked with `pc` (six words with `address`, four
    /// without), and the vector fetched through VBR.
    fn exception(&mut self, vector: u32, pc: u32, address: Option<u32>) -> Result<(), Lost> {
        let sr = self.sr();
        self.set_system_register(SystemRegister::Sr, (sr | SR_S) & !(SR_T1 | SR_T0));
        let format = if address.is_some() { FORMAT_SIX_WORD } else { FORMAT_FOUR_WORD };
        let format_vector = (u32::from(format) << FORMAT_SHIFT) | (vector * VECTOR_BYTES);
        let mut sp = self.register(Register::A7);
        if let Some(address) = address {
            sp = self.push(sp, Size::Long, address)?;
        }
        sp = self.push(sp, Size::Word, format_vector)?;
        sp = self.push(sp, Size::Long, pc)?;
        sp = self.push(sp, Size::Word, sr)?;
        self.set_register(Register::A7, sp);
        let table = self.system_register(SystemRegister::Vbr);
        let handler = self.load(Size::Long, table.wrapping_add(vector * VECTOR_BYTES)).map_err(|()| Lost)?;
        self.set_pc(handler);
        Ok(())
    }

    /// Pushes `value` below `sp`; the new SP.
    fn push(&mut self, sp: u32, size: Size, value: u32) -> Result<u32, Lost> {
        let sp = sp.wrapping_sub(size.bytes());
        self.store(size, sp, value).map_err(|()| Lost)?;
        Ok(sp)
    }

    fn fetch(&self, pc: u32) -> Result<u16, Lost> {
        let word = self.load(Size::Word, pc).map_err(|()| Lost)?;
        Ok(word as u16)
    }

    /// The PC, which background mode shows as RPC.
    const fn pc(&self) -> u32 {
        self.system_register(SystemRegister::Rpc)
    }

    const fn set_pc(&mut self, pc: u32) {
        self.set_system_register(SystemRegister::Rpc, pc);
    }
}
