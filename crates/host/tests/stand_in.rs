//! `PodTarget` over a stream to the stand-in pod: the host side end to
//! end, through `mc683xx_bdm_proto::pod`'s request rules, against a model CPU.

use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use mc683xx_bdm::pod::Error;
use mc683xx_bdm::pod::Pod;
use mc683xx_bdm::stand_in::serve;
use mc683xx_bdm::step::StepError;
use mc683xx_bdm::target::PodTarget;
use mc683xx_bdm::transport::StreamTransport;
use mc683xx_bdm_model::Target;
use mc683xx_bdm_model::board::ModelBoard;
use mc683xx_bdm_proto::dsclk::DEFAULT_HZ;
use mc683xx_bdm_proto::usb::PROTOCOL_VERSION;
use mc683xx_bdm_proto::usb::ReplyError;
use mc683xx_bdm_proto::usb::Status;
use mc683xx_target::Cpu32Target;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::function_code::FunctionCode;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;

const RAM_BASE: u32 = 0x0010_0000;
const RAM: usize = 4096;

type ModelPod = mc683xx_bdm_proto::pod::Pod<ModelBoard<RAM>>;

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

/// What `bdm` with `args` prints, run on a stand-in pod on a socket
/// that serves `pod` for its one connection; it must succeed.
fn bdm(pod: &Arc<Mutex<ModelPod>>, args: &[&str]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let at = listener.local_addr().unwrap();
    let served = Arc::clone(pod);
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        serve(stream, &served).unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_bdm"))
        .arg("--pod")
        .arg(format!("tcp:{at}"))
        .args(args)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(output.status.success(), "bdm {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn bdm_clock_shows_the_rate_or_sets_it() {
    let pod = Arc::new(Mutex::new(ModelPod::new(ModelBoard::new(Target::new(Address(RAM_BASE)), true))));
    assert_eq!(bdm(&pod, &["clock"]), format!("DSCLK {DEFAULT_HZ} Hz\n"));
    assert_eq!(bdm(&pod, &["clock", "100000"]), "DSCLK 100000 Hz\n");
    assert_eq!(bdm(&pod, &["clock"]), "DSCLK 100000 Hz\n", "the pod keeps it from one bdm to the next");
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
fn blocks_go_with_requests_in_flight_and_a_failure_mid_block_leaves_no_reply_behind() {
    let (mut target, _pod) = stand_in(true);
    target.reset_halted().unwrap();
    let data: Vec<u8> = (0..RAM).map(|n| (n * 7) as u8).collect();
    target.write_memory(Address(RAM_BASE), FunctionCode::SUPERVISOR_DATA, Size::Long, &data).unwrap();
    let mut back = vec![0; RAM];
    target.read_memory(Address(RAM_BASE), FunctionCode::SUPERVISOR_DATA, Size::Long, &mut back).unwrap();
    assert_eq!(back, data);

    // Two chunks in RAM, then eight past its end: the third fails, and
    // the requests sent behind it are answered after it.
    let tail = u32::try_from(RAM - 2 * 56).unwrap();
    let mut block = vec![0; 10 * 56];
    let result =
        target.read_memory(Address(RAM_BASE + tail), FunctionCode::SUPERVISOR_DATA, Size::Long, &mut block);
    assert!(matches!(result, Err(Error::Reply(ReplyError::Failed(Status::BusError)))), "{result:?}");
    assert_eq!(block[..2 * 56], data[RAM - 2 * 56..]);
    target.write_register(Register::D3, 0x1234_5678).unwrap();
    assert_eq!(target.read_register(Register::D3).unwrap(), 0x1234_5678, "its own reply, not a stale one");
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

#[test]
fn a_reset_back_into_background_mode_is_seen() {
    let (mut target, pod) = stand_in(true);
    target.reset_halted().unwrap();
    {
        let mut pod = pod.lock().unwrap();
        let board = pod.board_mut();
        board.target().reset_running();
        board.enter_background(Entry::Breakpoint);
    }
    // FREEZE high throughout, as the host sees it; the edge count moved.
    assert!(matches!(target.read_register(Register::D0), Err(Error::Reentered)));
    assert_eq!(target.halted().unwrap(), Some(Entry::Breakpoint));
    assert!(target.read_register(Register::D0).is_ok());
}

/// SR's T1, and opcodes that write SR (CPU32RM 4): `move #imm,sr`, `ori
/// #imm,sr`, RTE.
const T1: u32 = 0x8000;
const MOVE_TO_SR: u16 = 0x46FC;
const ORI_TO_SR: u16 = 0x007C;
const RTE: u16 = 0x4E73;
/// The trace and privilege violation vectors (CPU32RM Table 6-1), and
/// where the program's handlers for them are.
const TRACE: u32 = 9;
const PRIVILEGE: u32 = 8;
const TRACE_HANDLER: u32 = RAM_BASE + 0x200;
const PRIVILEGE_HANDLER: u32 = RAM_BASE + 0x280;

/// Points the program's vector `vector` at `handler`.
fn set_vector(target: &mut StandIn, vector: u32, handler: u32) {
    let address = Address(TABLE + vector * VECTOR_BYTES);
    target.write_memory(address, FunctionCode::SUPERVISOR_DATA, Size::Long, &handler.to_be_bytes()).unwrap();
}

#[test]
fn a_stepped_move_to_sr_keeps_the_t1_it_sets() {
    let (mut target, pod) = stepping(&[MOVE_TO_SR, 0xA000, NOP]);
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR | T1);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PROGRAM + 4);
    assert_eq!(target.read_register(Register::A7).unwrap(), STACK);
    clean(&pod);
}

#[test]
fn a_stepped_move_to_sr_keeps_the_t1_it_clears() {
    let (mut target, pod) = stepping(&[MOVE_TO_SR, 0x2000, NOP]);
    set_vector(&mut target, TRACE, TRACE_HANDLER);
    target.write_system_register(SystemRegister::Sr, SUPERVISOR | T1).unwrap();
    // The program traced the instruction itself: its trace handler is
    // next, the frame holding the SR the instruction loaded.
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), TRACE_HANDLER);
    let sp = target.read_register(Register::A7).unwrap();
    assert_eq!(sp, STACK - 12, "the program's own six-word trace frame");
    assert_eq!(long(&mut target, sp) >> 16, SUPERVISOR, "T1 cleared by the instruction");
    clean(&pod);
}

#[test]
fn a_stepped_ori_to_sr_sets_t1() {
    let (mut target, pod) = stepping(&[ORI_TO_SR, 0x8000, NOP]);
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR | T1);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PROGRAM + 4);
    clean(&pod);
}

#[test]
fn a_stepped_rte_keeps_the_trace_bits_it_pops() {
    let back = PROGRAM + 0x40;
    let (mut target, pod) = stepping(&[RTE]);
    // A four-word frame: SR with T1, the return address, format $0.
    let mut frame = Vec::new();
    frame.extend_from_slice(&u16::try_from(SUPERVISOR | T1).unwrap().to_be_bytes());
    frame.extend_from_slice(&back.to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    target.write_memory(Address(STACK - 8), FunctionCode::SUPERVISOR_DATA, Size::Word, &frame).unwrap();
    target.write_register(Register::A7, STACK - 8).unwrap();
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR | T1);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), back);
    assert_eq!(target.read_register(Register::A7).unwrap(), STACK, "the frame popped");
    clean(&pod);
}

#[test]
fn a_privileged_sr_write_in_user_mode_is_a_privilege_violation() {
    let user_stack = RAM_BASE + 0x300;
    let (mut target, pod) = stepping(&[MOVE_TO_SR, 0xA000]);
    set_vector(&mut target, PRIVILEGE, PRIVILEGE_HANDLER);
    target.write_system_register(SystemRegister::Usp, user_stack).unwrap();
    target.write_system_register(SystemRegister::Sr, 0).unwrap();
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PRIVILEGE_HANDLER);
    let sp = target.read_register(Register::A7).unwrap();
    assert_eq!(long(&mut target, sp) >> 16, 0, "the program's SR, no T1: the instruction did not run");
    clean(&pod);
}

/// MOVEC (CPU32RM 4, MOVEC): from Rn to a control register and back,
/// and its extension words for D0, A0 and D1 with VBR's code and SFC's.
const MOVEC_TO_CONTROL: u16 = 0x4E7B;
const MOVEC_FROM_CONTROL: u16 = 0x4E7A;
const D0_VBR: u16 = 0x0801;
const A0_VBR: u16 = 0x8801;
const D1_VBR: u16 = 0x1801;
const D0_SFC: u16 = 0x0000;
/// A second exception table the program moves VBR to, and where its
/// TRAP #1 and trace handlers are.
const NEW_TABLE: u32 = RAM_BASE + 0xC00;
const NEW_TRAP_HANDLER: u32 = RAM_BASE + 0x140;
const NEW_TRACE_HANDLER: u32 = RAM_BASE + 0x240;
/// A format $2 frame's format in its format/vector word.
const SIX_WORD_FORMAT: u32 = 0x2000;

/// Points vector `vector` of the table at `table` at `handler`.
fn set_vector_in(target: &mut StandIn, table: u32, vector: u32, handler: u32) {
    let address = Address(table + vector * VECTOR_BYTES);
    target.write_memory(address, FunctionCode::SUPERVISOR_DATA, Size::Long, &handler.to_be_bytes()).unwrap();
}

#[test]
fn a_stepped_movec_to_vbr_moves_the_table_the_next_step_uses() {
    for (register, extension) in [(Register::D0, D0_VBR), (Register::A0, A0_VBR)] {
        let (mut target, pod) = stepping(&[MOVEC_TO_CONTROL, extension, TRAP_1_OPCODE]);
        set_vector_in(&mut target, NEW_TABLE, TRAP_1, NEW_TRAP_HANDLER);
        target.write_register(register, NEW_TABLE).unwrap();
        assert_eq!(target.step().unwrap(), Entry::Breakpoint, "{register}");
        assert_eq!(target.read_system_register(SystemRegister::Vbr).unwrap(), NEW_TABLE, "{register}");
        assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PROGRAM + 4, "{register}");
        assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR, "{register}");
        assert_eq!(target.read_register(Register::A7).unwrap(), STACK, "{register}");
        assert_eq!(target.step().unwrap(), Entry::Breakpoint, "{register}");
        assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), NEW_TRAP_HANDLER, "{register}");
        assert_eq!(target.read_register(Register::A7).unwrap(), STACK - 8, "{register}");
        assert_eq!(target.read_system_register(SystemRegister::Vbr).unwrap(), NEW_TABLE, "{register}");
        clean(&pod);
    }
}

#[test]
fn a_stepped_movec_to_vbr_traced_by_the_program_goes_to_its_new_trace_handler() {
    let (mut target, pod) = stepping(&[MOVEC_TO_CONTROL, D0_VBR, NOP]);
    set_vector_in(&mut target, NEW_TABLE, TRACE, NEW_TRACE_HANDLER);
    target.write_register(Register::D0, NEW_TABLE).unwrap();
    target.write_system_register(SystemRegister::Sr, SUPERVISOR | T1).unwrap();
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), NEW_TRACE_HANDLER);
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR);
    let sp = target.read_register(Register::A7).unwrap();
    assert_eq!(sp, STACK - 12, "the program's own six-word trace frame");
    assert_eq!(long(&mut target, sp), ((SUPERVISOR | T1) << 16) | ((PROGRAM + 4) >> 16));
    assert_eq!(long(&mut target, sp + 4), ((PROGRAM + 4) << 16) | SIX_WORD_FORMAT | (TRACE * VECTOR_BYTES));
    assert_eq!(long(&mut target, sp + 8), PROGRAM, "the traced instruction's address");
    clean(&pod);
}

#[test]
fn a_stepped_movec_from_vbr_reads_the_programs_table() {
    let (mut target, pod) = stepping(&[MOVEC_FROM_CONTROL, D1_VBR, NOP]);
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_register(Register::D1).unwrap(), TABLE, "not the step's own");
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PROGRAM + 4);
    assert_eq!(target.read_system_register(SystemRegister::Vbr).unwrap(), TABLE);
    clean(&pod);
}

#[test]
fn a_stepped_movec_in_user_mode_is_a_privilege_violation() {
    let user_stack = RAM_BASE + 0x300;
    let (mut target, pod) = stepping(&[MOVEC_TO_CONTROL, D0_VBR]);
    let handler = Address(PRIVILEGE_HANDLER);
    target.write_memory(handler, FunctionCode::SUPERVISOR_DATA, Size::Word, &NOP.to_be_bytes()).unwrap();
    set_vector(&mut target, PRIVILEGE, PRIVILEGE_HANDLER);
    target.write_register(Register::D0, NEW_TABLE).unwrap();
    target.write_system_register(SystemRegister::Usp, user_stack).unwrap();
    target.write_system_register(SystemRegister::Sr, 0).unwrap();
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PRIVILEGE_HANDLER);
    assert_eq!(target.read_system_register(SystemRegister::Vbr).unwrap(), TABLE, "not run");
    assert_eq!(target.read_system_register(SystemRegister::Sr).unwrap(), SUPERVISOR);
    let sp = target.read_register(Register::A7).unwrap();
    assert_eq!(sp, STACK - 8);
    assert_eq!(long(&mut target, sp), PROGRAM >> 16, "the program's SR, user, no T1");
    // The handler's first instruction steps as any other.
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PRIVILEGE_HANDLER + 2);
    assert_eq!(target.read_register(Register::A7).unwrap(), STACK - 8);
    clean(&pod);
}

#[test]
fn a_stepped_movec_to_sfc_is_kept_after_the_step() {
    let sfc = 3;
    let (mut target, pod) = stepping(&[MOVEC_TO_CONTROL, D0_SFC, NOP]);
    target.write_register(Register::D0, sfc).unwrap();
    assert_eq!(target.step().unwrap(), Entry::Breakpoint);
    assert_eq!(target.read_system_register(SystemRegister::Rpc).unwrap(), PROGRAM + 4);
    assert_eq!(target.read_system_register(SystemRegister::Sfc).unwrap(), sfc);
    target.restore_function_codes().unwrap();
    let in_cpu = pod.lock().unwrap().board_mut().target().system_register(SystemRegister::Sfc);
    assert_eq!(in_cpu, sfc);
    clean(&pod);
}
