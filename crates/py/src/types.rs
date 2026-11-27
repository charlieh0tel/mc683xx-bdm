//! What the pod reports, as Python values: the entry source, ATEMP as
//! the pod read it, the target's pins, and how a gdb session ended.

use mc683xx_bdm::gdb;
use mc683xx_bdm_proto::usb;
use pyo3::pyclass;
use pyo3::pymethods;

/// Why the CPU entered background mode (Rust's
/// `mc683xx_target::Entry`; CPU32RM Table 7-2), as the
/// emulator's `mc683xx.Entry`.
#[pyclass(module = "mc683xx_bdm", frozen, eq, hash)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Entry {
    /// BKPT asserted.
    Breakpoint(),
    /// A BGND instruction.
    Background(),
    /// A double bus fault, with the second fault's SSW.
    DoubleBusFault {
        /// The special status word.
        ssw: u16,
    },
    /// An ATEMP Table 7-2 does not list, or one the pod read when it
    /// had not seen the entry.
    Unknown {
        /// ATEMP as read.
        atemp: u32,
    },
}

impl From<mc683xx_target::Entry> for Entry {
    fn from(entry: mc683xx_target::Entry) -> Self {
        match entry {
            mc683xx_target::Entry::Breakpoint => Self::Breakpoint(),
            mc683xx_target::Entry::Background => Self::Background(),
            mc683xx_target::Entry::DoubleBusFault { ssw } => Self::DoubleBusFault { ssw },
            mc683xx_target::Entry::Unknown { atemp } => Self::Unknown { atemp },
        }
    }
}

impl From<Entry> for mc683xx_target::Entry {
    fn from(entry: Entry) -> Self {
        match entry {
            Entry::Breakpoint() => Self::Breakpoint,
            Entry::Background() => Self::Background,
            Entry::DoubleBusFault { ssw } => Self::DoubleBusFault { ssw },
            Entry::Unknown { atemp } => Self::Unknown { atemp },
        }
    }
}

#[pymethods]
impl Entry {
    fn __str__(&self) -> String {
        mc683xx_target::Entry::from(*self).to_string()
    }

    fn __repr__(&self) -> String {
        match self {
            Self::Breakpoint() => "Entry.Breakpoint()".to_owned(),
            Self::Background() => "Entry.Background()".to_owned(),
            Self::DoubleBusFault { ssw } => format!("Entry.DoubleBusFault(ssw={ssw:#06x})"),
            Self::Unknown { atemp } => format!("Entry.Unknown(atemp={atemp:#010x})"),
        }
    }
}

/// ATEMP as the pod read it for the CPU's current stay in background
/// mode (Rust's `mc683xx_bdm_proto::usb::Atemp`).
#[pyclass(module = "mc683xx_bdm", frozen, eq, hash)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Atemp {
    /// The CPU is not in background mode.
    Running(),
    /// Read first after entry: the entry source.
    Fresh {
        /// ATEMP.
        value: u32,
    },
    /// Read when the pod had not seen the entry: not an entry source.
    Stale {
        /// ATEMP.
        value: u32,
    },
    /// In background mode, but the read after entry failed.
    Unread(),
}

impl From<usb::Atemp> for Atemp {
    fn from(atemp: usb::Atemp) -> Self {
        match atemp {
            usb::Atemp::None => Self::Running(),
            usb::Atemp::Fresh(value) => Self::Fresh { value },
            usb::Atemp::Stale(value) => Self::Stale { value },
            usb::Atemp::Unread => Self::Unread(),
        }
    }
}

#[pymethods]
impl Atemp {
    fn __repr__(&self) -> String {
        match self {
            Self::Running() => "Atemp.Running()".to_owned(),
            Self::Fresh { value } => format!("Atemp.Fresh(value={value:#010x})"),
            Self::Stale { value } => format!("Atemp.Stale(value={value:#010x})"),
            Self::Unread() => "Atemp.Unread()".to_owned(),
        }
    }
}

/// The target's pins as the pod reads them (Rust's
/// `mc683xx_bdm_proto::usb::Pins`): each level true when high.
#[pyclass(module = "mc683xx_bdm", frozen, get_all)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pins {
    /// FREEZE high: the CPU is in background mode.
    freeze: bool,
    /// RESET high: not in reset.
    reset: bool,
    /// DS high: no data strobe.
    ds: bool,
    /// BERR high: no bus error.
    berr: bool,
    /// Target VDD, millivolts.
    vdd_mv: u16,
    /// ATEMP for the CPU's current stay in background mode.
    atemp: Atemp,
}

impl From<usb::Pins> for Pins {
    fn from(pins: usb::Pins) -> Self {
        Self {
            freeze: pins.freeze,
            reset: pins.reset,
            ds: pins.ds,
            berr: pins.berr,
            vdd_mv: pins.vdd_mv,
            atemp: pins.atemp.into(),
        }
    }
}

#[pymethods]
impl Pins {
    fn __repr__(&self) -> String {
        let Self { freeze, reset, ds, berr, vdd_mv, atemp } = self;
        let title = |level: &bool| if *level { "True" } else { "False" };
        format!(
            "Pins(freeze={}, reset={}, ds={}, berr={}, vdd_mv={vdd_mv}, atemp={})",
            title(freeze),
            title(reset),
            title(ds),
            title(berr),
            atemp.__repr__()
        )
    }
}

/// How a gdb session ended, as `serve_gdb` returns it; as the
/// emulator's `mc683xx.Ending`.
#[pyclass(module = "mc683xx_bdm", eq, eq_int, hash, frozen, rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Ending {
    /// gdb detached: the target runs on.
    Detached,
    /// gdb killed the target: it stays halted.
    Killed,
    /// gdb closed the connection without detaching or killing: its
    /// breakpoints removed, the target halted.
    HungUp,
}

impl From<gdb::Ending> for Ending {
    fn from(ending: gdb::Ending) -> Self {
        match ending {
            gdb::Ending::Detached => Self::Detached,
            gdb::Ending::Killed => Self::Killed,
            gdb::Ending::HungUp => Self::HungUp,
        }
    }
}
