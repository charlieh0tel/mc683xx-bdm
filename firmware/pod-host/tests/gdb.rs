//! `bdm gdb` end to end: the `bdm` binary on a stand-in pod over a
//! model CPU32, driven by gdb's remote serial protocol on TCP, packet
//! by packet, as gdb would; and ended by a signal.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::os::unix::net::UnixListener;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use bdm_model::Target;
use bdm_model::board::ModelBoard;
use mc683xx_target::address::Address;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use pod_host::stand_in::serve;

const RAM_BASE: u32 = 0x0010_0000;
const RAM: usize = 4096;
/// The program, the step's scratch RAM and the supervisor stack.
const PROGRAM: u32 = RAM_BASE;
const SCRATCH: u32 = RAM_BASE + 0x800;
const STACK: u32 = RAM_BASE + 0x1000;
/// `moveq #5,d0`, `moveq #-1,d1`, NOP, and `ori.b #0,d0`, which the
/// model does not run: there it runs on until halted.
const CODE: [u8; 8] = [0x70, 0x05, 0x72, 0xFF, 0x4E, 0x71, 0x00, 0x00];
/// Where the NOP is.
const NOP_AT: u32 = PROGRAM + 4;
/// A breakpoint the program never reaches, and the word under it.
const UNREACHED: u32 = PROGRAM + 0x20;
const UNREACHED_WORD: [u8; 2] = [0x4E, 0x71];
/// The program's SFC and DFC: user data, not the supervisor data
/// gdb's memory accesses use.
const USER_DATA: u32 = 1;
/// BGND, a planted breakpoint.
const BGND: [u8; 2] = [0x4A, 0xFA];
/// How long bdm may take to plant a breakpoint, and how often to look.
const PLANTING: Duration = Duration::from_secs(5);
const PLANTING_POLL: Duration = Duration::from_millis(5);
/// The exit status of a process ended by a signal: this plus its number.
const SIGNALED_STATUS: i32 = 128;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
/// SR: supervisor.
const SUPERVISOR: u32 = 0x2000;
/// gdb's Ctrl-C.
const INTERRUPT: u8 = 0x03;
/// PC's index in a `g` reply, and hex digits in a register.
const PC: usize = 17;
const REGISTER_DIGITS: usize = 8;

type ModelPod = pod_proto::pod::Pod<ModelBoard<RAM>>;

/// gdb's end of the connection.
struct Gdb {
    stream: BufReader<TcpStream>,
}

impl Gdb {
    /// Sends `body` as a packet and returns the reply's body, acking it.
    fn ask(&mut self, body: &str) -> String {
        self.send(body);
        self.reply()
    }

    fn send(&mut self, body: &str) {
        let sum = body.bytes().fold(0u8, u8::wrapping_add);
        write!(self.stream.get_mut(), "${body}#{sum:02x}").unwrap();
    }

    /// The next packet's body, past any acks; acked.
    fn reply(&mut self) -> String {
        let mut byte = [0];
        loop {
            self.stream.read_exact(&mut byte).unwrap();
            if byte[0] == b'$' {
                break;
            }
        }
        let mut packet = Vec::new();
        self.stream.read_until(b'#', &mut packet).unwrap();
        packet.pop();
        let mut sum = [0; 2];
        self.stream.read_exact(&mut sum).unwrap();
        self.stream.get_mut().write_all(b"+").unwrap();
        String::from_utf8(expanded(&packet)).unwrap()
    }

    /// PC, from a `g` reply.
    fn pc(&mut self) -> u32 {
        let registers = self.ask("g");
        let at = PC * REGISTER_DIGITS;
        u32::from_str_radix(&registers[at..at + REGISTER_DIGITS], 16).unwrap()
    }
}

/// The remote protocol's run-length encoding: `*` and a count byte
/// repeat the byte before it, the count less 29 more times.
fn expanded(packet: &[u8]) -> Vec<u8> {
    /// What a count byte's value exceeds the repeats by.
    const RUN_BIAS: u8 = 29;
    let mut bytes: Vec<u8> = Vec::new();
    let mut rest = packet.iter();
    while let Some(&byte) = rest.next() {
        if byte == b'*' {
            let count = rest.next().expect("a count after *") - RUN_BIAS;
            let last = *bytes.last().expect("a byte to repeat");
            bytes.extend(std::iter::repeat_n(last, usize::from(count)));
        } else {
            bytes.push(byte);
        }
    }
    bytes
}

/// A stand-in pod on a socket in the temporary directory, its model
/// halted at [`PROGRAM`] with [`CODE`] there, and `bdm gdb` on it,
/// waiting for gdb at the address returned.
fn start(name: &str) -> (Child, String, Arc<Mutex<ModelPod>>) {
    let path = std::env::temp_dir().join(format!("bdm-gdb-test-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let mut model = Target::<RAM>::new(Address(RAM_BASE));
    model.ram_mut()[..CODE.len()].copy_from_slice(&CODE);
    let unreached = usize::try_from(UNREACHED - RAM_BASE).unwrap();
    model.ram_mut()[unreached..unreached + 2].copy_from_slice(&UNREACHED_WORD);
    model.set_system_register(SystemRegister::Sr, SUPERVISOR);
    model.set_system_register(SystemRegister::Sfc, USER_DATA);
    model.set_system_register(SystemRegister::Dfc, USER_DATA);
    model.set_register(Register::A7, STACK);
    model.set_system_register(SystemRegister::Rpc, PROGRAM);
    let pod = Arc::new(Mutex::new(ModelPod::new(ModelBoard::new(model, true))));
    let served = Arc::clone(&pod);
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        serve(stream, &served).unwrap();
    });
    let mut bdm = Command::new(env!("CARGO_BIN_EXE_bdm"))
        .arg("--pod")
        .arg(format!("unix:{}", path.display()))
        .args(["gdb", "--listen", "127.0.0.1:0", "--scratch", &format!("{SCRATCH:#x}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut waiting = String::new();
    BufReader::new(bdm.stderr.take().unwrap()).read_line(&mut waiting).unwrap();
    let at = waiting.split_whitespace().nth(5).unwrap_or_else(|| panic!("no address in {waiting:?}"));
    (bdm, at.to_owned(), pod)
}

/// [`start`], and gdb connected.
fn session(name: &str) -> (Child, Gdb, Arc<Mutex<ModelPod>>) {
    let (bdm, at, pod) = start(name);
    let stream = BufReader::new(TcpStream::connect(at).unwrap());
    (bdm, Gdb { stream }, pod)
}

/// Sends `bdm` the signal `number`, as `kill` does.
fn signal(bdm: &Child, number: i32) {
    let sent = Command::new("kill").arg(format!("-{number}")).arg(bdm.id().to_string()).status().unwrap();
    assert!(sent.success(), "kill -{number} failed");
}

/// The word at `address` in the model's RAM.
fn word(pod: &Mutex<ModelPod>, address: u32) -> [u8; 2] {
    let at = usize::try_from(address - RAM_BASE).unwrap();
    let mut pod = pod.lock().unwrap();
    let ram = pod.board_mut().target().ram_mut();
    [ram[at], ram[at + 1]]
}

/// The model's SFC and DFC.
fn function_codes(pod: &Mutex<ModelPod>) -> (u32, u32) {
    let mut pod = pod.lock().unwrap();
    let target = pod.board_mut().target();
    (target.system_register(SystemRegister::Sfc), target.system_register(SystemRegister::Dfc))
}

/// What `bdm` printed and whether it succeeded, once it exits.
fn finish(bdm: Child) -> (bool, String) {
    let (status, printed) = finish_with_status(bdm);
    (status == Some(0), printed)
}

/// What `bdm` printed and its exit status, once it exits.
fn finish_with_status(bdm: Child) -> (Option<i32>, String) {
    let output = bdm.wait_with_output().unwrap();
    (output.status.code(), String::from_utf8(output.stdout).unwrap())
}

#[test]
fn step_break_continue_interrupt_and_detach() {
    let (bdm, mut gdb, pod) = session("session");
    assert!(gdb.ask("?").starts_with("T05"), "halted for gdb");
    assert_eq!(gdb.ask(&format!("m{PROGRAM:x},8")), "700572ff4e710000");
    assert_eq!(gdb.pc(), PROGRAM);

    assert_eq!(gdb.ask("s"), "S05", "a step is a trap");
    assert_eq!(gdb.pc(), PROGRAM + 2);
    assert_eq!(pod.lock().unwrap().board_mut().target().register(Register::D0), 5);

    assert_eq!(gdb.ask(&format!("Z0,{NOP_AT:x},2")), "OK");
    assert_eq!(gdb.ask("c"), "T05thread:01;swbreak:;", "stopped at the breakpoint");
    assert_eq!(gdb.pc(), NOP_AT);
    assert_eq!(pod.lock().unwrap().board_mut().target().register(Register::D1), 0xFFFF_FFFF);
    assert_eq!(gdb.ask(&format!("z0,{NOP_AT:x},2")), "OK");
    assert_eq!(gdb.ask(&format!("m{NOP_AT:x},2")), "4e71", "the NOP back");

    gdb.send("c");
    gdb.stream.get_mut().write_all(&[INTERRUPT]).unwrap();
    assert_eq!(gdb.reply(), "S02", "Ctrl-C halts it");
    assert_eq!(gdb.pc(), NOP_AT + 2, "where the model stopped following");

    assert_eq!(gdb.ask("D"), "OK");
    let (succeeded, printed) = finish(bdm);
    assert!(succeeded);
    assert_eq!(printed, "gdb detached: running\n");
    let (violation, _) = pod.lock().unwrap().board_mut().target().violations();
    assert_eq!(violation, None);
}

#[test]
fn kill_leaves_it_halted() {
    let (bdm, mut gdb, pod) = session("kill");
    assert!(gdb.ask("?").starts_with("T05"));
    gdb.send("k");
    let (succeeded, printed) = finish(bdm);
    assert!(succeeded);
    assert_eq!(printed, "gdb killed: halted\n");
    assert!(pod.lock().unwrap().board_mut().target().frozen());
}

#[test]
fn sigint_while_running_removes_the_breakpoints() {
    let (bdm, mut gdb, pod) = session("sigint");
    assert!(gdb.ask("?").starts_with("T05"));
    assert_eq!(gdb.ask(&format!("Z0,{UNREACHED:x},2")), "OK");
    gdb.send("c");
    let deadline = Instant::now() + PLANTING;
    while word(&pod, UNREACHED) != BGND {
        assert!(Instant::now() < deadline, "the breakpoint was never planted");
        thread::sleep(PLANTING_POLL);
    }
    signal(&bdm, SIGINT);
    let (status, printed) = finish_with_status(bdm);
    assert_eq!(status, Some(SIGNALED_STATUS + SIGINT));
    assert_eq!(printed, "gdb ended by SIGINT: breakpoints removed, halted\n");
    assert_eq!(word(&pod, UNREACHED), UNREACHED_WORD, "the word BGND replaced");
    assert!(pod.lock().unwrap().board_mut().target().frozen());
    assert_eq!(function_codes(&pod), (USER_DATA, USER_DATA), "the program's");
}

#[test]
fn sigterm_while_waiting_for_gdb_leaves_the_target_alone() {
    let (bdm, _, pod) = start("sigterm");
    signal(&bdm, SIGTERM);
    let (status, printed) = finish_with_status(bdm);
    assert_eq!(status, Some(SIGNALED_STATUS + SIGTERM));
    assert_eq!(printed, "gdb never connected: SIGTERM\n");
    assert_eq!(function_codes(&pod), (USER_DATA, USER_DATA));
}
