//! When the pod feeds its hardware watchdog (PLAN.md, Firmware,
//! Watchdog).  Checked once a feed period: the pod feeds while it waits
//! for a request, however long the host is idle, and while the request
//! it is carrying out makes progress: a word over the link, or the
//! request's end.  A request stuck for the watchdog's timeout without
//! either is not fed, and the chip resets.

/// The feeding rule's state between checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Feeder {
    /// The progress count at the last check, wrapping.
    last: u32,
}

impl Default for Feeder {
    fn default() -> Self {
        Self::new()
    }
}

impl Feeder {
    /// Before the first check, at progress 0.
    #[must_use]
    pub const fn new() -> Self {
        Self { last: 0 }
    }

    /// Whether to feed now.  `busy`: a request is being carried out;
    /// `progress`: a count, wrapping, raised at each link word and at
    /// each request's end.
    pub const fn check(&mut self, busy: bool, progress: u32) -> bool {
        let moved = progress != self.last;
        self.last = progress;
        !busy || moved
    }
}

#[cfg(test)]
mod tests {
    use super::Feeder;

    #[test]
    fn idle_feeds_for_ever() {
        let mut feeder = Feeder::new();
        assert!((0..10_000).all(|_| feeder.check(false, 0)));
    }

    #[test]
    fn busy_with_progress_feeds() {
        let mut feeder = Feeder::new();
        assert!(feeder.check(true, 1));
        assert!(feeder.check(true, 2));
        assert!(feeder.check(true, 40));
    }

    #[test]
    fn busy_without_progress_starves() {
        let mut feeder = Feeder::new();
        assert!(feeder.check(true, 5));
        assert!(!feeder.check(true, 5));
        assert!(!feeder.check(true, 5));
        // The request ends: idle again, fed.
        assert!(feeder.check(false, 6));
    }

    #[test]
    fn stuck_from_the_first_check_starves() {
        let mut feeder = Feeder::new();
        assert!(!feeder.check(true, 0));
    }

    #[test]
    fn progress_wraps() {
        let mut feeder = Feeder::new();
        assert!(feeder.check(true, u32::MAX));
        assert!(feeder.check(true, u32::MAX.wrapping_add(1)));
        assert!(!feeder.check(true, 0));
    }

    #[test]
    fn a_stall_straight_after_idle_starves() {
        // A request that started and stalled after an idle check: the
        // busy check sees no progress since.
        let mut feeder = Feeder::new();
        assert!(feeder.check(false, 3));
        assert!(!feeder.check(true, 3));
    }
}
