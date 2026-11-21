//! `mc683xx_bdm_proto::pod` against the model board: the pod's answer to each
//! request (docs/usb-protocol.md), from the bytes in to the bytes out.

use embassy_futures::block_on;
use mc683xx_bdm_model::Target;
use mc683xx_bdm_model::board::ModelBoard;
use mc683xx_bdm_proto::bdm::Command;
use mc683xx_bdm_proto::pod::Pod;
use mc683xx_bdm_proto::usb::Atemp;
use mc683xx_bdm_proto::usb::HELLO_REQUEST;
use mc683xx_bdm_proto::usb::PACKET;
use mc683xx_bdm_proto::usb::PROTOCOL_VERSION;
use mc683xx_bdm_proto::usb::ReplyError;
use mc683xx_bdm_proto::usb::ReplyHeader;
use mc683xx_bdm_proto::usb::Request;
use mc683xx_bdm_proto::usb::Status;
use mc683xx_bdm_proto::usb::parse_atemp;
use mc683xx_bdm_proto::usb::parse_hello_reply;
use mc683xx_bdm_proto::usb::parse_pins;
use mc683xx_bdm_proto::usb::parse_reply;
use mc683xx_bdm_proto::usb::parse_reply_header;
use mc683xx_bdm_proto::usb::parse_value;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::register::Register;
use mc683xx_target::size::Size;

/// The model's RAM: where, and how much.
const RAM_BASE: u32 = 0x0010_0000;
const RAM: usize = 256;

type ModelPod = Pod<ModelBoard<RAM>>;

/// The Transfer request's opcode (docs/usb-protocol.md).
const TRANSFER: u8 = 0x01;
/// The tag every request here carries.
const TAG: u8 = 0x42;

/// A pod on a running model, its BDM enabled or not.
fn pod(bdm_enabled: bool) -> ModelPod {
    Pod::new(ModelBoard::new(Target::new(Address(RAM_BASE)), bdm_enabled))
}

/// A pod on a model halted before the pod started: its ATEMP is stale.
fn pod_on_halted() -> ModelPod {
    let mut target = Target::new(Address(RAM_BASE));
    target.enter_background(Entry::Background);
    Pod::new(ModelBoard::new(target, true))
}

/// Sends `request`, tagged [`TAG`]; returns the reply's bytes.
fn send(pod: &mut ModelPod, request: &Request<'_>) -> Vec<u8> {
    let mut packet = [0; PACKET];
    let length = request.encode(TAG, &mut packet).unwrap();
    raw(pod, &packet[..length])
}

fn raw(pod: &mut ModelPod, packet: &[u8]) -> Vec<u8> {
    let mut reply = [0; PACKET];
    let length = block_on(pod.handle(packet, &mut reply));
    reply[..length].to_vec()
}

/// A reply's header.
fn header(reply: &[u8]) -> ReplyHeader {
    parse_reply_header(reply).unwrap().0
}

/// A reply's result, its tag checked as [`TAG`].
fn status(reply: &[u8]) -> Result<&[u8], ReplyError> {
    let (header, message) = parse_reply_header(reply).unwrap();
    assert_eq!(header.tag, TAG, "the tag echoed");
    parse_reply(message)
}

fn failed(status: Status) -> Result<&'static [u8], ReplyError> {
    Err(ReplyError::Failed(status))
}

#[test]
fn hello() {
    let mut pod = pod(true);
    assert_eq!(parse_hello_reply(&raw(&mut pod, &HELLO_REQUEST)), Some(PROTOCOL_VERSION));
}

#[test]
fn bad_requests_echo_the_tag() {
    let mut pod = pod(true);
    for packet in [&[TAG][..], &[TAG, 0xFF], &[TAG, 0x02, 0], &[TAG, 0x0B, 0, 0, 0, 0, 3, 4], &[TAG, 0x0E]] {
        assert_eq!(status(&raw(&mut pod, packet)), failed(Status::BadRequest), "{packet:?}");
    }
    // No tag to echo: tagged 0, which no host uses.
    assert_eq!(raw(&mut pod, &[]), [0, 0, Status::BadRequest as u8]);
    let mut reply = [0; PACKET];
    let length = pod.oversized(&mut reply);
    assert_eq!(reply[..length], [0, 0, Status::BadRequest as u8]);
}

#[test]
fn replies_carry_freeze_edges() {
    let mut pod = pod(true);
    assert_eq!(header(&send(&mut pod, &Request::Pins)), ReplyHeader { tag: TAG, freeze_edges: 0 });
    send(&mut pod, &Request::ResetHalt);
    let entered = header(&send(&mut pod, &Request::Pins)).freeze_edges;
    assert_eq!(entered, 1);
    // A reset to run and the CPU's own entry: left, and back.
    send(&mut pod, &Request::ResetRun);
    pod.board_mut().enter_background(Entry::Background);
    assert_eq!(header(&send(&mut pod, &Request::Pins)).freeze_edges, entered + 2);
}

#[test]
fn commands_need_freeze() {
    let mut pod = pod(true);
    for request in [
        Request::Go,
        Request::ReadRegister(Register::D0),
        Request::ReadMemory { address: Address(RAM_BASE), size: Size::Byte, length: 1 },
    ] {
        assert_eq!(status(&send(&mut pod, &request)), failed(Status::NoFreeze), "{request:?}");
    }
    let pins = parse_pins(status(&send(&mut pod, &Request::Pins)).unwrap()).unwrap();
    assert!(!pins.freeze);
    assert_eq!(pins.atemp, Atemp::None);
}

#[test]
fn halt_without_bdm_enabled_is_no_freeze() {
    let mut pod = pod(false);
    assert_eq!(status(&send(&mut pod, &Request::Halt)), failed(Status::NoFreeze));
}

#[test]
fn halt_reads_atemp_once_and_keeps_it() {
    let mut pod = pod(true);
    assert_eq!(parse_atemp(status(&send(&mut pod, &Request::Halt)).unwrap()), Ok(Atemp::Fresh(0)));
    // Commands overwrite ATEMP in the CPU; the pod keeps what it read.
    send(&mut pod, &Request::WriteRegister(Register::D1, 0x1234));
    assert_eq!(parse_atemp(status(&send(&mut pod, &Request::Halt)).unwrap()), Ok(Atemp::Fresh(0)));
    let pins = parse_pins(status(&send(&mut pod, &Request::Pins)).unwrap()).unwrap();
    assert_eq!((pins.freeze, pins.atemp, pins.vdd_mv), (true, Atemp::Fresh(0), 5000));
}

#[test]
fn a_self_entry_after_go_is_read_first_and_fresh() {
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetHalt);
    assert_eq!(status(&send(&mut pod, &Request::Go)), Ok(&[][..]));
    assert!(!parse_pins(status(&send(&mut pod, &Request::Pins)).unwrap()).unwrap().freeze);
    pod.board_mut().enter_background(Entry::Background);
    let pins = parse_pins(status(&send(&mut pod, &Request::Pins)).unwrap()).unwrap();
    assert_eq!((pins.freeze, pins.atemp), (true, Atemp::Fresh(1)));
    let (violation, _) = pod.board_mut().target().violations();
    assert_eq!(violation, None);
}

#[test]
fn call_stacks_and_runs() {
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetHalt);
    let stack = RAM_BASE + 0x80;
    send(&mut pod, &Request::WriteRegister(Register::A7, stack));
    assert_eq!(status(&send(&mut pod, &Request::Call(Address(0x0004_8000)))), Ok(&[][..]));
    let target = pod.board_mut().target();
    assert!(!target.frozen());
    assert_eq!(target.register(Register::A7), stack - 4);
    assert_eq!(target.violations(), (None, 0));
}

#[test]
fn a_call_that_cannot_stack_is_a_bus_error_and_stays_halted() {
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetHalt);
    let unmapped = 0x00FF_0000;
    send(&mut pod, &Request::WriteRegister(Register::A7, unmapped));
    assert_eq!(status(&send(&mut pod, &Request::Call(Address(0x0004_8000)))), failed(Status::BusError));
    assert!(pod.board_mut().target().frozen());
    // The same stay in background mode: ATEMP as read at entry, and the
    // CPU takes the next command.
    let pins = parse_pins(status(&send(&mut pod, &Request::Pins)).unwrap()).unwrap();
    assert_eq!((pins.freeze, pins.atemp), (true, Atemp::Fresh(0)));
    let a7 = status(&send(&mut pod, &Request::ReadRegister(Register::A7))).map(parse_value);
    assert_eq!(a7, Ok(Ok(unmapped)));
    assert_eq!(pod.board_mut().target().violations(), (None, 0));
}

#[test]
fn a_cpu_halted_before_the_pod_started_is_stale() {
    let mut pod = pod_on_halted();
    assert_eq!(parse_atemp(status(&send(&mut pod, &Request::Halt)).unwrap()), Ok(Atemp::Stale(1)));
}

#[test]
fn reset_halt_is_a_new_entry() {
    let mut pod = pod_on_halted();
    assert_eq!(parse_atemp(status(&send(&mut pod, &Request::ResetHalt)).unwrap()), Ok(Atemp::Fresh(0)));
}

#[test]
fn a_failed_entry_read_fails_one_request() {
    let mut pod = pod(true);
    pod.board_mut().hold_dso_high(true);
    assert_eq!(status(&send(&mut pod, &Request::ResetHalt)), failed(Status::Illegal));
    // Not retried until the next entry: Pins and a raw transfer go on.
    let pins = parse_pins(status(&send(&mut pod, &Request::Pins)).unwrap()).unwrap();
    assert_eq!((pins.freeze, pins.atemp), (true, Atemp::Unread));
    assert_eq!(status(&send(&mut pod, &Request::Halt)).map(parse_atemp), Ok(Ok(Atemp::Unread)));
    // A Transfer of one NOP: DSO high comes back as is.
    assert_eq!(status(&raw(&mut pod, &[TAG, TRANSFER, 0, 0])), Ok(&[0xFF, 0xFF, 1, 0][..]));
    // A reset is a new entry, read again.
    pod.board_mut().hold_dso_high(false);
    assert_eq!(parse_atemp(status(&send(&mut pod, &Request::ResetHalt)).unwrap()), Ok(Atemp::Fresh(0)));
}

#[test]
fn reset_run_leaves_the_target_running_without_bdm() {
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetHalt);
    assert_eq!(status(&send(&mut pod, &Request::ResetRun)), Ok(&[][..]));
    let pins = parse_pins(status(&send(&mut pod, &Request::Pins)).unwrap()).unwrap();
    assert_eq!((pins.freeze, pins.atemp), (false, Atemp::None));
    // Background mode is off until a ResetHalt: Halt and Hold are refused
    // before BKPT moves, and the program runs on.
    assert_eq!(status(&send(&mut pod, &Request::Halt)), failed(Status::BdmDisabled));
    assert_eq!(status(&send(&mut pod, &Request::Hold)), failed(Status::BdmDisabled));
    assert!(!pod.board_mut().target().frozen());
    assert_eq!(parse_atemp(status(&send(&mut pod, &Request::ResetHalt)).unwrap()), Ok(Atemp::Fresh(0)));
    assert_eq!(parse_atemp(status(&send(&mut pod, &Request::Halt)).unwrap()), Ok(Atemp::Fresh(0)));
}

#[test]
fn an_entry_after_a_reset_to_run_lifts_the_halt_guard() {
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetRun);
    // A reset the pod did not make, with BKPT low: background mode on.
    pod.board_mut().enter_background(Entry::Breakpoint);
    send(&mut pod, &Request::Go);
    // The pod tries: the model, whose own BDM flag only its resets set,
    // then answers NoFreeze.
    assert_eq!(status(&send(&mut pod, &Request::Halt)), failed(Status::NoFreeze));
}

#[test]
fn a_reset_the_target_holds_is_reset_held() {
    let mut pod = pod(true);
    pod.board_mut().hold_reset(true);
    assert_eq!(status(&send(&mut pod, &Request::ResetRun)), failed(Status::ResetHeld));
    assert_eq!(status(&send(&mut pod, &Request::ResetHalt)), failed(Status::ResetHeld));
    pod.board_mut().hold_reset(false);
    assert!(status(&send(&mut pod, &Request::ResetRun)).is_ok());
}

#[test]
fn reset_run_is_not_stopped_by_a_link_out_of_frame() {
    // Halted before the pod started, its link garbled: no entry read
    // comes before the reset.
    let mut pod = pod_on_halted();
    pod.board_mut().hold_dso_high(true);
    assert_eq!(status(&send(&mut pod, &Request::ResetRun)), Ok(&[][..]));
    assert!(!pod.board_mut().target().frozen());
}

#[test]
fn resync_brings_the_link_into_frame() {
    for skew in [0, 1, 5, 16] {
        let mut pod = pod(true);
        send(&mut pod, &Request::ResetHalt);
        pod.board_mut().put_out_of_frame(skew);
        let atemp = parse_atemp(status(&send(&mut pod, &Request::Resync)).unwrap());
        assert!(matches!(atemp, Ok(Atemp::Stale(_))), "skew {skew}: {atemp:?}");
        send(&mut pod, &Request::WriteRegister(Register::D2, 0x1234_5678));
        let value = parse_value(status(&send(&mut pod, &Request::ReadRegister(Register::D2))).unwrap());
        assert_eq!(value, Ok(0x1234_5678), "skew {skew}");
    }
}

#[test]
fn resync_needs_a_halted_target() {
    let mut pod = pod(true);
    assert_eq!(status(&send(&mut pod, &Request::Resync)), failed(Status::NoFreeze));
}

#[test]
fn a_pod_started_under_a_halted_cpu_out_of_frame_needs_a_resync() {
    let mut pod = pod_on_halted();
    pod.board_mut().put_out_of_frame(9);
    // The entry read fails once, recorded unread; Resync frames the link.
    assert!(status(&send(&mut pod, &Request::Pins)).is_err());
    let atemp = parse_atemp(status(&send(&mut pod, &Request::Resync)).unwrap());
    assert!(matches!(atemp, Ok(Atemp::Stale(_))), "{atemp:?}");
}

#[test]
fn requests_that_need_no_target_skip_the_entry_read() {
    // A host's Hello, then Resync, on a link out of frame: the first
    // run works, and nothing is clocked before the Resync.
    let mut pod = pod_on_halted();
    pod.board_mut().put_out_of_frame(9);
    assert_eq!(parse_hello_reply(&raw(&mut pod, &HELLO_REQUEST)), Some(PROTOCOL_VERSION));
    assert!(status(&send(&mut pod, &Request::Version)).is_ok());
    assert!(status(&send(&mut pod, &Request::SetClock { dsclk_hz: 1_000_000 })).is_ok());
    let atemp = parse_atemp(status(&send(&mut pod, &Request::Resync)).unwrap());
    assert!(matches!(atemp, Ok(Atemp::Stale(_))), "{atemp:?}");
}

#[test]
fn version_is_the_boards_and_needs_no_target() {
    let mut pod = pod(false);
    let reply = send(&mut pod, &Request::Version);
    assert_eq!(status(&reply), Ok(concat!("mc683xx-bdm-model ", env!("CARGO_PKG_VERSION")).as_bytes()));
}

#[test]
fn hold_halts_and_leaves_the_entry_unread() {
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetHalt);
    send(&mut pod, &Request::Go);
    assert_eq!(status(&send(&mut pod, &Request::Hold)), Ok(&[][..]));
    // Nothing was clocked: the entry is read by the next request.
    let pins = parse_pins(status(&send(&mut pod, &Request::Pins)).unwrap()).unwrap();
    assert_eq!((pins.freeze, pins.atemp), (true, Atemp::Fresh(0)));
    let (violation, _) = pod.board_mut().target().violations();
    assert_eq!(violation, None);
}

#[test]
fn a_hung_access_is_ended_by_berr() {
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetHalt);
    pod.board_mut().target().memory_busy = u32::MAX;
    let read = Request::ReadMemory { address: Address(RAM_BASE), size: Size::Word, length: 2 };
    assert_eq!(status(&send(&mut pod, &read)), failed(Status::BusError));
    // The CPU answers again.
    pod.board_mut().target().memory_busy = 0;
    assert!(status(&send(&mut pod, &read)).is_ok());
}

#[test]
fn berr_pulse_needs_a_halted_target_and_says_whether_anything_hung() {
    let mut pod = pod(true);
    assert_eq!(status(&send(&mut pod, &Request::BerrPulse)), failed(Status::NoFreeze));
    send(&mut pod, &Request::ResetHalt);
    assert_eq!(parse_value(status(&send(&mut pod, &Request::BerrPulse)).unwrap()), Ok(0));
}

#[test]
fn berr_pulse_drives_the_link_first() {
    // A CPU hung in a BDM read before the pod booted: the pod has not
    // driven the link, and nothing else in the request does.
    let mut target = Target::new(Address(RAM_BASE));
    target.enter_background(Entry::Background);
    target.memory_busy = u32::MAX;
    for word in Command::Read(Size::Word, Address(RAM_BASE)).encode().as_slice() {
        target.exchange(*word, false);
    }
    let mut pod = Pod::new(ModelBoard::new(target, true));
    assert_eq!(parse_value(status(&send(&mut pod, &Request::BerrPulse)).unwrap()), Ok(1));
}

#[test]
fn commands_drive_the_link_after_a_go_the_cpu_did_not_take() {
    // GO's word drops DSI as it ends, taken or not; FREEZE has not moved,
    // so no entry read drives the link again.
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetHalt);
    let data = [1, 2];
    for request in [
        Request::WriteMemory { address: Address(RAM_BASE), size: Size::Word, data: &data },
        Request::ReadMemory { address: Address(RAM_BASE), size: Size::Word, length: 2 },
        Request::ReadRegister(Register::D0),
    ] {
        pod.board_mut().put_out_of_frame(9);
        send(&mut pod, &Request::Go);
        assert!(pod.board_mut().target().frozen());
        pod.board_mut().put_out_of_frame(0);
        assert!(status(&send(&mut pod, &request)).is_ok(), "{request:?}");
    }
}

#[test]
fn memory_round_trips() {
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetHalt);
    let data = [1, 2, 3, 4, 5, 6, 7, 8];
    let write = Request::WriteMemory { address: Address(RAM_BASE), size: Size::Word, data: &data };
    assert_eq!(status(&send(&mut pod, &write)), Ok(&[][..]));
    let read = Request::ReadMemory { address: Address(RAM_BASE), size: Size::Long, length: 8 };
    assert_eq!(status(&send(&mut pod, &read)), Ok(&data[..]));
    let outside = Request::ReadMemory { address: Address(0), size: Size::Byte, length: 1 };
    assert_eq!(status(&send(&mut pod, &outside)), failed(Status::BusError));
}

#[test]
fn set_clock_is_held_to_the_range() {
    let mut pod = pod(true);
    let rate = |pod: &mut ModelPod, dsclk_hz| {
        parse_value(status(&send(pod, &Request::SetClock { dsclk_hz })).unwrap())
    };
    assert_eq!(rate(&mut pod, 0), Ok(1));
    assert_eq!(rate(&mut pod, 10_000_000), Ok(4_000_000));
}
