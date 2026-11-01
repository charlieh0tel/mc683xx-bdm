//! [`Cpu32Target`] over the pod: the trait's halts, registers and
//! memory, with what it needs kept between requests (the entry source,
//! the program's SFC and DFC).

use mc683xx_target::Cpu32Target;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::function_code::FunctionCode;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;

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
/// It knows the entry source only if it saw the entry: by its own halt
/// or reset, or by being the first to command the CPU after one.  Made
/// for a target someone else has already commanded in background mode,
/// it reads a stale ATEMP, which decodes as [`Entry::Unknown`] or as the
/// wrong source.
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

    /// Stacks RPC and runs from `address` (CPU32RM 7.2.8.12), the
    /// program's SFC and DFC restored first.
    ///
    /// # Errors
    /// As [`Error`].
    pub fn call(&mut self, address: Address) -> Result<(), Error> {
        self.restore_function_codes()?;
        self.halted = None;
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

    /// The halted state, reading the entry source if this is the first
    /// look since the target stopped.
    fn halted_state(&mut self) -> Result<&mut Halted, Error> {
        if self.halted.is_none() {
            let atemp = self.pod.read_system_register(SystemRegister::Atemp)?;
            self.halted = Some(Halted::new(Entry::from_atemp(atemp)));
        }
        Ok(self.halted.as_mut().expect("just set"))
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
        let entry = Entry::from_atemp(self.pod.reset_halt()?);
        self.halted = Some(Halted::new(entry));
        Ok(entry)
    }

    fn halt(&mut self) -> Result<Entry, Error> {
        if let Some(entry) = self.halted()? {
            return Ok(entry);
        }
        let entry = Entry::from_atemp(self.pod.halt()?);
        self.halted = Some(Halted::new(entry));
        Ok(entry)
    }

    fn halted(&mut self) -> Result<Option<Entry>, Error> {
        if let Some(halted) = self.halted {
            return Ok(Some(halted.entry));
        }
        if !self.pod.pins()?.freeze {
            return Ok(None);
        }
        Ok(Some(self.halted_state()?.entry))
    }

    fn go(&mut self) -> Result<(), Error> {
        self.restore_function_codes()?;
        self.halted = None;
        self.pod.go()
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
    use pod_proto::usb::Request;

    use super::PodTarget;
    use crate::script::Script;
    use crate::script::parsed;
    use crate::script::pod;

    /// A reply carrying a u32.
    fn value(value: u32) -> Vec<u8> {
        std::iter::once(0).chain(value.to_le_bytes()).collect()
    }

    /// A reply with no body.
    const DONE: &[u8] = &[0];

    /// A Pins reply: FREEZE as given, RESET and DS high, 5 V.
    fn pins(freeze: bool) -> Vec<u8> {
        let levels = u8::from(freeze) | 0b110;
        [&[0, levels][..], &5000_u16.to_le_bytes()].concat()
    }

    fn target(replies: &[&[u8]]) -> PodTarget<Script> {
        PodTarget::new(pod(replies))
    }

    fn requests(target: PodTarget<Script>) -> Vec<Vec<u8>> {
        target.pod.into_transport().requests
    }

    #[test]
    fn a_halt_is_kept_until_resumed() {
        let mut target = target(&[&pins(false), &value(1), DONE, &pins(false)]);
        assert_eq!(target.halt().unwrap(), Entry::Background);
        assert_eq!(target.halt().unwrap(), Entry::Background);
        assert_eq!(target.halted().unwrap(), Some(Entry::Background));
        target.go().unwrap();
        assert_eq!(target.halted().unwrap(), None);
        let sent = requests(target);
        let sent: Vec<_> = sent.iter().map(|r| parsed(r)).collect();
        assert_eq!(sent, [Request::Pins, Request::Halt, Request::Go, Request::Pins]);
    }

    #[test]
    fn a_halt_seen_by_polling_reads_atemp_first() {
        let mut target = target(&[&pins(true), &value(0), &value(0x55)]);
        assert_eq!(target.halted().unwrap(), Some(Entry::Breakpoint));
        assert_eq!(target.read_register(Register::D0).unwrap(), 0x55);
        let sent = requests(target);
        assert_eq!(parsed(&sent[1]), Request::ReadSystemRegister(SystemRegister::Atemp));
        assert_eq!(parsed(&sent[2]), Request::ReadRegister(Register::D0));
    }

    #[test]
    fn a_register_read_after_an_unseen_halt_reads_atemp_first() {
        let mut target = target(&[&value(1), &value(0x55)]);
        assert_eq!(target.read_register(Register::D0).unwrap(), 0x55);
        assert_eq!(target.halted().unwrap(), Some(Entry::Background));
        assert_eq!(parsed(&requests(target)[0]), Request::ReadSystemRegister(SystemRegister::Atemp));
    }

    #[test]
    fn an_unlisted_atemp_is_still_a_halt() {
        let mut target = target(&[&value(0x1234)]);
        assert_eq!(target.reset_halted().unwrap(), Entry::Unknown { atemp: 0x1234 });
    }

    #[test]
    fn memory_calls_set_sfc_and_go_restores_it() {
        let mut target = target(&[
            &value(0),        // ResetHalt: ATEMP
            &value(1),        // SFC, the program's
            DONE,             // SFC = 5
            &[0, 0xAB, 0xCD], // ReadMemory
            &[0, 0xEF, 0x01], // ReadMemory, SFC already 5
            DONE,             // SFC = 1, restored
            DONE,             // Go
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
        let mut target = target(&[&value(0), DONE, DONE, DONE]);
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
        let mut target = target(&[&value(0), &value(5), &[0, 9], DONE]);
        target.reset_halted().unwrap();
        let mut data = [0; 1];
        target.read_memory(Address(0), FunctionCode::SUPERVISOR_DATA, Size::Byte, &mut data).unwrap();
        target.go().unwrap();
        assert_eq!(requests(target).len(), 4, "no SFC write, either way");
    }
}
