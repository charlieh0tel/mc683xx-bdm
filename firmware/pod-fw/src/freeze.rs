//! FREEZE, watched in a task of its own: its level and a count of its
//! edges, so the pod can tell a new entry into background mode from the
//! one it already read ATEMP for (`mc683xx_bdm_proto::entry`).

use embassy_futures::select::select;
use embassy_rp::Peri;
use embassy_rp::gpio::Input;
use embassy_rp::gpio::Pin;
use embassy_rp::gpio::Pull;
use embassy_rp::pac::IO_BANK0;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Duration;
use embassy_time::Timer;
use mc683xx_bdm_proto::entry::FreezeLatches;
use mc683xx_bdm_proto::entry::freeze_edges;
use portable_atomic::AtomicBool;
use portable_atomic::AtomicU32;
use portable_atomic::Ordering;

/// How often the level and latches are read even without an edge
/// interrupt: covers an edge between reading the level and arming the
/// next wait, which no interrupt reports.
const REFRESH: Duration = Duration::from_millis(10);
/// Pins per INTR register, each with 4 bits: level low, level high, edge
/// low, edge high (RP2040 and RP2350 datasheets, `IO_BANK0`'s INTR0).
const PINS_PER_INTR: usize = 8;

/// FREEZE as [`watch`] last saw it.
pub(crate) struct Freeze {
    /// High: the CPU is in background mode.
    high: AtomicBool,
    /// Edges seen since the pod started, wrapping.
    edges: AtomicU32,
    /// Raised after each change of `high`.
    changed: Signal<CriticalSectionRawMutex, ()>,
}

/// The one FREEZE line.
pub(crate) static FREEZE: Freeze =
    Freeze { high: AtomicBool::new(false), edges: AtomicU32::new(0), changed: Signal::new() };

impl Freeze {
    pub(crate) fn is_high(&self) -> bool {
        self.high.load(Ordering::Acquire)
    }

    pub(crate) fn edges(&self) -> u32 {
        self.edges.load(Ordering::Acquire)
    }

    /// Waits for FREEZE high.
    pub(crate) async fn wait_high(&self) {
        loop {
            self.changed.reset();
            if self.is_high() {
                return;
            }
            self.changed.wait().await;
        }
    }

    /// Waits for FREEZE low, or for its edge count to move from
    /// `edges`: the CPU left background mode, perhaps entering again.
    pub(crate) async fn wait_left(&self, edges: u32) {
        loop {
            self.changed.reset();
            if !self.is_high() || self.edges() != edges {
                return;
            }
            self.changed.wait().await;
        }
    }

    /// Takes in a level read from the pin and the edges since the last.
    fn update(&self, high: bool, edges: u32) {
        self.edges.fetch_add(edges, Ordering::AcqRel);
        self.high.store(high, Ordering::Release);
        self.changed.signal(());
    }
}

/// Keeps [`FREEZE`] current, for ever, from `pin` (pulled down).  At
/// start the level is taken without counting an edge: the pod did not
/// see the CPU enter.
///
/// Each wait arms the pin's edge interrupt, and arming clears its edge
/// latches (embassy-rp's `InputFuture`).  The interrupt leaves them set,
/// so after each wakeup the latches hold the edges since the arm, read
/// before the next arm clears them; an edge outside them (while
/// arming, or between reading them and the level) shows in the level
/// (`mc683xx_bdm_proto::entry::freeze_edges`).
pub(crate) async fn watch(pin: Peri<'static, impl Pin>) -> ! {
    let gpio = usize::from(pin.pin());
    let mut pin = Input::new(pin, Pull::Down);
    let mut was_high = pin.is_high();
    FREEZE.high.store(was_high, Ordering::Release);
    loop {
        select(pin.wait_for_any_edge(), Timer::after(REFRESH)).await;
        let latched = IO_BANK0.intr(gpio / PINS_PER_INTR).read();
        let latches = FreezeLatches {
            rose: latched.edge_high(gpio % PINS_PER_INTR),
            fell: latched.edge_low(gpio % PINS_PER_INTR),
        };
        let high = pin.is_high();
        FREEZE.update(high, freeze_edges(was_high, high, latches));
        was_high = high;
    }
}
