//! `bdm`: the pod from the command line.

use std::process::ExitCode;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use mc683xx_target::Cpu32Target;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::function_code::FunctionCode;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;
use pod_host::pod::Pod;
use pod_host::target::PodTarget;
use pod_host::transport::StreamTransport;
use pod_host::transport::Transport;
use pod_host::transport::UsbTransport;
use pod_proto::usb::Atemp;
use pod_proto::usb::PROTOCOL_VERSION;

/// Bytes per hex dump line.
const DUMP_LINE: usize = 16;

/// Loopback words: all clear and all set, alternating bits, and a
/// walking one and a walking zero, so a stuck, shorted or shifted bit
/// shows.
fn loopback_words() -> Vec<u16> {
    let walking: Vec<u16> = (0..u16::BITS).map(|bit| 1 << bit).collect();
    [0x0000, 0xFFFF, 0xA5A5, 0x5A5A]
        .into_iter()
        .chain(walking.iter().copied())
        .chain(walking.iter().map(|w| !w))
        .collect()
}

/// The first word that did not come back as sent: in loopback the 17th
/// bit shifted in is the control bit sent, always clear.
fn loopback_mismatch(words: &[u16], responses: &[u32]) -> Option<(usize, u16, u32)> {
    words
        .iter()
        .zip(responses)
        .enumerate()
        .find(|(_, (word, response))| u32::from(**word) != **response)
        .map(|(n, (word, response))| (n, *word, *response))
}

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
    /// FREEZE, RESET and DS levels, target VDD, and ATEMP when halted.
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
    /// Hex dump LENGTH bytes from ADDRESS.
    Dump {
        #[arg(value_parser = number)]
        address: u32,
        #[arg(value_parser = number)]
        length: u32,
        #[arg(long, value_enum, default_value_t = Width::Word)]
        size: Width,
        /// Function code, the address space: 5 supervisor data, 6
        /// supervisor program, 1 and 2 user, 7 CPU space.
        #[arg(long, value_parser = function_code, default_value = "5")]
        fc: FunctionCode,
    },
    /// Write BYTES at ADDRESS.
    Write {
        #[arg(value_parser = number)]
        address: u32,
        /// Bytes, each a number.
        #[arg(value_parser = byte, required = true)]
        bytes: Vec<u8>,
        #[arg(long, value_enum, default_value_t = Width::Byte)]
        size: Width,
        /// Function code, as for dump.
        #[arg(long, value_parser = function_code, default_value = "5")]
        fc: FunctionCode,
    },
    /// With DSI jumpered to DSO (GP3 to GP4) on a bare Pico, no
    /// target: each word should come back as sent.  Checks the PIO,
    /// framing and USB (docs/bring-up.md).
    Loopback {
        /// Times through the test patterns.
        #[arg(long, default_value_t = 1)]
        rounds: u32,
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

/// System registers in the order `regs` prints them; ATEMP is left
/// out, as reading it after entry gives only a scratch value.
const SYSTEM_REGISTERS: [SystemRegister; 9] = [
    SystemRegister::Rpc,
    SystemRegister::Pcc,
    SystemRegister::Sr,
    SystemRegister::Usp,
    SystemRegister::Ssp,
    SystemRegister::Vbr,
    SystemRegister::Sfc,
    SystemRegister::Dfc,
    SystemRegister::Far,
];

/// A register by name: `d0`-`a7`, `sp` for A7, or a system register.
fn register(name: &str) -> Result<Named> {
    if name.eq_ignore_ascii_case("sp") {
        return Ok(Named::Data(Register::A7));
    }
    name.parse()
        .map(Named::Data)
        .or_else(|_| name.parse().map(Named::System))
        .map_err(|_| anyhow::anyhow!("no register {name}"))
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

fn function_code(text: &str) -> Result<FunctionCode> {
    let bits = number(text)?;
    let code = FunctionCode::new(bits);
    if u32::from(code.bits()) == bits { Ok(code) } else { bail!("not a function code, 0-7: {text}") }
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
    let mut target = PodTarget::new(Pod::new(connect(pod)?));
    let version = target.pod().hello()?;
    if version != PROTOCOL_VERSION {
        bail!("the pod speaks protocol {version}, this bdm {PROTOCOL_VERSION}");
    }
    let done = execute(&mut target, command);
    // The next bdm starts knowing nothing: leave the program's SFC and
    // DFC, whether or not the command worked.
    let restored = target.restore_function_codes();
    done?;
    Ok(restored?)
}

fn execute<T: Transport>(target: &mut PodTarget<T>, command: Command) -> Result<()> {
    match command {
        Command::Pins => {
            let pins = target.pod().pins()?;
            let level = |high: bool| if high { "high" } else { "low" };
            println!("FREEZE {}", level(pins.freeze));
            println!("RESET  {}", level(pins.reset));
            println!("DS     {}", level(pins.ds));
            println!("VDD    {}.{:03} V", pins.vdd_mv / 1000, pins.vdd_mv % 1000);
            match pins.atemp {
                Atemp::None => {}
                Atemp::Fresh(atemp) => println!("ATEMP  ${atemp:08x} ({})", Entry::from_atemp(atemp)),
                Atemp::Stale(atemp) => println!("ATEMP  ${atemp:08x} (stale: the pod did not see the entry)"),
            }
        }
        Command::Halt => println!("halted: {}", target.halt()?),
        Command::Reset => println!("halted: {}", target.reset_halted()?),
        Command::Go => target.go()?,
        Command::Call { address } => target.call(Address(address))?,
        Command::Clock { hz } => println!("DSCLK {} Hz", target.pod().set_clock(hz)?),
        Command::Regs => {
            for register in Register::ALL {
                println!("{register:<5} ${:08x}", target.read_register(register)?);
            }
            for register in SYSTEM_REGISTERS {
                println!("{register:<5} ${:08x}", target.read_system_register(register)?);
            }
        }
        Command::Get { register: name } => {
            let value = match register(&name)? {
                Named::Data(r) => target.read_register(r)?,
                Named::System(r) => target.read_system_register(r)?,
            };
            println!("${value:08x}");
        }
        Command::Set { register: name, value } => match register(&name)? {
            Named::Data(r) => target.write_register(r, value)?,
            Named::System(r) => target.write_system_register(r, value)?,
        },
        Command::Dump { address, length, size, fc } => {
            let mut data = vec![0; usize::try_from(length)?];
            target.read_memory(Address(address), fc, size.into(), &mut data)?;
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
        Command::Write { address, bytes, size, fc } => {
            target.write_memory(Address(address), fc, size.into(), &bytes)?
        }
        Command::Loopback { rounds } => {
            let words = loopback_words();
            let started = std::time::Instant::now();
            for round in 0..rounds {
                let responses = target.pod().transfer(&words)?;
                if responses.len() != words.len() {
                    bail!("loopback: {} responses to {} words", responses.len(), words.len());
                }
                if let Some((n, word, response)) = loopback_mismatch(&words, &responses) {
                    bail!(
                        "loopback: round {round}, word {n}: sent {word:04x}, got {}:{:04x}",
                        response >> 16,
                        response & 0xFFFF
                    );
                }
            }
            let total = words.len() * usize::try_from(rounds)?;
            println!("loopback: {total} words ok in {:.3} s", started.elapsed().as_secs_f64());
        }
        Command::Xfer { words } => {
            for (word, response) in words.iter().zip(target.pod().transfer(&words)?) {
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
    use super::FunctionCode;
    use super::Named;
    use super::Register;
    use super::SystemRegister;
    use super::function_code;
    use super::loopback_mismatch;
    use super::loopback_words;
    use super::number;
    use super::register;

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
        assert_eq!(register("SP").unwrap(), Named::Data(Register::A7));
        assert_eq!(register("sfc").unwrap(), Named::System(SystemRegister::Sfc));
        assert!(register("d8").is_err());
        assert!(register("x1").is_err());
        assert!(register("").is_err());
        for r in Register::ALL {
            assert_eq!(register(&r.to_string()).unwrap(), Named::Data(r));
        }
    }

    #[test]
    fn loopback_patterns_cover_every_bit_both_ways() {
        let words = loopback_words();
        for bit in 0..u16::BITS {
            assert!(words.iter().any(|w| w & (1 << bit) != 0));
            assert!(words.iter().any(|w| w & (1 << bit) == 0));
        }
    }

    #[test]
    fn loopback_finds_the_first_bad_word() {
        let words = [0x1234, 0x5678, 0x9ABC];
        assert_eq!(loopback_mismatch(&words, &[0x1234, 0x5678, 0x9ABC]), None);
        assert_eq!(loopback_mismatch(&words, &[0x1234, 0x1_5678, 0x0000]), Some((1, 0x5678, 0x1_5678)));
    }

    #[test]
    fn function_codes() {
        assert_eq!(function_code("5").unwrap(), FunctionCode::SUPERVISOR_DATA);
        assert_eq!(function_code("7").unwrap(), FunctionCode::CPU_SPACE);
        assert!(function_code("8").is_err());
        assert!(function_code("0xD").is_err());
    }
}
