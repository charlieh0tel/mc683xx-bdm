//! The chip's hardware watchdog (PLAN.md, Firmware, Watchdog), fed from
//! a task of its own by `pod_proto::watchdog`'s rule: while the pod
//! waits for a request, and while a request makes progress.  A panic
//! (`panic`) or a request stuck without a link word starves it, and the
//! chip resets.

use embassy_rp::Peri;
use embassy_rp::peripherals::WATCHDOG;
#[cfg(feature = "rtt")]
use embassy_rp::watchdog::ResetReason;
use embassy_rp::watchdog::Watchdog;
use embassy_time::Duration;
use embassy_time::Timer;
use pod_proto::watchdog::Feeder;
use portable_atomic::AtomicBool;
use portable_atomic::AtomicU32;
use portable_atomic::Ordering;

/// Unfed this long, the chip resets: within the RP2040's limit (2^24
/// ticks of 1 us, counted twice each: RP2040-E1), and long against any
/// legitimate gap between link words: a `ResetHalt`'s 110 ms, or one word
/// at the slowest DSCLK, 143 ms.
const TIMEOUT: Duration = Duration::from_secs(8);
/// How often the rule is checked: a check that falls in a gap between
/// words misses one feed of the several in [`TIMEOUT`].
const FEED_PERIOD: Duration = Duration::from_secs(1);

/// What the serve loop is doing, as the feeder reads it.
pub(crate) struct Activity {
    /// A request is being carried out.
    busy: AtomicBool,
    /// Link words and request ends, wrapping.
    progress: AtomicU32,
}

/// The pod's one serve loop.
pub(crate) static ACTIVITY: Activity = Activity { busy: AtomicBool::new(false), progress: AtomicU32::new(0) };

impl Activity {
    /// A request is carried out until the guard drops.
    pub(crate) fn request(&'static self) -> Busy {
        self.busy.store(true, Ordering::Release);
        Busy(self)
    }

    /// A word went over the link.
    pub(crate) fn step(&self) {
        self.progress.fetch_add(1, Ordering::AcqRel);
    }
}

/// While it lives, a request is being carried out.
pub(crate) struct Busy(&'static Activity);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.step();
        self.0.busy.store(false, Ordering::Release);
    }
}

/// Starts the watchdog and feeds it, for ever.  It pauses while a
/// debugger halts the core, so a panic under probe-rs (rtt) stays for
/// inspection.
pub(crate) async fn feed(watchdog: Peri<'static, WATCHDOG>) -> ! {
    let mut watchdog = Watchdog::new(watchdog);
    #[cfg(feature = "rtt")]
    if watchdog.reset_reason() == Some(ResetReason::TimedOut) {
        defmt::warn!("reset by the watchdog");
    }
    watchdog.pause_on_debug(true);
    watchdog.start(TIMEOUT);
    let mut feeder = Feeder::new();
    loop {
        Timer::after(FEED_PERIOD).await;
        if feeder.check(ACTIVITY.busy.load(Ordering::Acquire), ACTIVITY.progress.load(Ordering::Acquire)) {
            watchdog.feed(TIMEOUT);
        }
    }
}
