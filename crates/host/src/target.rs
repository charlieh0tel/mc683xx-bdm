//! [`Cpu32Target`] over the pod: the trait's halts, registers and
//! memory, with what it needs kept between requests (the entry source,
//! the program's SFC and DFC), and its step by T1 trace through an
//! exception table in scratch RAM (`step`).  The pod reads ATEMP first
//! after each entry and reports it (`mc683xx_bdm_proto::entry`); this never
//! reads ATEMP.

use std::time::Duration;
use std::time::Instant;

use mc683xx_bdm_proto::usb::Atemp;
use mc683xx_bdm_proto::usb::ReplyError;
use mc683xx_bdm_proto::usb::Status;
use mc683xx_target::Cpu32Target;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::function_code::FunctionCode;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;

use crate::pod::Error;
use crate::pod::Pod;
use crate::step::FRAME_HEAD;
use crate::step::Frame;
use crate::step::MAX_FRAMES;
use crate::step::SR_T1;
use crate::step::SR_TRACE;
use crate::step::StepError;
use crate::step::after_stub;
use crate::step::frame_bytes;
use crate::step::scratch_image;
use crate::step::stub;
use crate::step::unwind;
use crate::step::vector_address;
use crate::transport::Transport;

/// How long a step may run before it is taken as not stopping: many
/// times one instruction's and its exceptions', and a few USB round
/// trips.
const STEP_TIMEOUT: Duration = Duration::from_millis(100);

/// The supervisor data space, where the step's table, stub and frames
/// are read and written, as the CPU stacks and fetches vectors.
const SUPERVISOR_DATA: FunctionCode = FunctionCode::SUPERVISOR_DATA;

/// One of SFC and DFC: the program's value, and what the CPU holds now.
/// Both `None` until read or written; they differ only after a memory
/// call set the register to its function code.
#[derive(Debug, Default, Clone, Copy)]
struct FunctionCodeRegister {
    /// The value the program had, and resuming restores.
    program: Option<u32>,
    /// The value in the CPU.
    current: Option<u32>,
}

/// The target while halted: what it keeps until it resumes.
#[derive(Debug, Clone, Copy)]
struct Halted {
    /// The entry source, from ATEMP read first after entry.
    entry: Entry,
    /// SFC, which READ and DUMP go through (CPU32RM 7.2.8.8).
    sfc: FunctionCodeRegister,
    /// DFC, which WRITE and FILL go through.
    dfc: FunctionCodeRegister,
}

impl Halted {
    fn new(entry: Entry) -> Self {
        Self { entry, sfc: FunctionCodeRegister::default(), dfc: FunctionCodeRegister::default() }
    }

    fn register(&mut self, register: SystemRegister) -> Option<&mut FunctionCodeRegister> {
        match register {
            SystemRegister::Sfc => Some(&mut self.sfc),
            SystemRegister::Dfc => Some(&mut self.dfc),
            _ => None,
        }
    }
}

/// A CPU32 through the pod, as [`Cpu32Target`].
///
/// The entry source is the pod's: [`Entry::Unknown`] when the pod did
/// not see the entry (it started with the CPU already halted).
#[derive(Debug)]
pub struct PodTarget<T> {
    pod: Pod<T>,
    /// `Some` while the target is known to be halted.
    halted: Option<Halted>,
    /// Where a step's exception table and stub go
    /// ([`step::SCRATCH_BYTES`](crate::step::SCRATCH_BYTES)).
    scratch: Option<Address>,
    /// The table and stub are in scratch RAM: written since the
    /// program last ran, other than in a step, or was reset.
    scratch_written: bool,
}

impl<T: Transport> PodTarget<T> {
    /// The target behind `pod`.
    pub const fn new(pod: Pod<T>) -> Self {
        Self { pod, halted: None, scratch: None, scratch_written: false }
    }

    /// Gives a step `scratch`, [`step::SCRATCH_BYTES`](crate::step::SCRATCH_BYTES)
    /// of RAM the program does not use, for its exception table and
    /// stub; `None` and [`Cpu32Target::step`] fails.
    pub const fn set_scratch(&mut self, scratch: Option<Address>) {
        self.scratch = scratch;
        self.scratch_written = false;
    }

    /// The pod, for the requests the trait does not carry (pins, clock,
    /// raw transfers).  What it does bypasses what this keeps: a raw
    /// transfer that resumes the target or changes SFC or DFC leaves
    /// this wrong until [`Cpu32Target::reset_halted`].
    pub const fn pod(&mut self) -> &mut Pod<T> {
        &mut self.pod
    }

    /// Stacks RPC and runs from `address` (CPU32RM 7.2.8.13), the
    /// program's SFC and DFC restored first.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn call(&mut self, address: Address) -> Result<(), Error> {
        self.leave()?;
        self.scratch_written = false;
        self.pod.call(address)
    }

    /// Puts back the program's SFC and DFC where a memory call changed
    /// them, without resuming: for a caller that stops using the target
    /// halted, such as a command-line tool about to exit.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn restore_function_codes(&mut self) -> Result<(), Error> {
        for register in [SystemRegister::Sfc, SystemRegister::Dfc] {
            let Some(&mut state) = function_code_register(&mut self.halted, register) else { return Ok(()) };
            if let Some(program) = state.program
                && state.current != Some(program)
            {
                self.request(|pod| pod.write_system_register(register, program))?;
                self.function_code_state(register).current = Some(program);
            }
        }
        Ok(())
    }

    /// Holds BKPT low until the next request (`Pod::hold`); forgets
    /// the halted state.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn hold(&mut self) -> Result<(), Error> {
        self.halted = None;
        self.pod.hold()
    }

    /// Resets the target to run, background mode off; forgets the
    /// halted state.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn reset_running(&mut self) -> Result<(), Error> {
        self.halted = None;
        self.scratch_written = false;
        self.pod.reset_run()
    }

    /// Brings the halted target's serial link back into frame; its
    /// entry source is now unknown.  The program's SFC and DFC are
    /// kept, to be put back on resuming, and what the CPU holds is taken
    /// as unknown: a write may have reached it before the link failed.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn resync(&mut self) -> Result<Atemp, Error> {
        let kept = self.halted.take();
        self.scratch_written = false;
        let atemp = self.pod.resync()?;
        if let Some(kept) = kept
            && let Ok(Some(entry)) = entry(atemp)
        {
            let unknown = |state: FunctionCodeRegister| FunctionCodeRegister { current: None, ..state };
            self.halted = Some(Halted { entry, sfc: unknown(kept.sfc), dfc: unknown(kept.dfc) });
        }
        Ok(atemp)
    }

    /// `call` on the pod, the halted state forgotten if the pod found
    /// the target running: it left background mode (a reset, say) since
    /// it was last seen halted.
    fn request<R>(&mut self, call: impl FnOnce(&mut Pod<T>) -> Result<R, Error>) -> Result<R, Error> {
        let result = call(&mut self.pod);
        if matches!(result, Err(Error::Reply(ReplyError::Failed(Status::NoFreeze)))) {
            self.halted = None;
        }
        result
    }

    /// SFC's or DFC's state, while the target is halted.
    fn function_code_state(&mut self, register: SystemRegister) -> &mut FunctionCodeRegister {
        function_code_register(&mut self.halted, register).expect("halted, on SFC or DFC")
    }

    /// Puts back the program's SFC and DFC and forgets the halted
    /// state, before a command that resumes the target.
    fn leave(&mut self) -> Result<(), Error> {
        self.restore_function_codes()?;
        self.halted = None;
        Ok(())
    }

    /// Why the target is halted, asking the pod if this is the first
    /// look since it stopped; `None` while it runs.
    fn look(&mut self) -> Result<Option<Entry>, Error> {
        if let Some(halted) = self.halted {
            return Ok(Some(halted.entry));
        }
        let Some(entry) = entry(self.pod.pins()?.atemp)? else { return Ok(None) };
        self.halted = Some(Halted::new(entry));
        Ok(Some(entry))
    }

    /// The halted state, asking the pod for the entry source if this is
    /// the first look since the target stopped.
    ///
    /// # Errors
    /// As [`Error`]; the pod's `NoFreeze` if the target runs.
    fn halted_state(&mut self) -> Result<&mut Halted, Error> {
        self.look()?.ok_or(ReplyError::Failed(Status::NoFreeze))?;
        Ok(self.halted.as_mut().expect("look keeps the halted state"))
    }

    /// Keeps the entry a halt or reset reported.
    fn halted_at(&mut self, atemp: Atemp) -> Result<Entry, Error> {
        let entry = entry(atemp)?.ok_or(ReplyError::Malformed)?;
        self.halted = Some(Halted::new(entry));
        Ok(entry)
    }

    /// Waits for a step's GO to end in background mode; `None` if it
    /// did not within [`STEP_TIMEOUT`] and was halted.
    fn stop_stepping(&mut self) -> Result<Option<Entry>, Error> {
        let deadline = Instant::now() + STEP_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(entry) = self.look()? {
                return Ok(Some(entry));
            }
        }
        self.halt()?;
        Ok(None)
    }

    /// After a step that did not reach the stub: the program's VBR, and
    /// its trace bits in SR.
    fn untrace(&mut self, program_sr: u32, program_vbr: u32) -> Result<(), Error> {
        let sr = self.read_system_register(SystemRegister::Sr)?;
        self.write_system_register(SystemRegister::Sr, (sr & !SR_TRACE) | (program_sr & SR_TRACE))?;
        self.write_system_register(SystemRegister::Vbr, program_vbr)
    }

    /// After a step that reached the stub: reads the frames stacked on
    /// the way, unwinds them into the program's table's state, and
    /// writes that back with the program's VBR.
    fn unwind(&mut self, stub: Address, program_sr: u16, program_vbr: u32) -> Result<(), Error> {
        let sp = self.read_register(Register::A7)?;
        let sr = self.read_system_register(SystemRegister::Sr)? as u16;
        let mut chain = vec![Frame::cpu(sr, stub.0)];
        let mut at = Address(sp);
        while chain.last().is_some_and(|frame| frame.pc == stub.0) {
            if chain.len() > MAX_FRAMES {
                return Err(StepError::TooManyFrames.into());
            }
            let mut head = [0; FRAME_HEAD];
            self.read_memory(at, SUPERVISOR_DATA, Size::Word, &mut head)?;
            let bytes = frame_bytes(&head, at)?;
            let mut rest = vec![0; bytes - FRAME_HEAD];
            if !rest.is_empty() {
                self.read_memory(at.offset(FRAME_HEAD as u32), SUPERVISOR_DATA, Size::Word, &mut rest)?;
            }
            chain.push(Frame::from_head(&head, rest));
            at = at.offset(bytes as u32);
        }
        let bottom = at;
        let chain = unwind(chain, stub, program_sr, |vector| {
            let mut handler = [0; 4];
            let address = vector_address(Address(program_vbr), vector);
            self.read_memory(address, SUPERVISOR_DATA, Size::Word, &mut handler)?;
            Ok::<_, Error>(u32::from_be_bytes(handler))
        })?;
        let (cpu, frames) = chain.split_first().expect("the chain starts with the CPU");
        let stacked: Vec<u8> = frames.iter().flat_map(Frame::bytes).collect();
        let sp = Address(bottom.0.wrapping_sub(stacked.len() as u32));
        if !stacked.is_empty() {
            self.write_memory(sp, SUPERVISOR_DATA, Size::Word, &stacked)?;
        }
        // A7 while S is set, so SSP; an SR clearing S then makes USP A7.
        // SR last: until it is written a failure's `untrace` is right.
        self.write_register(Register::A7, sp.0)?;
        self.write_system_register(SystemRegister::Rpc, cpu.pc)?;
        self.write_system_register(SystemRegister::Vbr, program_vbr)?;
        self.write_system_register(SystemRegister::Sr, u32::from(cpu.sr))
    }

    /// The step from VBR at the stub's table on: SR's T1 set, GO, and
    /// the stop worked out.
    fn trace(&mut self, scratch: Address, program_sr: u32, program_vbr: u32) -> Result<Entry, Error> {
        self.write_system_register(SystemRegister::Vbr, scratch.0)?;
        self.write_system_register(SystemRegister::Sr, (program_sr & !SR_TRACE) | SR_T1)?;
        self.leave()?;
        self.pod.go()?;
        let entry = self.stop_stepping()?;
        let pc = self.read_system_register(SystemRegister::Rpc)?;
        // RPC past the stub is the stub's BGND, even when the wait ran
        // out and the halt found the target already there.
        let at_stub = pc == after_stub(scratch).0 && entry.is_none_or(|entry| entry == Entry::Background);
        if at_stub {
            self.unwind(stub(scratch), program_sr as u16, program_vbr)?;
            return Ok(Entry::Breakpoint);
        }
        self.untrace(program_sr, program_vbr)?;
        entry.ok_or_else(|| StepError::NotStopped(Address(pc)).into())
    }

    /// Sets SFC or DFC to `function_code` for a memory call, reading the
    /// program's value first so it can be restored.
    fn select(&mut self, register: SystemRegister, function_code: FunctionCode) -> Result<(), Error> {
        let wanted = u32::from(function_code.bits());
        let state = *self.function_code_state(register);
        if state.current == Some(wanted) {
            return Ok(());
        }
        if state.program.is_none() {
            let value = self.request(|pod| pod.read_system_register(register))?;
            let state = self.function_code_state(register);
            state.program = Some(value);
            state.current = Some(value);
            if FunctionCode::new(value) == function_code {
                return Ok(());
            }
        }
        // Unknown until the reply: the write may reach the CPU and its
        // reply be lost, and resuming then puts the program's back.
        self.function_code_state(register).current = None;
        self.request(|pod| pod.write_system_register(register, wanted))?;
        self.function_code_state(register).current = Some(wanted);
        Ok(())
    }
}

impl<T: Transport> Cpu32Target for PodTarget<T> {
    type Error = Error;

    fn reset_halted(&mut self) -> Result<Entry, Error> {
        self.halted = None;
        self.scratch_written = false;
        let atemp = self.pod.reset_halt()?;
        self.halted_at(atemp)
    }

    /// A halt already seen is confirmed by FREEZE, so a target that
    /// left background mode since (a reset) is halted again.
    fn halt(&mut self) -> Result<Entry, Error> {
        if let Some(halted) = self.halted {
            if self.pod.pins()?.freeze {
                return Ok(halted.entry);
            }
            self.halted = None;
        }
        let atemp = self.pod.halt()?;
        self.halted_at(atemp)
    }

    fn halted(&mut self) -> Result<Option<Entry>, Error> {
        self.look()
    }

    fn go(&mut self) -> Result<(), Error> {
        self.leave()?;
        self.scratch_written = false;
        self.pod.go()
    }

    /// T1 trace with VBR pointing at a table in scratch RAM whose every
    /// vector is a BGND stub (PLAN.md, Host, gdb).  At the stub the
    /// frames stacked on the way are unwound (`step::unwind`) and VBR
    /// put back.  A step that stops elsewhere (a BGND stepped, a double
    /// bus fault) gets VBR and the program's trace bits back.  The
    /// program's T0 is not honored: its trace on a change of flow is
    /// not taken.  A step that fails once VBR is changed gets VBR and
    /// the trace bits back, best effort; frames it had begun to unwind
    /// stay as they were left.
    ///
    /// # Errors
    /// [`StepError`]: no scratch RAM, a step that did not stop (it is
    /// then halted), or frames not as a step stacks them; or as
    /// [`Error`].
    fn step(&mut self) -> Result<Entry, Error> {
        let scratch = self.scratch.ok_or(StepError::NoScratch)?;
        if scratch.is_odd() {
            return Err(StepError::OddScratch(scratch).into());
        }
        self.halted_state()?;
        let program_sr = self.read_system_register(SystemRegister::Sr)?;
        let program_vbr = self.read_system_register(SystemRegister::Vbr)?;
        if !self.scratch_written {
            self.write_memory(scratch, SUPERVISOR_DATA, Size::Word, &scratch_image(scratch))?;
            self.scratch_written = true;
        }
        let stepped = self.trace(scratch, program_sr, program_vbr);
        if let Err(error) = &stepped
            && !matches!(error, Error::Step(StepError::NotStopped(_)))
        {
            // Best effort, and the step's error is the one reported: a
            // target that runs or a link that failed refuses this too.
            self.untrace(program_sr, program_vbr).ok();
        }
        stepped
    }

    fn read_register(&mut self, register: Register) -> Result<u32, Error> {
        self.halted_state()?;
        self.request(|pod| pod.read_register(register))
    }

    fn write_register(&mut self, register: Register, value: u32) -> Result<(), Error> {
        self.halted_state()?;
        self.request(|pod| pod.write_register(register, value))
    }

    fn read_system_register(&mut self, register: SystemRegister) -> Result<u32, Error> {
        let halted = self.halted_state()?;
        if let Some(program) = halted.register(register).and_then(|state| state.program) {
            return Ok(program);
        }
        let value = self.request(|pod| pod.read_system_register(register))?;
        if let Some(state) = function_code_register(&mut self.halted, register) {
            state.program = Some(value);
            state.current = Some(value);
        }
        Ok(value)
    }

    fn write_system_register(&mut self, register: SystemRegister, value: u32) -> Result<(), Error> {
        self.halted_state()?;
        self.request(|pod| pod.write_system_register(register, value))?;
        if let Some(state) = function_code_register(&mut self.halted, register) {
            state.program = Some(value);
            state.current = Some(value);
        }
        Ok(())
    }

    fn read_memory(
        &mut self,
        address: Address,
        function_code: FunctionCode,
        size: Size,
        data: &mut [u8],
    ) -> Result<(), Error> {
        self.halted_state()?;
        self.select(SystemRegister::Sfc, function_code)?;
        self.request(|pod| pod.read_memory(address, size, data))
    }

    fn write_memory(
        &mut self,
        address: Address,
        function_code: FunctionCode,
        size: Size,
        data: &[u8],
    ) -> Result<(), Error> {
        self.halted_state()?;
        self.select(SystemRegister::Dfc, function_code)?;
        self.request(|pod| pod.write_memory(address, size, data))
    }
}

/// The entry source a pod's ATEMP gives, `None` while the target runs.
///
/// # Errors
/// The pod could not read ATEMP after this entry: a target halted with
/// no entry source cannot be worked on until the next entry.
const fn entry(atemp: Atemp) -> Result<Option<Entry>, ReplyError> {
    match atemp {
        Atemp::None => Ok(None),
        Atemp::Fresh(value) => Ok(Some(Entry::from_atemp(value))),
        Atemp::Stale(value) => Ok(Some(Entry::Unknown { atemp: value })),
        Atemp::Unread => Err(ReplyError::Failed(Status::Unexpected)),
    }
}

/// SFC's or DFC's state if `register` is one of them and the target is
/// halted.
fn function_code_register(
    halted: &mut Option<Halted>,
    register: SystemRegister,
) -> Option<&mut FunctionCodeRegister> {
    halted.as_mut().and_then(|halted| halted.register(register))
}

#[cfg(test)]
mod tests {
    use mc683xx_bdm_proto::usb::Atemp;
    use mc683xx_bdm_proto::usb::PACKET;
    use mc683xx_bdm_proto::usb::Pins;
    use mc683xx_bdm_proto::usb::ReplyError;
    use mc683xx_bdm_proto::usb::ReplyHeader;
    use mc683xx_bdm_proto::usb::Request;

    use mc683xx_bdm_proto::usb::Status;
    use mc683xx_bdm_proto::usb::encode_atemp;
    use mc683xx_bdm_proto::usb::encode_pins;
    use mc683xx_target::Cpu32Target;
    use mc683xx_target::address::Address;
    use mc683xx_target::entry::Entry;
    use mc683xx_target::function_code::FunctionCode;
    use mc683xx_target::register::Register;
    use mc683xx_target::register::SystemRegister;
    use mc683xx_target::size::Size;

    use super::PodTarget;
    use crate::pod::Error;
    use crate::script::Answers;
    use crate::script::Script;
    use crate::script::answering;
    use crate::script::parsed;
    use crate::script::pod;
    use crate::step::SR_T1;
    use crate::step::StepError;
    use crate::step::after_stub;
    use crate::transport::Transport;

    /// A reply carrying a u32.
    fn value(value: u32) -> Vec<u8> {
        std::iter::once(0).chain(value.to_le_bytes()).collect()
    }

    /// A reply with no body.
    const DONE: &[u8] = &[0];

    /// A header the script replaces: replies here are status and body.
    const HEADER: ReplyHeader = ReplyHeader { tag: 0, freeze_edges: 0 };
    const HEADER_BYTES: usize = 2;

    /// A Pins reply: RESET and DS high, 5 V, FREEZE high unless `atemp`
    /// is `Atemp::None`.
    fn pins(atemp: Atemp) -> Vec<u8> {
        let mut packet = [0; PACKET];
        let pins =
            Pins { freeze: atemp != Atemp::None, reset: true, ds: true, berr: true, vdd_mv: 5000, atemp };
        let length = encode_pins(HEADER, &pins, &mut packet);
        packet[HEADER_BYTES..length].to_vec()
    }

    /// A `Halt` or `ResetHalt` reply.
    fn halted(atemp: Atemp) -> Vec<u8> {
        let mut packet = [0; PACKET];
        let length = encode_atemp(HEADER, atemp, &mut packet);
        packet[HEADER_BYTES..length].to_vec()
    }

    fn target(replies: &[&[u8]]) -> PodTarget<Script> {
        PodTarget::new(pod(replies))
    }

    fn requests(target: PodTarget<Script>) -> Vec<Vec<u8>> {
        target.pod.into_transport().requests
    }

    /// A step's scratch RAM, the program's SR and VBR, and A7 at the
    /// stub, with the trace frame on top.
    const SCRATCH: Address = Address(0x0010_0000);
    const PROGRAM_SR: u32 = 0x2000;
    const PROGRAM_VBR: u32 = 0;
    const STACK: u32 = 0x3000;
    /// The trace frame's PC, the instruction after the one stepped.
    const TRACED_PC: u32 = 0x2002;
    /// The trace frame: format $2, vector 9, the stepped SR with T1.
    const TRACE_FRAME: [u8; 12] = [0xA0, 0x00, 0, 0, 0x20, 0x02, 0x20, 0x24, 0, 0, 0x12, 0x34];

    /// The pod's reply to `request` in a step that reaches the stub with
    /// the trace frame alone stacked.
    fn stepping(request: Request<'_>) -> Vec<u8> {
        let memory = |bytes: &[u8]| std::iter::once(0).chain(bytes.iter().copied()).collect();
        match request {
            Request::Halt => halted(Atemp::Fresh(1)),
            Request::Pins => pins(Atemp::Fresh(1)),
            Request::ReadSystemRegister(SystemRegister::Sr) => value(PROGRAM_SR),
            Request::ReadSystemRegister(SystemRegister::Vbr) => value(PROGRAM_VBR),
            Request::ReadSystemRegister(SystemRegister::Sfc) => value(5),
            Request::ReadSystemRegister(SystemRegister::Rpc) => value(after_stub(SCRATCH).0),
            Request::ReadRegister(Register::A7) => value(STACK),
            Request::ReadMemory { address: Address(STACK), .. } => memory(&TRACE_FRAME[..8]),
            Request::ReadMemory { address: Address(address), .. } if address == STACK + 8 => {
                memory(&TRACE_FRAME[8..])
            }
            _ => DONE.to_vec(),
        }
    }

    /// A halted target over `pod`, with scratch RAM already written.
    fn halted_for_a_step<T: Transport>(pod: crate::pod::Pod<T>) -> PodTarget<T> {
        let mut target = PodTarget::new(pod);
        target.set_scratch(Some(SCRATCH));
        target.scratch_written = true;
        target.halt().unwrap();
        target
    }

    /// The requests `target` sent.
    fn answered<F: FnMut(Request<'_>) -> Vec<u8>>(target: PodTarget<Answers<F>>) -> Vec<Vec<u8>> {
        target.pod.into_transport().requests
    }

    /// What a failed step does last: the program's trace bits and VBR
    /// back.
    fn untraced() -> [Request<'static>; 3] {
        [
            Request::ReadSystemRegister(SystemRegister::Sr),
            Request::WriteSystemRegister(SystemRegister::Sr, PROGRAM_SR),
            Request::WriteSystemRegister(SystemRegister::Vbr, PROGRAM_VBR),
        ]
    }

    #[test]
    fn a_step_to_the_stub_unwinds_and_writes_sr_last() {
        let mut target = halted_for_a_step(answering(stepping));
        assert_eq!(target.step().unwrap(), Entry::Breakpoint);
        let sent = answered(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(
            sent[sent.len() - 4..],
            [
                Request::WriteRegister(Register::A7, STACK + TRACE_FRAME.len() as u32),
                Request::WriteSystemRegister(SystemRegister::Rpc, TRACED_PC),
                Request::WriteSystemRegister(SystemRegister::Vbr, PROGRAM_VBR),
                Request::WriteSystemRegister(SystemRegister::Sr, PROGRAM_SR),
            ]
        );
    }

    #[test]
    fn a_step_at_the_stub_as_the_wait_ends_is_unwound() {
        let mut target = halted_for_a_step(answering(|request| match request {
            Request::Pins => pins(Atemp::None),
            request => stepping(request),
        }));
        assert_eq!(target.step().unwrap(), Entry::Breakpoint);
        let sent = answered(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert!(sent.contains(&Request::Halt));
        assert_eq!(sent.last(), Some(&Request::WriteSystemRegister(SystemRegister::Sr, PROGRAM_SR)));
        assert!(sent.contains(&Request::WriteSystemRegister(SystemRegister::Rpc, TRACED_PC)));
    }

    #[test]
    fn a_step_that_does_not_stop_is_halted_and_untraced() {
        let elsewhere = 0x2000;
        let mut target = halted_for_a_step(answering(|request| match request {
            Request::Pins => pins(Atemp::None),
            Request::ReadSystemRegister(SystemRegister::Rpc) => value(elsewhere),
            request => stepping(request),
        }));
        assert!(matches!(target.step(), Err(Error::Step(StepError::NotStopped(Address(0x2000))))));
        let sent = answered(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(sent[sent.len() - 3..], untraced());
    }

    #[test]
    fn a_step_failing_before_go_gets_vbr_and_trace_back() {
        let mut target = halted_for_a_step(answering(|request| match request {
            Request::WriteSystemRegister(SystemRegister::Sr, sr) if sr & SR_T1 != 0 => {
                vec![Status::Illegal as u8]
            }
            request => stepping(request),
        }));
        assert!(matches!(target.step(), Err(Error::Reply(ReplyError::Failed(Status::Illegal)))));
        let sent = answered(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert!(!sent.contains(&Request::Go));
        assert_eq!(sent[sent.len() - 3..], untraced());
    }

    #[test]
    fn a_step_whose_frames_do_not_parse_gets_vbr_and_trace_back() {
        let mut target = halted_for_a_step(answering(|request| match request {
            Request::ReadMemory { address: Address(STACK), .. } => vec![0, 0x20, 0, 0, 0, 0, 0, 0x10, 0x24],
            request => stepping(request),
        }));
        assert!(matches!(target.step(), Err(Error::Step(StepError::Format { format: 1, .. }))));
        let sent = answered(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(sent[sent.len() - 3..], untraced());
    }

    #[test]
    fn a_halt_is_kept_until_resumed() {
        let mut target =
            target(&[&halted(Atemp::Fresh(1)), &pins(Atemp::Fresh(1)), DONE, &pins(Atemp::None)]);
        assert_eq!(target.halt().unwrap(), Entry::Background);
        assert_eq!(target.halt().unwrap(), Entry::Background);
        assert_eq!(target.halted().unwrap(), Some(Entry::Background));
        target.go().unwrap();
        assert_eq!(target.halted().unwrap(), None);
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(sent, [Request::Halt, Request::Pins, Request::Go, Request::Pins]);
    }

    #[test]
    fn a_kept_halt_without_freeze_halts_again() {
        let mut target = target(&[&halted(Atemp::Fresh(1)), &pins(Atemp::None), &halted(Atemp::Fresh(0))]);
        assert_eq!(target.halt().unwrap(), Entry::Background);
        assert_eq!(target.halt().unwrap(), Entry::Breakpoint);
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(sent, [Request::Halt, Request::Pins, Request::Halt]);
    }

    #[test]
    fn no_freeze_forgets_the_halt() {
        let no_freeze = [Status::NoFreeze as u8];
        let mut target = target(&[&halted(Atemp::Fresh(1)), &no_freeze, &pins(Atemp::None)]);
        target.halt().unwrap();
        assert!(matches!(
            target.read_register(Register::D0),
            Err(Error::Reply(ReplyError::Failed(Status::NoFreeze)))
        ));
        assert_eq!(target.halted().unwrap(), None, "asks the pod again");
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(sent, [Request::Halt, Request::ReadRegister(Register::D0), Request::Pins]);
    }

    #[test]
    fn no_freeze_on_a_memory_call_forgets_the_halt() {
        let no_freeze = [Status::NoFreeze as u8];
        let mut target = target(&[&halted(Atemp::Fresh(0)), &value(5), &no_freeze, &pins(Atemp::None)]);
        target.reset_halted().unwrap();
        let mut data = [0; 1];
        assert!(
            target.read_memory(Address(0), FunctionCode::SUPERVISOR_DATA, Size::Byte, &mut data).is_err()
        );
        assert_eq!(target.halted().unwrap(), None);
    }

    #[test]
    fn a_halt_seen_by_polling_is_the_pods() {
        let mut target = target(&[&pins(Atemp::Fresh(0)), &value(0x55)]);
        assert_eq!(target.halted().unwrap(), Some(Entry::Breakpoint));
        assert_eq!(target.read_register(Register::D0).unwrap(), 0x55);
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(sent, [Request::Pins, Request::ReadRegister(Register::D0)]);
    }

    #[test]
    fn a_register_call_asks_the_pod_why_first() {
        let mut target = target(&[&pins(Atemp::Fresh(1)), &value(0x55)]);
        assert_eq!(target.read_register(Register::D0).unwrap(), 0x55);
        assert_eq!(target.halted().unwrap(), Some(Entry::Background));
    }

    #[test]
    fn a_register_call_while_running_is_refused() {
        let mut target = target(&[&pins(Atemp::None)]);
        assert!(matches!(
            target.read_register(Register::D0),
            Err(Error::Reply(ReplyError::Failed(Status::NoFreeze)))
        ));
    }

    #[test]
    fn an_unseen_entry_is_unknown() {
        let mut target = target(&[&pins(Atemp::Stale(0))]);
        assert_eq!(target.halted().unwrap(), Some(Entry::Unknown { atemp: 0 }));
    }

    #[test]
    fn an_unlisted_atemp_is_still_a_halt() {
        let mut target = target(&[&halted(Atemp::Fresh(0x1234))]);
        assert_eq!(target.reset_halted().unwrap(), Entry::Unknown { atemp: 0x1234 });
    }

    #[test]
    fn memory_calls_set_sfc_and_go_restores_it() {
        let mut target = target(&[
            &halted(Atemp::Fresh(0)), // ResetHalt
            &value(1),                // SFC, the program's
            DONE,                     // SFC = 5
            &[0, 0xAB, 0xCD],         // ReadMemory
            &[0, 0xEF, 0x01],         // ReadMemory, SFC already 5
            DONE,                     // SFC = 1, restored
            DONE,                     // Go
        ]);
        target.reset_halted().unwrap();
        let mut data = [0; 2];
        target.read_memory(Address(0x100), FunctionCode::SUPERVISOR_DATA, Size::Word, &mut data).unwrap();
        assert_eq!(data, [0xAB, 0xCD]);
        target.read_memory(Address(0x102), FunctionCode::SUPERVISOR_DATA, Size::Word, &mut data).unwrap();
        assert_eq!(target.read_system_register(SystemRegister::Sfc).unwrap(), 1, "the program's, not 5");
        target.go().unwrap();
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        let read = |address| Request::ReadMemory { address: Address(address), size: Size::Word, length: 2 };
        assert_eq!(
            sent,
            [
                Request::ResetHalt,
                Request::ReadSystemRegister(SystemRegister::Sfc),
                Request::WriteSystemRegister(SystemRegister::Sfc, 5),
                read(0x100),
                read(0x102),
                Request::WriteSystemRegister(SystemRegister::Sfc, 1),
                Request::Go,
            ]
        );
    }

    #[test]
    fn a_written_dfc_is_the_programs_and_saves_a_write() {
        let mut target = target(&[&halted(Atemp::Fresh(0)), DONE, DONE, DONE]);
        target.reset_halted().unwrap();
        target.write_system_register(SystemRegister::Dfc, 1).unwrap();
        target.write_memory(Address(0x200), FunctionCode::USER_DATA, Size::Byte, &[7]).unwrap();
        target.go().unwrap();
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(
            sent,
            [
                Request::ResetHalt,
                Request::WriteSystemRegister(SystemRegister::Dfc, 1),
                Request::WriteMemory { address: Address(0x200), size: Size::Byte, data: &[7] },
                Request::Go,
            ]
        );
    }

    #[test]
    fn resync_keeps_the_programs_sfc_to_restore() {
        let mut target = target(&[
            &halted(Atemp::Fresh(0)), // ResetHalt
            &value(1),                // SFC, the program's
            DONE,                     // SFC = 5
            &[0, 9],                  // ReadMemory
            &halted(Atemp::Stale(0)), // Resync
            DONE,                     // SFC = 1, restored
            DONE,                     // Go
        ]);
        target.reset_halted().unwrap();
        let mut data = [0; 1];
        target.read_memory(Address(0), FunctionCode::SUPERVISOR_DATA, Size::Byte, &mut data).unwrap();
        target.resync().unwrap();
        assert_eq!(target.halted().unwrap(), Some(Entry::Unknown { atemp: 0 }));
        target.go().unwrap();
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(
            sent[sent.len() - 3..],
            [Request::Resync, Request::WriteSystemRegister(SystemRegister::Sfc, 1), Request::Go,]
        );
    }

    #[test]
    fn an_sfc_write_with_no_reply_is_restored() {
        let mut target = target(&[
            &halted(Atemp::Fresh(0)),    // ResetHalt
            &value(1),                   // SFC, the program's
            &[Status::Unexpected as u8], // SFC = 5, the reply lost
            DONE,                        // SFC = 1, restored
            DONE,                        // Go
        ]);
        target.reset_halted().unwrap();
        let mut data = [0; 1];
        assert!(
            target.read_memory(Address(0), FunctionCode::SUPERVISOR_DATA, Size::Byte, &mut data).is_err()
        );
        target.go().unwrap();
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(
            sent[sent.len() - 2..],
            [Request::WriteSystemRegister(SystemRegister::Sfc, 1), Request::Go]
        );
    }

    #[test]
    fn a_program_sfc_that_already_matches_is_left() {
        let mut target = target(&[&halted(Atemp::Fresh(0)), &value(5), &[0, 9], DONE]);
        target.reset_halted().unwrap();
        let mut data = [0; 1];
        target.read_memory(Address(0), FunctionCode::SUPERVISOR_DATA, Size::Byte, &mut data).unwrap();
        target.go().unwrap();
        assert_eq!(requests(target).len(), 4, "no SFC write, either way");
    }
}
