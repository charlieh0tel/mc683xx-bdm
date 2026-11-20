//! [`Cpu32Target`] over the pod: the trait's halts, registers and
//! memory, with what it needs kept between requests (the entry source,
//! the program's SFC and DFC).  The pod reads ATEMP first after each
//! entry and reports it (`pod_proto::entry`); this never reads ATEMP.

use mc683xx_target::Cpu32Target;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::function_code::FunctionCode;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;
use pod_proto::usb::Atemp;
use pod_proto::usb::ReplyError;
use pod_proto::usb::Status;

use crate::pod::Error;
use crate::pod::Pod;
use crate::transport::Transport;

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
}

impl<T: Transport> PodTarget<T> {
    /// The target behind `pod`.
    pub const fn new(pod: Pod<T>) -> Self {
        Self { pod, halted: None }
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
        self.pod.call(address)
    }

    /// Puts back the program's SFC and DFC where a memory call changed
    /// them, without resuming: for a caller that stops using the target
    /// halted, such as a command-line tool about to exit.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn restore_function_codes(&mut self) -> Result<(), Error> {
        let Some(halted) = self.halted.as_mut() else { return Ok(()) };
        for register in [SystemRegister::Sfc, SystemRegister::Dfc] {
            let state = halted.register(register).expect("SFC and DFC are kept");
            if let Some(program) = state.program
                && state.current != Some(program)
            {
                self.pod.write_system_register(register, program)?;
                state.current = Some(program);
            }
        }
        Ok(())
    }

    /// Resets the target to run, background mode off; forgets the
    /// halted state.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn reset_running(&mut self) -> Result<(), Error> {
        self.halted = None;
        self.pod.reset_run()
    }

    /// Brings the halted target's serial link back into frame;
    /// forgets the halted state, whose entry source is now unknown.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn resync(&mut self) -> Result<Atemp, Error> {
        self.halted = None;
        self.pod.resync()
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

    /// Sets SFC or DFC to `function_code` for a memory call, reading the
    /// program's value first so it can be restored.
    fn select(&mut self, register: SystemRegister, function_code: FunctionCode) -> Result<(), Error> {
        let wanted = u32::from(function_code.bits());
        let pod = &mut self.pod;
        let state = function_code_register(&mut self.halted, register)
            .expect("a memory call runs halted, on SFC or DFC");
        if state.current == Some(wanted) {
            return Ok(());
        }
        if state.program.is_none() {
            let value = pod.read_system_register(register)?;
            state.program = Some(value);
            state.current = Some(value);
            if FunctionCode::new(value) == function_code {
                return Ok(());
            }
        }
        pod.write_system_register(register, wanted)?;
        state.current = Some(wanted);
        Ok(())
    }
}

impl<T: Transport> Cpu32Target for PodTarget<T> {
    type Error = Error;

    fn reset_halted(&mut self) -> Result<Entry, Error> {
        self.halted = None;
        let atemp = self.pod.reset_halt()?;
        self.halted_at(atemp)
    }

    fn halt(&mut self) -> Result<Entry, Error> {
        if let Some(halted) = self.halted {
            return Ok(halted.entry);
        }
        let atemp = self.pod.halt()?;
        self.halted_at(atemp)
    }

    fn halted(&mut self) -> Result<Option<Entry>, Error> {
        self.look()
    }

    fn go(&mut self) -> Result<(), Error> {
        self.leave()?;
        self.pod.go()
    }

    /// Not yet: T1 tracing through a stub in scratch RAM (PLAN.md,
    /// Host, gdb).
    fn step(&mut self) -> Result<Entry, Error> {
        Err(Error::Unsupported("step"))
    }

    fn read_register(&mut self, register: Register) -> Result<u32, Error> {
        self.halted_state()?;
        self.pod.read_register(register)
    }

    fn write_register(&mut self, register: Register, value: u32) -> Result<(), Error> {
        self.halted_state()?;
        self.pod.write_register(register, value)
    }

    fn read_system_register(&mut self, register: SystemRegister) -> Result<u32, Error> {
        let halted = self.halted_state()?;
        if let Some(program) = halted.register(register).and_then(|state| state.program) {
            return Ok(program);
        }
        let value = self.pod.read_system_register(register)?;
        if let Some(state) = function_code_register(&mut self.halted, register) {
            state.program = Some(value);
            state.current = Some(value);
        }
        Ok(value)
    }

    fn write_system_register(&mut self, register: SystemRegister, value: u32) -> Result<(), Error> {
        self.halted_state()?;
        self.pod.write_system_register(register, value)?;
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
        self.pod.read_memory(address, size, data)
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
        self.pod.write_memory(address, size, data)
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
    use mc683xx_target::Cpu32Target;
    use mc683xx_target::address::Address;
    use mc683xx_target::entry::Entry;
    use mc683xx_target::function_code::FunctionCode;
    use mc683xx_target::register::Register;
    use mc683xx_target::register::SystemRegister;
    use mc683xx_target::size::Size;
    use pod_proto::usb::Atemp;
    use pod_proto::usb::PACKET;
    use pod_proto::usb::Pins;
    use pod_proto::usb::ReplyError;
    use pod_proto::usb::Request;
    use pod_proto::usb::Status;
    use pod_proto::usb::encode_atemp;
    use pod_proto::usb::encode_pins;

    use super::PodTarget;
    use crate::pod::Error;
    use crate::script::Script;
    use crate::script::parsed;
    use crate::script::pod;

    /// A reply carrying a u32.
    fn value(value: u32) -> Vec<u8> {
        std::iter::once(0).chain(value.to_le_bytes()).collect()
    }

    /// A reply with no body.
    const DONE: &[u8] = &[0];

    /// A Pins reply: RESET and DS high, 5 V, FREEZE high unless `atemp`
    /// is `Atemp::None`.
    fn pins(atemp: Atemp) -> Vec<u8> {
        let mut packet = [0; PACKET];
        let pins =
            Pins { freeze: atemp != Atemp::None, reset: true, ds: true, berr: true, vdd_mv: 5000, atemp };
        let length = encode_pins(&pins, &mut packet);
        packet[..length].to_vec()
    }

    /// A `Halt` or `ResetHalt` reply.
    fn halted(atemp: Atemp) -> Vec<u8> {
        let mut packet = [0; PACKET];
        let length = encode_atemp(atemp, &mut packet);
        packet[..length].to_vec()
    }

    fn target(replies: &[&[u8]]) -> PodTarget<Script> {
        PodTarget::new(pod(replies))
    }

    fn requests(target: PodTarget<Script>) -> Vec<Vec<u8>> {
        target.pod.into_transport().requests
    }

    #[test]
    fn a_halt_is_kept_until_resumed() {
        let mut target = target(&[&halted(Atemp::Fresh(1)), DONE, &pins(Atemp::None)]);
        assert_eq!(target.halt().unwrap(), Entry::Background);
        assert_eq!(target.halt().unwrap(), Entry::Background);
        assert_eq!(target.halted().unwrap(), Some(Entry::Background));
        target.go().unwrap();
        assert_eq!(target.halted().unwrap(), None);
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(sent, [Request::Halt, Request::Go, Request::Pins]);
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
    fn a_program_sfc_that_already_matches_is_left() {
        let mut target = target(&[&halted(Atemp::Fresh(0)), &value(5), &[0, 9], DONE]);
        target.reset_halted().unwrap();
        let mut data = [0; 1];
        target.read_memory(Address(0), FunctionCode::SUPERVISOR_DATA, Size::Byte, &mut data).unwrap();
        target.go().unwrap();
        assert_eq!(requests(target).len(), 4, "no SFC write, either way");
    }
}
