//! `pod_proto::pod` against the model board: the pod's answer to each
//! request (docs/usb-protocol.md), from the bytes in to the bytes out.

use bdm_model::Target;
use bdm_model::board::ModelBoard;
use embassy_futures::block_on;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::register::Register;
use mc683xx_target::size::Size;
use pod_proto::pod::Pod;
use pod_proto::usb::Atemp;
use pod_proto::usb::PACKET;
use pod_proto::usb::PROTOCOL_VERSION;
use pod_proto::usb::ReplyError;
use pod_proto::usb::Request;
use pod_proto::usb::Status;
use pod_proto::usb::parse_atemp;
use pod_proto::usb::parse_pins;
use pod_proto::usb::parse_reply;
use pod_proto::usb::parse_value;

/// The model's RAM: where, and how much.
const RAM_BASE: u32 = 0x0010_0000;
const RAM: usize = 256;

type ModelPod = Pod<ModelBoard<RAM>>;

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

/// Sends `request`; returns the reply's bytes.
fn send(pod: &mut ModelPod, request: &Request<'_>) -> Vec<u8> {
    let mut packet = [0; PACKET];
    let length = request.encode(&mut packet).unwrap();
    raw(pod, &packet[..length])
}

fn raw(pod: &mut ModelPod, packet: &[u8]) -> Vec<u8> {
    let mut reply = [0; PACKET];
    let length = block_on(pod.handle(packet, &mut reply));
    reply[..length].to_vec()
}

fn status(reply: &[u8]) -> Result<&[u8], ReplyError> {
    parse_reply(reply)
}

fn failed(status: Status) -> Result<&'static [u8], ReplyError> {
    Err(ReplyError::Failed(status))
}

#[test]
fn hello() {
    let mut pod = pod(true);
    assert_eq!(parse_value(status(&send(&mut pod, &Request::Hello)).unwrap()), Ok(PROTOCOL_VERSION));
}

#[test]
fn bad_requests_are_one_byte() {
    let mut pod = pod(true);
    for packet in [&[][..], &[0xFF], &[0x02, 0], &[0x0B, 0, 0, 0, 0, 3, 4]] {
        assert_eq!(raw(&mut pod, packet), [Status::BadRequest as u8], "{packet:?}");
    }
}

#[test]
fn commands_need_freeze() {
    let mut pod = pod(true);
    for request in [
        Request::Go,
        Request::ReadRegister(Register::D0),
        Request::ReadMemory { address: Address(RAM_BASE), size: Size::Byte, length: 1 },
    ] {
        assert_eq!(send(&mut pod, &request), [Status::NoFreeze as u8], "{request:?}");
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
fn memory_round_trips() {
    let mut pod = pod(true);
    send(&mut pod, &Request::ResetHalt);
    let data = [1, 2, 3, 4, 5, 6, 7, 8];
    let write = Request::WriteMemory { address: Address(RAM_BASE), size: Size::Word, data: &data };
    assert_eq!(status(&send(&mut pod, &write)), Ok(&[][..]));
    let read = Request::ReadMemory { address: Address(RAM_BASE), size: Size::Long, length: 8 };
    assert_eq!(status(&send(&mut pod, &read)), Ok(&data[..]));
    let outside = Request::ReadMemory { address: Address(0), size: Size::Byte, length: 1 };
    assert_eq!(send(&mut pod, &outside), [Status::BusError as u8]);
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
