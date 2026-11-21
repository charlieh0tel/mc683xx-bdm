//! A step by T1 trace through an exception table of the pod's own
//! (PLAN.md, Host, gdb): the table's layout in scratch RAM, and how
//! the frames stacked on the way to its BGND stub are unwound into the
//! state `Cpu32Target::step` promises, the CPU before the first
//! instruction of the last handler a program's own table would have
//! entered.

use mc683xx_target::address::Address;

/// SR's trace bits (CPU32RM 2.3.3): T1 traces every instruction, T0
/// changes of flow.
pub(crate) const SR_T1: u32 = 0x8000;
pub(crate) const SR_TRACE: u32 = 0xC000;
/// SR's supervisor bit: the program space a stepped instruction is read
/// from.
pub(crate) const SR_S: u32 = 0x2000;

/// BGND (CPU32RM 4, BGND), the stub every vector points at.
pub(crate) const BGND: [u8; 2] = [0x4A, 0xFA];

/// Vectors in an exception table (CPU32RM 6.1.4), and bytes in each.
const VECTORS: u32 = 256;
const VECTOR_BYTES: u32 = 4;
/// The trace vector (CPU32RM Table 6-1).
const TRACE_VECTOR: u32 = 9;

/// Where the stub sits in scratch RAM: after the table.
const STUB_OFFSET: u32 = VECTORS * VECTOR_BYTES;
/// Scratch RAM a step needs: the table and the stub.
pub const SCRATCH_BYTES: u32 = STUB_OFFSET + BGND.len() as u32;

/// The first eight bytes of every frame (CPU32RM 6.4): SR, PC and the
/// format/vector word.
pub(crate) const FRAME_HEAD: usize = 8;
/// The format's and the vector offset's places in the format/vector
/// word.
const FORMAT_SHIFT: u16 = 12;
const VECTOR_OFFSET_MASK: u16 = 0x0FFF;
/// Bytes in each CPU32 frame format (CPU32RM 6.4): $0 four words, $2
/// six, $C twelve (a bus error).
const FOUR_WORD: (u16, usize) = (0x0, 8);
const SIX_WORD: (u16, usize) = (0x2, 12);
const BUS_ERROR: (u16, usize) = (0xC, 24);

/// Opcodes that write the whole SR (CPU32RM 4), T1 and T0 with it:
/// `move <ea>,sr`, by its mask, and `andi`, `ori` and `eori` `#imm,sr`,
/// RTE, STOP and LPSTOP's first word.
const MOVE_TO_SR: (u16, u16) = (0xFFC0, 0x46C0);
const ANDI_TO_SR: u16 = 0x027C;
const ORI_TO_SR: u16 = 0x007C;
const EORI_TO_SR: u16 = 0x0A7C;
const RTE: u16 = 0x4E73;
const STOP: u16 = 0x4E72;
const LPSTOP: u16 = 0xF800;

/// What the stepped instruction does to SR's trace bits, as it leaves
/// them for the next instruction.  The step sets T1 (T0 clear) for
/// itself, so where the instruction keeps the bits the program's go
/// back, and where it writes them its own stay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TraceEffect {
    /// Not an SR writer: the program's bits.
    Kept,
    /// Loads SR whole (MOVE to SR, RTE, STOP, LPSTOP): what it left.
    Loaded,
    /// ANDs, ORs or EORs SR with an immediate: the program's bits so
    /// combined.
    Combined {
        /// `andi`, `ori` or `eori` `#imm,sr`'s opcode.
        opcode: u16,
        /// The immediate.
        immediate: u16,
    },
}

impl TraceEffect {
    /// Whether `opcode` combines SR with an immediate word that follows
    /// it, which [`TraceEffect::of`] then needs.
    pub(crate) const fn needs_immediate(opcode: u16) -> bool {
        matches!(opcode, ANDI_TO_SR | ORI_TO_SR | EORI_TO_SR)
    }

    /// The effect of the instruction whose first word is `opcode`, and
    /// whose second, for an immediate SR op, is `immediate`.
    pub(crate) const fn of(opcode: u16, immediate: u16) -> Self {
        match opcode {
            ANDI_TO_SR | ORI_TO_SR | EORI_TO_SR => Self::Combined { opcode, immediate },
            RTE | STOP | LPSTOP => Self::Loaded,
            _ if opcode & MOVE_TO_SR.0 == MOVE_TO_SR.1 => Self::Loaded,
            _ => Self::Kept,
        }
    }

    /// SR's trace bits after the instruction, from the program's SR
    /// before it and the SR it left with the step's T1.
    pub(crate) const fn trace_bits(self, program_sr: u16, left_sr: u16) -> u16 {
        let trace = SR_TRACE as u16;
        let bits = match self {
            Self::Kept => program_sr,
            Self::Loaded => left_sr,
            Self::Combined { opcode: ANDI_TO_SR, immediate } => program_sr & immediate,
            Self::Combined { opcode: ORI_TO_SR, immediate } => program_sr | immediate,
            Self::Combined { immediate, .. } => program_sr ^ immediate,
        };
        bits & trace
    }
}

/// The most frames one step can stack: the instruction's exception, the
/// trace, and interrupts on top, each of a higher level.  More is a
/// stub reached some other way.
pub(crate) const MAX_FRAMES: usize = 10;

/// Why a step failed.
#[derive(Debug, thiserror::Error)]
pub enum StepError {
    /// No scratch RAM was given for the table and stub.
    #[error("a step needs {SCRATCH_BYTES} bytes of scratch RAM, and none was given")]
    NoScratch,
    /// The scratch RAM's address is odd: the stub is an instruction.
    #[error("scratch RAM at {0} is odd")]
    OddScratch(Address),
    /// The target did not reach the stub; it was halted where it was.
    #[error("the step did not stop; halted at {0}")]
    NotStopped(Address),
    /// A frame on the stack is not a CPU32 format.
    #[error("stack frame at {at}: format ${format:X} is not a CPU32 frame")]
    Format {
        /// Where the frame starts.
        at: Address,
        /// Its format.
        format: u16,
    },
    /// Frames on the stack kept returning to the stub.
    #[error("more than {MAX_FRAMES} frames stacked in one step")]
    TooManyFrames,
}

/// The table and stub in scratch RAM at `scratch`: every vector the
/// stub.
pub(crate) fn scratch_image(scratch: Address) -> Vec<u8> {
    let stub = stub(scratch).0.to_be_bytes();
    let mut image: Vec<u8> = (0..VECTORS).flat_map(|_| stub).collect();
    image.extend_from_slice(&BGND);
    image
}

/// The stub's address.
pub(crate) const fn stub(scratch: Address) -> Address {
    scratch.offset(STUB_OFFSET)
}

/// RPC once the stub's BGND has entered background mode: past it
/// (CPU32RM 4, BGND).
pub(crate) const fn after_stub(scratch: Address) -> Address {
    stub(scratch).offset(BGND.len() as u32)
}

/// Where vector `vector` is in the table at `table`.
pub(crate) const fn vector_address(table: Address, vector: u32) -> Address {
    table.offset(vector * VECTOR_BYTES)
}

/// Bytes in the frame whose first eight bytes are `head`.
///
/// # Errors
/// [`StepError::Format`] for a format the CPU32 does not stack.
pub(crate) fn frame_bytes(head: &[u8; FRAME_HEAD], at: Address) -> Result<usize, StepError> {
    let [.., f0, f1] = *head;
    let format = u16::from_be_bytes([f0, f1]) >> FORMAT_SHIFT;
    [FOUR_WORD, SIX_WORD, BUS_ERROR]
        .into_iter()
        .find(|(known, _)| *known == format)
        .map(|(_, bytes)| bytes)
        .ok_or(StepError::Format { at, format })
}

/// An exception frame (CPU32RM 6.4), or the CPU's own SR and PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    /// The stacked SR.
    pub(crate) sr: u16,
    /// The stacked PC, where an RTE returns.
    pub(crate) pc: u32,
    /// The format and vector offset.
    format_vector: u16,
    /// The bytes after the first eight, as stacked.
    rest: Vec<u8>,
}

impl Frame {
    /// The frame whose first eight bytes are `head` and the rest `rest`.
    pub(crate) fn from_head(head: &[u8; FRAME_HEAD], rest: Vec<u8>) -> Self {
        let [s0, s1, p0, p1, p2, p3, f0, f1] = *head;
        Self {
            sr: u16::from_be_bytes([s0, s1]),
            pc: u32::from_be_bytes([p0, p1, p2, p3]),
            format_vector: u16::from_be_bytes([f0, f1]),
            rest,
        }
    }

    /// The CPU's SR and PC, as a frame with no format.
    pub(crate) const fn cpu(sr: u16, pc: u32) -> Self {
        Self { sr, pc, format_vector: 0, rest: Vec::new() }
    }

    /// The exception's vector number.
    pub(crate) const fn vector(&self) -> u32 {
        (self.format_vector & VECTOR_OFFSET_MASK) as u32 / VECTOR_BYTES
    }

    /// The frame as stacked.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(FRAME_HEAD + self.rest.len());
        bytes.extend_from_slice(&self.sr.to_be_bytes());
        bytes.extend_from_slice(&self.pc.to_be_bytes());
        bytes.extend_from_slice(&self.format_vector.to_be_bytes());
        bytes.extend_from_slice(&self.rest);
        bytes
    }
}

/// Unwinds a step that reached the stub at `stub`.
///
/// `chain` is the CPU (its SR and the stub as PC) and then each frame
/// on the stack, top first, down to the first whose PC is not the
/// stub: the frame stacked from the program, with its SR's trace bits
/// the step's.  `program_sr` is the SR the program had; with T1 set
/// the trace is its own and is kept like any other exception.
/// `effect` is the stepped instruction's on the trace bits.
/// `vector(n)` reads the program's vector `n`.
///
/// Returns the chain as the program's table would have left it: the
/// step's trace frame gone, the frame above it given its SR and PC,
/// the trace bits in the frame stacked from the program put right (the
/// instruction's own if that frame is the trace's, so it ran; the
/// program's if another exception stacked it before the instruction
/// ran), and every PC that was the stub the handler of the exception
/// below.
///
/// # Errors
/// `vector`'s.
pub(crate) fn unwind<E>(
    mut chain: Vec<Frame>,
    stub: Address,
    program_sr: u16,
    effect: TraceEffect,
    mut vector: impl FnMut(u32) -> Result<u32, E>,
) -> Result<Vec<Frame>, E> {
    let trace_bits = SR_TRACE as u16;
    let frames = chain.len();
    if let Some(bottom) = chain.last_mut() {
        let ran = frames > 1 && bottom.vector() == TRACE_VECTOR;
        let effect = if ran { effect } else { TraceEffect::Kept };
        bottom.sr = (bottom.sr & !trace_bits) | effect.trace_bits(program_sr, bottom.sr);
    }
    let program_traces = u32::from(program_sr) & SR_T1 != 0;
    let ours = chain.iter().skip(1).position(|frame| frame.vector() == TRACE_VECTOR).map(|at| at + 1);
    if let Some(at) = ours.filter(|_| !program_traces) {
        let trace = chain.remove(at);
        let above = &mut chain[at - 1];
        above.sr = trace.sr;
        above.pc = trace.pc;
    }
    for at in 0..chain.len() - 1 {
        if chain[at].pc == stub.0 {
            chain[at].pc = vector(chain[at + 1].vector())?;
        }
    }
    Ok(chain)
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use mc683xx_target::address::Address;

    use super::Frame;
    use super::SCRATCH_BYTES;
    use super::StepError;
    use super::TraceEffect;
    use super::frame_bytes;
    use super::scratch_image;
    use super::stub;
    use super::unwind;

    const SCRATCH: Address = Address(0x0010_0000);
    const STUB: u32 = 0x0010_0400;
    /// Supervisor, mask 0: the program's SR.
    const PROGRAM_SR: u16 = 0x2000;
    /// The program's SR as the step ran it, T1 set.
    const TRACED_SR: u16 = 0xA000;
    /// A handler's SR: supervisor, tracing off.
    const HANDLER_SR: u16 = 0x2000;

    /// A frame of `format` for `vector`, stacked with `sr` and `pc`.
    fn frame(format: u16, vector: u16, sr: u16, pc: u32) -> Frame {
        let rest = if format == 2 { vec![0, 0, 0x12, 0x34] } else { Vec::new() };
        let mut head = [0; 8];
        head[..2].copy_from_slice(&sr.to_be_bytes());
        head[2..6].copy_from_slice(&pc.to_be_bytes());
        head[6..].copy_from_slice(&((format << 12) | (vector * 4)).to_be_bytes());
        Frame::from_head(&head, rest)
    }

    /// The program's vector `n`: `$1000 + n * $10`.
    fn program_vector(n: u32) -> Result<u32, Infallible> {
        Ok(0x1000 + n * 0x10)
    }

    #[test]
    fn the_scratch_image_points_every_vector_at_the_stub() {
        let image = scratch_image(SCRATCH);
        assert_eq!(image.len(), SCRATCH_BYTES as usize);
        assert_eq!(stub(SCRATCH), Address(STUB));
        assert!(image[..1024].chunks(4).all(|vector| vector == STUB.to_be_bytes()));
        assert_eq!(image[1024..], [0x4A, 0xFA]);
    }

    #[test]
    fn frame_sizes_by_format() {
        let head = |format: u8| [0, 0, 0, 0, 0, 0, format << 4, 0x24];
        assert_eq!(frame_bytes(&head(0), SCRATCH).unwrap(), 8);
        assert_eq!(frame_bytes(&head(2), SCRATCH).unwrap(), 12);
        assert_eq!(frame_bytes(&head(0xC), SCRATCH).unwrap(), 24);
        assert!(matches!(frame_bytes(&head(1), SCRATCH), Err(StepError::Format { format: 1, .. })));
    }

    #[test]
    fn a_plain_step_drops_the_trace_frame() {
        let chain = vec![Frame::cpu(HANDLER_SR, STUB), frame(2, 9, TRACED_SR, 0x2002)];
        let unwound = unwind(chain, Address(STUB), PROGRAM_SR, TraceEffect::Kept, program_vector).unwrap();
        assert_eq!(unwound, [Frame::cpu(PROGRAM_SR, 0x2002)]);
    }

    #[test]
    fn a_trap_is_taken_to_its_handler() {
        let chain = vec![
            Frame::cpu(HANDLER_SR, STUB),
            frame(2, 9, HANDLER_SR, STUB),
            frame(0, 33, TRACED_SR, 0x2002),
        ];
        let unwound = unwind(chain, Address(STUB), PROGRAM_SR, TraceEffect::Kept, program_vector).unwrap();
        assert_eq!(unwound, [Frame::cpu(HANDLER_SR, 0x1210), frame(0, 33, PROGRAM_SR, 0x2002)]);
    }

    #[test]
    fn an_interrupt_after_the_trace_is_stacked_where_the_trace_was() {
        let interrupt_sr = 0x2300;
        let chain = vec![
            Frame::cpu(interrupt_sr, STUB),
            frame(0, 64, HANDLER_SR, STUB),
            frame(2, 9, TRACED_SR, 0x2002),
        ];
        let unwound = unwind(chain, Address(STUB), PROGRAM_SR, TraceEffect::Kept, program_vector).unwrap();
        assert_eq!(unwound, [Frame::cpu(interrupt_sr, 0x1400), frame(0, 64, PROGRAM_SR, 0x2002)]);
    }

    #[test]
    fn an_interrupt_at_the_start_is_taken_untraced() {
        let chain = vec![Frame::cpu(0x2500, STUB), frame(0, 70, TRACED_SR | 0x0004, 0x2000)];
        let unwound = unwind(chain, Address(STUB), PROGRAM_SR, TraceEffect::Kept, program_vector).unwrap();
        assert_eq!(unwound, [Frame::cpu(0x2500, 0x1460), frame(0, 70, PROGRAM_SR | 0x0004, 0x2000)]);
    }

    #[test]
    fn a_program_tracing_itself_keeps_its_trace() {
        let chain = vec![Frame::cpu(HANDLER_SR, STUB), frame(2, 9, TRACED_SR, 0x2002)];
        let unwound = unwind(chain, Address(STUB), TRACED_SR, TraceEffect::Kept, program_vector).unwrap();
        assert_eq!(unwound, [Frame::cpu(HANDLER_SR, 0x1090), frame(2, 9, TRACED_SR, 0x2002)]);
    }

    #[test]
    fn a_program_tracing_changes_of_flow_gets_its_t0_back() {
        let t0 = 0x6000;
        let chain = vec![Frame::cpu(HANDLER_SR, STUB), frame(2, 9, TRACED_SR, 0x2002)];
        let unwound = unwind(chain, Address(STUB), t0, TraceEffect::Kept, program_vector).unwrap();
        assert_eq!(unwound, [Frame::cpu(t0, 0x2002)]);
    }

    /// `move #$a000,sr` and `move #$2000,sr`'s effect.
    const LOADED: TraceEffect = TraceEffect::Loaded;

    #[test]
    fn a_stepped_move_to_sr_keeps_the_t1_it_set() {
        let chain = vec![Frame::cpu(HANDLER_SR, STUB), frame(2, 9, TRACED_SR, 0x2004)];
        let unwound = unwind(chain, Address(STUB), PROGRAM_SR, LOADED, program_vector).unwrap();
        assert_eq!(unwound, [Frame::cpu(TRACED_SR, 0x2004)]);
    }

    #[test]
    fn a_stepped_move_to_sr_keeps_the_t1_it_cleared() {
        // The program traced itself, so its trace is taken; the frame
        // carries the SR the instruction loaded.
        let chain = vec![Frame::cpu(HANDLER_SR, STUB), frame(2, 9, PROGRAM_SR, 0x2004)];
        let unwound = unwind(chain, Address(STUB), TRACED_SR, LOADED, program_vector).unwrap();
        assert_eq!(unwound, [Frame::cpu(HANDLER_SR, 0x1090), frame(2, 9, PROGRAM_SR, 0x2004)]);
    }

    #[test]
    fn immediate_sr_ops_combine_with_the_programs_bits() {
        let t1 = 0x8000;
        for (opcode, immediate, left, expected) in [
            (0x007C, t1, TRACED_SR, TRACED_SR),       // ori: T1 set
            (0x0A7C, t1, PROGRAM_SR, TRACED_SR),      // eori: T1 toggled on, not off
            (0x027C, 0x7FFF, PROGRAM_SR, PROGRAM_SR), // andi: T1 cleared
            (0x027C, 0xFFFF, TRACED_SR, PROGRAM_SR),  // andi: the step's T1 not kept
        ] {
            let effect = TraceEffect::of(opcode, immediate);
            assert!(TraceEffect::needs_immediate(opcode));
            let chain = vec![Frame::cpu(HANDLER_SR, STUB), frame(2, 9, left, 0x2004)];
            let unwound = unwind(chain, Address(STUB), PROGRAM_SR, effect, program_vector).unwrap();
            assert_eq!(unwound, [Frame::cpu(expected, 0x2004)], "{opcode:04x} #{immediate:04x}");
        }
    }

    #[test]
    fn an_sr_writer_that_did_not_run_leaves_the_programs_bits() {
        // An interrupt taken before it: the frame stacked from the
        // program is the interrupt's, with the step's T1.
        let chain = vec![Frame::cpu(0x2500, STUB), frame(0, 70, TRACED_SR, 0x2000)];
        let unwound = unwind(chain, Address(STUB), PROGRAM_SR, LOADED, program_vector).unwrap();
        assert_eq!(unwound, [Frame::cpu(0x2500, 0x1460), frame(0, 70, PROGRAM_SR, 0x2000)]);
    }

    #[test]
    fn sr_writers_are_known_by_their_opcodes() {
        for opcode in [0x46FC, 0x46C0, 0x46D7, 0x4E73, 0x4E72, 0xF800] {
            assert_eq!(TraceEffect::of(opcode, 0), TraceEffect::Loaded, "{opcode:04x}");
        }
        // NOP, MOVE to CCR, ORI to CCR, MOVE from SR: not SR writers.
        for opcode in [0x4E71, 0x44FC, 0x003C, 0x40C0] {
            assert_eq!(TraceEffect::of(opcode, 0), TraceEffect::Kept, "{opcode:04x}");
            assert!(!TraceEffect::needs_immediate(opcode));
        }
    }

    #[test]
    fn frames_keep_their_bytes() {
        let six = frame(2, 5, 0x2700, 0x0012_3456);
        assert_eq!(six.bytes(), [0x27, 0, 0, 0x12, 0x34, 0x56, 0x20, 0x14, 0, 0, 0x12, 0x34]);
        assert_eq!(six.vector(), 5);
    }
}
