//! `PodTarget` over a stream to the stand-in pod: the host side end to
//! end, through `pod_proto::pod`'s request rules, against a model CPU.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use bdm_model::Target;
use bdm_model::board::ModelBoard;
use mc683xx_target::Cpu32Target;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::function_code::FunctionCode;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;
use pod_host::pod::Error;
use pod_host::pod::Pod;
use pod_host::stand_in::serve;
use pod_host::step::StepError;
use pod_host::target::PodTarget;
use pod_host::transport::StreamTransport;
use pod_proto::usb::PROTOCOL_VERSION;
use pod_proto::usb::ReplyError;
use pod_proto::usb::Status;

const RAM_BASE: u32 = 0x0010_0000;
const RAM: usize = 4096;

type ModelPod = pod_proto::pod::Pod<ModelBoard<RAM>>;

/// A target on a stand-in pod in a thread, and the stand-in's pod, to
/// act on the model between requests.
fn stand_in(bdm_enabled: bool) -> (PodTarget<StreamTransport<UnixStream>>, Arc<Mutex<ModelPod>>) {
    let (host, pod_side) = UnixStream::pair().unwrap();
    let model = Target::new(Address(RAM_BASE));
    let pod = Arc::new(Mutex::new(ModelPod::new(ModelBoard::new(model, bdm_enabled))));
    let served = Arc::clone(&pod);
    thread::spawn(move || serve(pod_side, &served).unwrap());
    (PodTarget::new(Pod::new(StreamTransport::new(host))), pod)
}

#[test]
fn a_session() {
    let (mut target, pod) = stand_in(true);
    assert_eq!(target.pod().hello().unwrap(), PROTOCOL_VERSION);
    assert_eq!(target.reset_halted().unwrap(), Entry::Breakpoint);

    target.write_register(Register::D3, 0xDEAD_BEEF).unwrap();
    assert_eq!(target.read_register(Register::D3).unwrap(), 0xDEAD_BEEF);

    target.write_system_register(SystemRegister::Sfc, 1).unwrap();
    let data: Vec<u8> = (0..100).collect();
    target.write_memory(Address(RAM_BASE), FunctionCode::SUPERVISOR_DATA, Size::Word, &data).unwrap();
    let mut back = vec![0; 100];
    target.read_memory(Address(RAM_BASE), FunctionCode::SUPERVISOR_DATA, Size::Long, &mut back).unwrap();
    assert_eq!(back, data);
    assert_eq!(target.read_system_register(SystemRegister::Sfc).unwrap(), 1, "the program's");

    target.go().unwrap();
    assert_eq!(target.halted().unwrap(), None);
    {
        let mut pod = pod.lock().unwrap();
        let board = pod.board_mut();
        assert_eq!(board.target().system_register(SystemRegister::Sfc), 1, "restored before GO");
        board.enter_background(Entry::Background);
    }
    assert_eq!(target.halted().unwrap(), Some(Entry::Background));
    assert_eq!(target.halt().unwrap(), Entry::Background);
    let (violation, _) = pod.lock().unwrap().board_mut().target().violations();
    assert_eq!(violation, None);
}

#[test]
fn halt_without_bdm_enabled_fails() {
    let (mut target, _pod) = stand_in(false);
    assert!(target.halt().is_err());
    assert_eq!(target.reset_halted().unwrap(), Entry::Breakpoint);
}

#[test]
fn a_call_that_cannot_stack_fails_and_stays_halted() {
    let (mut target, pod) = stand_in(true);
    target.reset_halted().unwrap();
    target.write_register(Register::A7, 0x00FF_0000).unwrap();
    let error = target.call(Address(RAM_BASE)).unwrap_err();
    assert!(matches!(error, Error::Reply(ReplyError::Failed(Status::BusError))), "{error}");
    assert_eq!(target.halted().unwrap(), Some(Entry::Breakpoint));
    assert_eq!(target.read_register(Register::A7).unwrap(), 0x00FF_0000);
    clean(&pod);
}

/// Where a stepped program runs, its exception table, the step's
/// scratch RAM, and the top of its supervisor stack.
const PROGRAM: u32 = RAM_BASE;
const TABLE: u32 = RAM_BASE + 0x400;
const SCRATCH: u32 = RAM_BASE + 0x800;
const STACK: u32 = RAM_BASE + 0x1000;
/// Where the program's TRAP #1 and interrupt handlers are.
const TRAP_HANDLER: u32 = RAM_BASE + 0x100;
const INTERRUPT_HANDLER: u32 = RAM_BASE + 0x180;
/// Vectors (CPU32RM Table 6-1): TRAP #1, and a device's.
const TRAP_1: u32 = 33;
const DEVICE: u8 = 64;
/// Bytes in a vector.
const VECTOR_BYTES: u32 = 4;
/// SR: supervisor, interrupts masked to level 0; the mask's place;
/// and N.
const SUPERVISOR: u32 = 0x2000;
const MASK_SHIFT: u32 = 8;
const NEGATIVE: u32 = 0x8;
/// Opcodes (CPU32RM 4): NOP, BGND, TRAP #1, `moveq #5,d0`, `moveq
/// #-1,d1`, and `ori.b #0,d0`, which the model does not run.
const NOP: u16 = 0x4E71;
const BGND: u16 = 0x4AFA;
const TRAP_1_OPCODE: u16 = 0x4E41;
const MOVEQ_5_D0: u16 = 0x7005;
const MOVEQ_MINUS_1_D1: u16 = 0x72FF;
const ORI_B: u16 = 0x0000;

type StandIn = PodTarget<StreamTransport<UnixStream>>;

/// A target on a stand-in, reset into background mode, running
/// `program` from [`PROGRAM`] in supervisor mode, with its table at
/// [`TABLE`] and the step's scratch at [`SCRATCH`].
fn stepping(program: &[u16]) -> (StandIn, Arc<Mutex<ModelPod>>) {
    let (mut target, pod) = stand_in(true);
    target.set_scratch(Some(Address(SCRATCH)));
    target.reset_halted().unwrap();
    let code: Vec<u8> = program.iter().flat_map(|word| word.to_be_bytes()).collect();
    target.write_memory(Address(PROGRAM), FunctionCode::SUPERVISOR_DATA, Size::Word, &code).unwrap();
    for (vector, handler) in [(TRAP_1, TRAP_HANDLER), (u32::from(DEVICE), INTERRUPT_HANDLER)] {
        let address = Address(TABLE + vector * VECTOR_BYTES);
        target
            .write_memory(address, FunctionCode::SUPERVISOR_DATA, Size::Long, &handler.to_be_bytes())
            .unwrap();
    }
    target.write_system_register(SystemRegister::Sr, SUPERVISOR).unwrap();
    target.write_register(Register::A7, STACK).unwrap();
    target.write_system_register(SystemRegister::Vbr, TABLE).unwrap();
    target.write_system_register(SystemRegister::Rpc, PROGRAM).unwrap();
    (target, pod)
}

/// The long at `address`.
fn long(target: &mut StandIn, address: u32) -> u32 {
    let mut data = [0; 4];
    target.read_memory(Address(address), FunctionCode::SUPERVISOR_DATA, Size::Word, &mut data).unwrap();
    u32::from_be_bytes(data)
}

/// Checks the model saw nothing a real CPU32 would not take.
fn clean(pod: &Mutex<ModelPod>) {
    assert_eq!(pod.lock().unwrap().board_mut().target().violations(), (None, 0));
}

#[test]
fn a_step_runs_one_instruction() {
    let (mut target, pod) = stepping(&[MOVEQ_5_D0, MOVEQ_MINUS_1_D1]);
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PROGRAM + 2);
    assert_eq!(target.read_register(Register::D0).unwrap(), 5);
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR, "T1 gone");
    assert_eq!(target.read_register(Register::A7).unwrap(), STACK, "the trace frame gone");
    assert_eq!(target.read_system_register(SystemRegister::Vbr).unwrap(), TABLE);
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PROGRAM + 4);
    assert_eq!(target.read_register(Register::D1).unwrap(), 0xFFFF_FFFF);
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR | NEGATIVE);
    clean(&pod);
}

#[test]
fn a_step_in_user_mode_comes_back_to_user_mode() {
    let user_stack = RAM_BASE + 0x300;
    let (mut target, pod) = stepping(&[NOP]);
    target.write_system_register(SystemRegister::Usp, user_stack).unwrap();
    target.write_system_register(SystemRegister::Sr, 0).unwrap();
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), 0);
    assert_eq!(target.read_register(Register::A7).unwrap(), user_stack);
    assert_eq!(target.read_system_register(SystemRegister::Ssp).unwrap(), STACK);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PROGRAM + 2);
    clean(&pod);
}

#[test]
fn a_stepped_trap_stops_at_its_handler() {
    let (mut target, pod) = stepping(&[TRAP_1_OPCODE]);
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), TRAP_HANDLER);
    let sp = target.read_register(Register::A7).unwrap();
    assert_eq!(sp, STACK - 8, "the trap's four-word frame alone");
    assert_eq!(long(&mut target, sp), (SUPERVISOR << 16) | ((PROGRAM + 2) >> 16));
    assert_eq!(long(&mut target, sp + 4), ((PROGRAM + 2) << 16) | (TRAP_1 * VECTOR_BYTES));
    assert_eq!(target.read_system_register(SystemRegister::Vbr).unwrap(), TABLE);
    clean(&pod);
}

#[test]
fn an_interrupt_pending_is_taken_by_a_step() {
    let level = 3;
    let (mut target, pod) = stepping(&[MOVEQ_5_D0]);
    pod.lock().unwrap().board_mut().target().request_interrupt(level, DEVICE);
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), INTERRUPT_HANDLER);
    let sr = SUPERVISOR | (u32::from(level) << MASK_SHIFT);
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), sr);
    assert_eq!(target.read_register(Register::D0).unwrap(), 0, "the instruction not run");
    let sp = target.read_register(Register::A7).unwrap();
    assert_eq!(sp, STACK - 8);
    assert_eq!(long(&mut target, sp), (SUPERVISOR << 16) | (PROGRAM >> 16), "the program's SR, untraced");
    clean(&pod);
}

#[test]
fn a_stepped_bgnd_is_a_background_entry() {
    let (mut target, pod) = stepping(&[BGND]);
    assert_eq!(target.step().unwrap(), Entry::Background);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PROGRAM + 2);
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR);
    assert_eq!(target.read_system_register(SystemRegister::Vbr).unwrap(), TABLE);
    clean(&pod);
}

#[test]
fn a_step_that_does_not_stop_is_halted() {
    let (mut target, pod) = stepping(&[ORI_B, 0]);
    let error = target.step().unwrap_err();
    assert!(matches!(error, Error::Step(StepError::NotStopped(Address(PROGRAM)))), "{error}");
    assert_eq!(target.halted().unwrap(), Some(Entry::Breakpoint));
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR);
    assert_eq!(target.read_system_register(SystemRegister::Vbr).unwrap(), TABLE);
    clean(&pod);
}

#[test]
fn a_step_needs_scratch_ram() {
    let (mut target, _pod) = stepping(&[NOP]);
    target.set_scratch(None);
    assert!(matches!(target.step(), Err(Error::Step(StepError::NoScratch))));
    target.set_scratch(Some(Address(SCRATCH + 1)));
    assert!(matches!(target.step(), Err(Error::Step(StepError::OddScratch(_)))));
}
