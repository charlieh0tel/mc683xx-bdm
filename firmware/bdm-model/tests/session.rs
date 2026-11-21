//! `pod_proto::session` against the model: every command sequence the
//! pod uses, run against a CPU32 that answers as CPU32RM 7.2.8 says and
//! records what a real one would not take.

use bdm_model::Target;
use bdm_model::Violation;
use embassy_futures::block_on;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;
use pod_proto::bdm::Command;
use pod_proto::session::Error;
use pod_proto::session::Link;
use pod_proto::session::Session;

/// The model's RAM: where, and how much.
const RAM_BASE: u32 = 0x0010_0000;
const RAM: usize = 256;
const RETRIES: u32 = 8;

type Model = Target<RAM>;

/// A model stopped by BKPT, in a session.
fn halted() -> Session<Model> {
    let mut target = Model::new(Address(RAM_BASE));
    target.enter_background(Entry::Breakpoint);
    Session::new(target, RETRIES)
}

/// Checks the model saw nothing a real CPU32 would not take.
fn clean(session: &mut Session<Model>) {
    assert_eq!(session.link_mut().violations(), (None, 0));
}

#[test]
fn atemp_read_first_gives_the_entry_source() {
    for (entry, atemp) in [
        (Entry::Breakpoint, 0x0000_0000),
        (Entry::Background, 0x0000_0001),
        (Entry::DoubleBusFault { ssw: 0x1234 }, 0x1234_FFFF),
    ] {
        let mut target = Model::new(Address(RAM_BASE));
        target.enter_background(entry);
        let mut session = Session::new(target, RETRIES);
        assert_eq!(block_on(session.read_system_register(SystemRegister::Atemp)), Ok(atemp));
        clean(&mut session);
    }
}

#[test]
fn atemp_is_scratch_after_another_command() {
    let mut session = halted();
    block_on(session.execute(&Command::WriteRegister(Register::D0, 1))).unwrap();
    assert_ne!(block_on(session.read_system_register(SystemRegister::Atemp)), Ok(0));
    clean(&mut session);
}

#[test]
fn every_register_round_trips() {
    let mut session = halted();
    for (n, register) in Register::ALL.into_iter().enumerate() {
        let value = 0x1111_1111_u32.wrapping_mul(u32::try_from(n + 1).unwrap()) ^ 0x8000_0001;
        block_on(session.execute(&Command::WriteRegister(register, value))).unwrap();
        assert_eq!(session.link_mut().register(register), value, "{register:?}");
        assert_eq!(block_on(session.read_register(register)), Ok(value), "{register:?}");
    }
    clean(&mut session);
}

#[test]
fn system_registers_round_trip() {
    let mut session = halted();
    session.link_mut().set_system_register(SystemRegister::Sr, 0x2700);
    assert_eq!(block_on(session.read_system_register(SystemRegister::Sr)), Ok(0x2700));
    block_on(session.execute(&Command::WriteSystemRegister(SystemRegister::Sfc, 5))).unwrap();
    assert_eq!(session.link_mut().system_register(SystemRegister::Sfc), 5);
    assert_eq!(block_on(session.read_system_register(SystemRegister::Sfc)), Ok(5));
    clean(&mut session);
}

#[test]
fn memory_round_trips_in_every_size() {
    for busy in [0, 3] {
        for size in [Size::Byte, Size::Word, Size::Long] {
            let mut session = halted();
            session.link_mut().memory_busy = busy;
            let data: [u8; 16] =
                core::array::from_fn(|n| u8::try_from(n).unwrap().wrapping_mul(17).wrapping_add(1));
            let address = Address(RAM_BASE + 0x20);
            block_on(session.write_block(address, size, &data)).unwrap();
            assert_eq!(&session.link_mut().ram_mut()[0x20..0x30], &data, "{size:?} busy {busy}");
            let mut back = [0; 16];
            block_on(session.read_block(address, size, &mut back)).unwrap();
            assert_eq!(back, data, "{size:?} busy {busy}");
            clean(&mut session);
        }
    }
}

#[test]
fn memory_is_big_endian() {
    let mut session = halted();
    block_on(session.execute(&Command::Write(Size::Long, Address(RAM_BASE), 0x0102_0304))).unwrap();
    assert_eq!(&session.link_mut().ram_mut()[..4], &[1, 2, 3, 4]);
    assert_eq!(block_on(session.execute(&Command::Read(Size::Word, Address(RAM_BASE + 2)))), Ok(0x0304));
    assert_eq!(block_on(session.execute(&Command::Read(Size::Byte, Address(RAM_BASE + 1)))), Ok(0x02));
    clean(&mut session);
}

#[test]
fn bus_error_then_the_next_command_works() {
    let mut session = halted();
    assert_eq!(
        block_on(session.execute(&Command::Read(Size::Word, Address(0x00FF_0000)))),
        Err(Error::BusError)
    );
    // An odd word: an address error, which BDM reports the same way.
    assert_eq!(
        block_on(session.execute(&Command::Write(Size::Word, Address(RAM_BASE + 1), 0))),
        Err(Error::BusError)
    );
    block_on(session.execute(&Command::WriteRegister(Register::D2, 42))).unwrap();
    assert_eq!(block_on(session.read_register(Register::D2)), Ok(42));
    clean(&mut session);
}

#[test]
fn block_past_the_end_stops_at_the_bus_error() {
    let mut session = halted();
    let mut data = [0; 8];
    let start = Address(RAM_BASE + u32::try_from(RAM).unwrap() - 4);
    assert_eq!(block_on(session.read_block(start, Size::Long, &mut data)), Err(Error::BusError));
    clean(&mut session);
}

#[test]
fn not_ready_past_the_retries() {
    let mut session = halted();
    session.link_mut().memory_busy = RETRIES + 2;
    assert_eq!(
        block_on(session.execute(&Command::Read(Size::Byte, Address(RAM_BASE)))),
        Err(Error::NotReady(RETRIES))
    );
    clean(&mut session);
}

#[test]
fn illegal_command_then_the_next_command_works() {
    let mut session = halted();
    // A RSREG select code with no register: illegal.
    let link = session.link_mut();
    block_on(link.transfer(0x2582)).unwrap();
    assert_eq!(block_on(link.transfer(0x0000)), Ok(0x1_FFFF));
    assert_eq!(block_on(session.execute(&Command::Nop)), Ok(0));
    clean(&mut session);
}

#[test]
fn go_leaves_background_mode_with_dsi_released() {
    let mut session = halted();
    block_on(session.execute(&Command::Go)).unwrap();
    assert!(!session.link_mut().frozen());
    clean(&mut session);
    // Any clock now is a BKPT pulse.
    block_on(session.execute(&Command::Nop)).ok();
    assert_eq!(session.link_mut().violations().0, Some(Violation::ClockedWhileRunning(0x0000)));
}

#[test]
fn go_without_the_release_is_caught() {
    let mut session = halted();
    block_on(session.link_mut().transfer(0x0C00)).unwrap();
    assert_eq!(session.link_mut().violations().0, Some(Violation::ExitNotSignaled(0x0C00)));
}

#[test]
fn release_on_a_word_that_stays_is_caught() {
    let mut session = halted();
    block_on(session.link_mut().transfer_exit(0x0000)).unwrap();
    assert_eq!(session.link_mut().violations().0, Some(Violation::ExitSignaledEarly(0x0000)));
}

#[test]
fn call_stacks_rpc_and_runs() {
    let mut session = halted();
    let stack = RAM_BASE + 0x80;
    session.link_mut().set_register(Register::A7, stack);
    session.link_mut().set_system_register(SystemRegister::Rpc, 0x0004_1234);
    block_on(session.execute(&Command::Call(Address(0x0004_8000)))).unwrap();
    let target = session.link_mut();
    assert!(!target.frozen());
    assert_eq!(target.register(Register::A7), stack - 4);
    assert_eq!(target.system_register(SystemRegister::Rpc), 0x0004_8000);
    assert_eq!(&target.ram_mut()[0x7C..0x80], &[0x00, 0x04, 0x12, 0x34]);
    clean(&mut session);
}

#[test]
fn call_that_cannot_stack_stays_in_background_mode() {
    let mut session = halted();
    session.link_mut().set_register(Register::A7, 0x00FF_0000);
    session.link_mut().set_system_register(SystemRegister::Rpc, 0x0004_1234);
    // The session clocks nothing after CALL; FREEZE stays asserted, and
    // the release was not a violation.
    assert_eq!(block_on(session.execute(&Command::Call(Address(0x0004_8000)))), Ok(0));
    assert!(session.link_mut().frozen());
    // The next word collects the bus error, and the CPU takes commands.
    assert_eq!(block_on(session.exit_refused()), Error::BusError);
    assert_eq!(block_on(session.read_register(Register::A7)), Ok(0x00FF_0000));
    assert_eq!(block_on(session.read_system_register(SystemRegister::Rpc)), Ok(0x0004_1234));
    clean(&mut session);
}

#[test]
fn dump_without_read_is_caught() {
    let mut session = halted();
    block_on(session.execute(&Command::Dump(Size::Word))).ok();
    assert_eq!(session.link_mut().violations().0, Some(Violation::DumpWithoutRead));
}

#[test]
fn reset_peripherals_completes() {
    let mut session = halted();
    assert_eq!(block_on(session.execute(&Command::ResetPeripherals)), Ok(0));
    assert_eq!(session.link_mut().peripheral_resets(), 1);
    assert!(session.link_mut().frozen());
    clean(&mut session);
}

#[test]
fn clocking_a_running_cpu_is_caught() {
    let mut session = Session::new(Model::new(Address(RAM_BASE)), RETRIES);
    block_on(session.link_mut().transfer(0x0000)).unwrap();
    assert_eq!(session.link_mut().violations().0, Some(Violation::ClockedWhileRunning(0x0000)));
}

#[test]
fn a7_is_the_stack_pointer_sr_selects() {
    let supervisor = 0x2000;
    let (ssp, usp) = (RAM_BASE + 0x80, RAM_BASE + 0x40);
    let mut target = Model::new(Address(RAM_BASE));
    target.set_system_register(SystemRegister::Sr, supervisor);
    target.set_register(Register::A7, ssp);
    target.set_system_register(SystemRegister::Usp, usp);
    assert_eq!(target.system_register(SystemRegister::Ssp), ssp);
    target.set_system_register(SystemRegister::Sr, 0);
    assert_eq!(target.register(Register::A7), usp);
    assert_eq!(target.system_register(SystemRegister::Ssp), ssp);
    target.set_system_register(SystemRegister::Sr, supervisor);
    assert_eq!(target.register(Register::A7), ssp);
}
