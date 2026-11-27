//! `bdm-stand-in`: a pod on a socket, answering for a model CPU32
//! (`mc683xx_bdm_model`) with no hardware.  `bdm --pod unix:PATH` talks to it.

use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::bail;
use clap::Parser;
use mc683xx_bdm::stand_in::serve;
use mc683xx_bdm_model::Target;
use mc683xx_bdm_model::board::ModelBoard;
use mc683xx_bdm_proto::pod::Pod;
use mc683xx_target::Address;
use mc683xx_target::Entry;
use mc683xx_target::parse_integer;

/// The model's RAM, in bytes.
const RAM: usize = 64 * 1024;

/// How long a new connection waits for the last client's thread to see
/// its end of file before it is refused (docs/usb-protocol.md,
/// Transport).
const HANDOVER: Duration = Duration::from_millis(100);
/// How often it looks.
const HANDOVER_POLL: Duration = Duration::from_millis(5);

type ModelPod = Pod<ModelBoard<RAM>>;

/// A stand-in pod for a model CPU32 (docs/usb-protocol.md, stream
/// transport).  One client at a time; another is closed at once.
#[derive(Debug, Parser)]
#[command(name = "bdm-stand-in", version)]
struct Cli {
    /// Where to listen: `unix:PATH` or `tcp:HOST:PORT`.
    listen: String,
    /// Where the model's 64 KiB of RAM starts (decimal, `0x` or `0b`);
    /// every other address is a bus error.
    #[arg(long, value_parser = parse_integer::<u32>, default_value = "0x100000")]
    ram_base: u32,
    /// Start with BDM not enabled, as after a reset without BKPT held:
    /// a halt then fails until a reset into background mode.
    #[arg(long)]
    no_bdm: bool,
    /// Start halted, before the pod's time: its ATEMP is stale.
    #[arg(long)]
    halted: bool,
}

/// Serves each connection in its own thread, closing it if another
/// client is still connected after [`HANDOVER`].
fn accept<S: std::io::Read + std::io::Write + Send + 'static>(
    connections: impl Iterator<Item = std::io::Result<S>>,
    pod: &Arc<Mutex<ModelPod>>,
) -> Result<()> {
    let busy = Arc::new(AtomicBool::new(false));
    for stream in connections {
        let stream = stream?;
        let deadline = Instant::now() + HANDOVER;
        while busy.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(HANDOVER_POLL);
        }
        if busy.swap(true, Ordering::AcqRel) {
            eprintln!("bdm-stand-in: refused a second client");
            continue;
        }
        let (pod, busy) = (Arc::clone(pod), Arc::clone(&busy));
        thread::spawn(move || {
            if let Err(error) = serve(stream, &pod) {
                eprintln!("bdm-stand-in: client dropped: {error}");
            }
            busy.store(false, Ordering::Release);
        });
    }
    Ok(())
}

fn run(cli: &Cli) -> Result<()> {
    let mut target = Target::<RAM>::new(Address(cli.ram_base));
    if cli.halted {
        target.enter_background(Entry::Background);
    }
    let pod = Arc::new(Mutex::new(Pod::new(ModelBoard::new(target, !cli.no_bdm))));
    match cli.listen.split_once(':') {
        Some(("unix", path)) => accept(UnixListener::bind(path)?.incoming(), &pod),
        Some(("tcp", address)) => accept(TcpListener::bind(address)?.incoming(), &pod),
        _ => bail!("listen on unix:PATH or tcp:HOST:PORT, not {}", cli.listen),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bdm-stand-in: {error:#}");
            ExitCode::FAILURE
        }
    }
}
