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
use pod_host::pod::Pod;
use pod_host::stand_in::serve;
use pod_host::target::PodTarget;
use pod_host::transport::StreamTransport;
use pod_proto::usb::PROTOCOL_VERSION;

const RAM_BASE: u32 = 0x0010_0000;
const RAM: usize = 256;

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
