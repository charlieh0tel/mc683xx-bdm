//! FREEZE, watched in a task of its own: its level and a count of its
//! edges, so the pod can tell a new entry into background mode from the
//! one it already read ATEMP for (`pod_proto::entry`).

use embassy_futures::select::Either;
use embassy_futures::select::select;
use embassy_rp::gpio::Input;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Duration;
use embassy_time::Timer;
use portable_atomic::AtomicBool;
use portable_atomic::AtomicU32;
use portable_atomic::Ordering;

/// How often the level is read even without an edge interrupt: covers
/// an edge between reading the level and arming the next wait.
const REFRESH: Duration = Duration::from_millis(10);

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

    /// Takes in a level read from the pin: one edge, or two coalesced,
    /// if it moved; any edge counts.
    fn update(&self, high: bool, edge: bool) {
        if edge || high != self.is_high() {
            self.edges.fetch_add(1, Ordering::AcqRel);
        }
        self.high.store(high, Ordering::Release);
        self.changed.signal(());
    }
}

/// Keeps [`FREEZE`] current, for ever.  At start the level is taken
/// without counting an edge: the pod did not see the CPU enter.
pub(crate) async fn watch(mut pin: Input<'static>) -> ! {
    FREEZE.high.store(pin.is_high(), Ordering::Release);
    loop {
        let edge = matches!(select(pin.wait_for_any_edge(), Timer::after(REFRESH)).await, Either::First(()));
        FREEZE.update(pin.is_high(), edge);
    }
}
