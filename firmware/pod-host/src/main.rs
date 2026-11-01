//! `bdm`: the pod from the command line.

use std::process::ExitCode;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use mc683xx_target::address::Address;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;
use pod_host::pod::Pod;
use pod_host::transport::StreamTransport;
use pod_host::transport::Transport;
use pod_host::transport::UsbTransport;
use pod_proto::usb::PROTOCOL_VERSION;

/// Bytes per hex dump line.
const DUMP_LINE: usize = 16;

/// The CPU32 BDM pod (PLAN.md).
#[derive(Debug, Parser)]
#[command(name = "bdm", version)]
struct Cli {
    /// The pod: `usb`, or a stand-in on `unix:PATH` or `tcp:HOST:PORT`
    /// (docs/usb-protocol.md).
    #[arg(long, default_value = "usb")]
    pod: String,
    #[command(subcommand)]
    command: Command,
}

/// What to do.  Numbers are decimal, or hex with `0x` or `$`.
#[derive(Debug, Subcommand)]
enum Command {
    /// FREEZE, RESET and DS levels, and target VDD.
    Pins,
    /// Stop the target in background mode.
    Halt,
    /// Reset the target into background mode.
    Reset,
    /// Resume at RPC.
    Go,
    /// Stack RPC and run from ADDRESS.
    Call {
        #[arg(value_parser = number)]
        address: u32,
    },
    /// Set DSCLK's rate.
    Clock {
        #[arg(value_parser = number)]
        hz: u32,
    },
    /// Every data, address and system register.
    Regs,
    /// One register: d0-d7, a0-a7, sp, or a system register (rpc, pcc,
    /// atemp, far, vbr, sr, usp, ssp, sfc, dfc).
    Get { register: String },
    /// Write a register.
    Set {
        register: String,
        #[arg(value_parser = number)]
        value: u32,
    },
    /// Hex dump LENGTH bytes from ADDRESS, through SFC.
    Dump {
        #[arg(value_parser = number)]
        address: u32,
        #[arg(value_parser = number)]
        length: u32,
        #[arg(long, value_enum, default_value_t = Width::Word)]
        size: Width,
    },
    /// Write BYTES at ADDRESS, through DFC.
    Write {
        #[arg(value_parser = number)]
        address: u32,
        /// Bytes, each a number.
        #[arg(value_parser = byte, required = true)]
        bytes: Vec<u8>,
        #[arg(long, value_enum, default_value_t = Width::Byte)]
        size: Width,
    },
    /// Raw serial words, each answered with the 17 bits shifted in.
    /// With the target running, the clocks are BKPT pulses.
    Xfer {
        #[arg(value_parser = word, required = true)]
        words: Vec<u16>,
    },
}

/// An access size on the command line.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Width {
    Byte,
    Word,
    Long,
}

impl From<Width> for Size {
    fn from(width: Width) -> Self {
        match width {
            Width::Byte => Self::Byte,
            Width::Word => Self::Word,
            Width::Long => Self::Long,
        }
    }
}

/// A register by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Named {
    Data(Register),
    System(SystemRegister),
}

/// System registers' names, in the order `regs` prints them.
const SYSTEM_NAMES: [(&str, SystemRegister); 10] = [
    ("rpc", SystemRegister::Rpc),
    ("pcc", SystemRegister::Pcc),
    ("sr", SystemRegister::Sr),
    ("usp", SystemRegister::Usp),
    ("ssp", SystemRegister::Ssp),
    ("vbr", SystemRegister::Vbr),
    ("sfc", SystemRegister::Sfc),
    ("dfc", SystemRegister::Dfc),
    ("far", SystemRegister::Far),
    ("atemp", SystemRegister::Atemp),
];

fn register(name: &str) -> Result<Named> {
    let name = name.to_ascii_lowercase();
    if name == "sp" {
        return Ok(Named::Data(Register::A7));
    }
    if let Some((_, register)) = SYSTEM_NAMES.iter().find(|(n, _)| *n == name) {
        return Ok(Named::System(*register));
    }
    let (bank, number) = name.split_at_checked(1).context("empty register name")?;
    let number: u16 =
        number.parse().ok().filter(|n| *n < 8).with_context(|| format!("no register {name}"))?;
    let code = match bank {
        "d" => number,
        "a" => number + 8,
        _ => bail!("no register {name}"),
    };
    Ok(Named::Data(Register::from_code(code).context("register code")?))
}

/// The name `regs` prints for a data or address register.
fn register_name(register: Register) -> String {
    let code = register.code();
    if code < 8 { format!("d{code}") } else { format!("a{}", code - 8) }
}

fn number(text: &str) -> Result<u32> {
    let hex = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")).or_else(|| text.strip_prefix('$'));
    match hex {
        Some(digits) => u32::from_str_radix(digits, 16),
        None => text.parse(),
    }
    .with_context(|| format!("not a number: {text}"))
}

fn byte(text: &str) -> Result<u8> {
    u8::try_from(number(text)?).with_context(|| format!("not a byte: {text}"))
}

fn word(text: &str) -> Result<u16> {
    u16::try_from(number(text)?).with_context(|| format!("not a 16-bit word: {text}"))
}

/// What ATEMP says about how the CPU entered background mode (CPU32RM
/// Table 7-2).
fn entry_source(atemp: u32) -> String {
    match atemp {
        0x0000_0000 => "hardware breakpoint".to_owned(),
        0x0000_0001 => "BGND instruction".to_owned(),
        _ if atemp & 0xFFFF == 0xFFFF => format!("double bus fault, SSW ${:04x}", atemp >> 16),
        _ => "unknown".to_owned(),
    }
}

/// The transport `--pod` names.
fn connect(pod: &str) -> Result<Box<dyn Transport>> {
    Ok(match pod.split_once(':') {
        None if pod == "usb" => Box::new(UsbTransport::open()?),
        Some(("unix", path)) => Box::new(StreamTransport::unix(path)?),
        Some(("tcp", address)) => Box::new(StreamTransport::tcp(address)?),
        _ => bail!("--pod is usb, unix:PATH or tcp:HOST:PORT, not {pod}"),
    })
}

fn run(pod: &str, command: Command) -> Result<()> {
    let mut pod = Pod::new(connect(pod)?);
    let version = pod.hello()?;
    if version != PROTOCOL_VERSION {
        bail!("the pod speaks protocol {version}, this bdm {PROTOCOL_VERSION}");
    }
    match command {
        Command::Pins => {
            let pins = pod.pins()?;
            let level = |high: bool| if high { "high" } else { "low" };
            println!("FREEZE {}", level(pins.freeze));
            println!("RESET  {}", level(pins.reset));
            println!("DS     {}", level(pins.ds));
            println!("VDD    {}.{:03} V", pins.vdd_mv / 1000, pins.vdd_mv % 1000);
        }
        Command::Halt | Command::Reset => {
            let atemp = if matches!(command, Command::Halt) { pod.halt()? } else { pod.reset_halt()? };
            println!("halted: {} (ATEMP ${atemp:08x})", entry_source(atemp));
        }
        Command::Go => pod.go()?,
        Command::Call { address } => pod.call(Address(address))?,
        Command::Clock { hz } => println!("DSCLK {} Hz", pod.set_clock(hz)?),
        Command::Regs => {
            for register in Register::ALL {
                println!("{:<5} ${:08x}", register_name(register), pod.read_register(register)?);
            }
            for (name, register) in SYSTEM_NAMES.iter().filter(|(_, r)| *r != SystemRegister::Atemp) {
                println!("{name:<5} ${:08x}", pod.read_system_register(*register)?);
            }
        }
        Command::Get { register: name } => {
            let value = match register(&name)? {
                Named::Data(r) => pod.read_register(r)?,
                Named::System(r) => pod.read_system_register(r)?,
            };
            println!("${value:08x}");
        }
        Command::Set { register: name, value } => match register(&name)? {
            Named::Data(r) => pod.write_register(r, value)?,
            Named::System(r) => pod.write_system_register(r, value)?,
        },
        Command::Dump { address, length, size } => {
            let mut data = vec![0; usize::try_from(length)?];
            pod.read_memory(Address(address), size.into(), &mut data)?;
            for (n, line) in data.chunks(DUMP_LINE).enumerate() {
                let at = address.wrapping_add(u32::try_from(n * DUMP_LINE)?);
                let hex: Vec<String> = line.iter().map(|b| format!("{b:02x}")).collect();
                let text: String = line
                    .iter()
                    .map(|b| if b.is_ascii_graphic() || *b == b' ' { char::from(*b) } else { '.' })
                    .collect();
                println!("{at:08x}  {:<47}  {text}", hex.join(" "));
            }
        }
        Command::Write { address, bytes, size } => pod.write_memory(Address(address), size.into(), &bytes)?,
        Command::Xfer { words } => {
            for (word, response) in words.iter().zip(pod.transfer(&words)?) {
                println!("{word:04x} -> {}:{:04x}", response >> 16, response & 0xFFFF);
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli.pod, cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bdm: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Named;
    use super::Register;
    use super::SystemRegister;
    use super::entry_source;
    use super::number;
    use super::register;
    use super::register_name;

    #[test]
    fn numbers() {
        assert_eq!(number("42").unwrap(), 42);
        assert_eq!(number("0xFFFA00").unwrap(), 0x00FF_FA00);
        assert_eq!(number("$ff").unwrap(), 0xFF);
        assert!(number("0xG").is_err());
    }

    #[test]
    fn register_names() {
        assert_eq!(register("d0").unwrap(), Named::Data(Register::D0));
        assert_eq!(register("A7").unwrap(), Named::Data(Register::A7));
        assert_eq!(register("sp").unwrap(), Named::Data(Register::A7));
        assert_eq!(register("sfc").unwrap(), Named::System(SystemRegister::Sfc));
        assert!(register("d8").is_err());
        assert!(register("x1").is_err());
        assert!(register("").is_err());
        for r in Register::ALL {
            assert_eq!(register(&register_name(r)).unwrap(), Named::Data(r));
        }
    }

    #[test]
    fn entry_sources() {
        assert_eq!(entry_source(0), "hardware breakpoint");
        assert_eq!(entry_source(1), "BGND instruction");
        assert_eq!(entry_source(0x1234_FFFF), "double bus fault, SSW $1234");
        assert_eq!(entry_source(0x0000_0002), "unknown");
    }
}
