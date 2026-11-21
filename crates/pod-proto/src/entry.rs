//! When the pod reads ATEMP: first after each entry into background
//! mode, before any other command, since the first command after entry
//! overwrites it (CPU32RM 7.2.3).  The pod counts FREEZE's edges; a
//! count that moved since the last read means a new entry; how the
//! firmware counts them is [`freeze_edges`].

use crate::usb::Atemp;

/// What the pod knows of ATEMP for the CPU's current stay in background
/// mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryWatch {
    /// FREEZE's edge count when ATEMP was last read, and what was read.
    read: Option<(u32, Atemp)>,
}

impl Default for EntryWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl EntryWatch {
    /// Nothing read yet.
    #[must_use]
    pub const fn new() -> Self {
        Self { read: None }
    }

    /// Whether ATEMP must be read now, before any other command, and if
    /// so how to take the value: `Atemp::Fresh` after an entry the pod
    /// saw (FREEZE has had an edge since the pod started), `Atemp::Stale`
    /// when the CPU was halted from the start.  `edges` is FREEZE's edge
    /// count.
    #[must_use]
    pub fn read_needed(&self, frozen: bool, edges: u32) -> Option<fn(u32) -> Atemp> {
        if !frozen || matches!(self.read, Some((at, _)) if at == edges) {
            return None;
        }
        Some(if edges == 0 { Atemp::Stale } else { Atemp::Fresh })
    }

    /// ATEMP has been read, at FREEZE edge count `edges`.
    pub const fn record(&mut self, edges: u32, atemp: Atemp) {
        self.read = Some((edges, atemp));
    }

    /// ATEMP for the current stay, as a reply carries it.
    #[must_use]
    pub fn atemp(&self, frozen: bool, edges: u32) -> Atemp {
        match self.read {
            Some((at, atemp)) if frozen && at == edges => atemp,
            _ => Atemp::None,
        }
    }
}

/// FREEZE's edge latches, one per direction, set since they were last
/// cleared: the firmware's pin latches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FreezeLatches {
    /// A rising edge: the CPU entered background mode.
    pub rose: bool,
    /// A falling edge: the CPU left it.
    pub fell: bool,
}

/// FREEZE's edges between two reads of its level, `was_high` then `high`,
/// from the latches set in between.  A latch holds one edge of its
/// direction, and an edge while the latches are cleared to arm the next
/// wait sets none; each such loss changes the count's parity, so where
/// it disagrees with the levels, one edge more is counted.  Two lost
/// edges together are not seen.
#[must_use]
pub fn freeze_edges(was_high: bool, high: bool, latches: FreezeLatches) -> u32 {
    let latched = u32::from(latches.rose) + u32::from(latches.fell);
    if (latched % 2 == 1) == (was_high != high) { latched } else { latched + 1 }
}

#[cfg(test)]
mod tests {
    use super::Atemp;
    use super::EntryWatch;
    use super::FreezeLatches;
    use super::freeze_edges;

    const NONE: FreezeLatches = FreezeLatches { rose: false, fell: false };
    const ROSE: FreezeLatches = FreezeLatches { rose: true, fell: false };
    const FELL: FreezeLatches = FreezeLatches { rose: false, fell: true };
    const BOTH: FreezeLatches = FreezeLatches { rose: true, fell: true };

    #[test]
    fn latched_edges_count_as_they_are() {
        assert_eq!(freeze_edges(false, false, NONE), 0);
        assert_eq!(freeze_edges(false, true, ROSE), 1);
        assert_eq!(freeze_edges(true, false, FELL), 1);
        // GO, then a BGND straight after: left and entered again.
        assert_eq!(freeze_edges(true, true, BOTH), 2);
    }

    #[test]
    fn an_edge_lost_while_arming_is_counted_from_the_level() {
        assert_eq!(freeze_edges(true, false, NONE), 1);
        assert_eq!(freeze_edges(false, true, NONE), 1);
        // Left, latched; entered again while the latches were cleared.
        assert_eq!(freeze_edges(true, true, FELL), 2);
        // Left and entered, latched; left again, lost.
        assert_eq!(freeze_edges(true, false, BOTH), 3);
    }

    #[test]
    fn running_needs_nothing() {
        let watch = EntryWatch::new();
        assert!(watch.read_needed(false, 3).is_none());
        assert_eq!(watch.atemp(false, 3), Atemp::None);
    }

    #[test]
    fn halted_from_the_start_is_stale() {
        let mut watch = EntryWatch::new();
        let take = watch.read_needed(true, 0).expect("a read");
        watch.record(0, take(5));
        assert_eq!(watch.atemp(true, 0), Atemp::Stale(5));
        assert!(watch.read_needed(true, 0).is_none(), "read once");
    }

    #[test]
    fn each_entry_is_read_once_and_fresh() {
        let mut watch = EntryWatch::new();
        let take = watch.read_needed(true, 2).expect("a read");
        watch.record(2, take(1));
        assert_eq!(watch.atemp(true, 2), Atemp::Fresh(1));
        assert!(watch.read_needed(true, 2).is_none());
        // Left and came back between two requests: a new entry.
        let take = watch.read_needed(true, 4).expect("a new read");
        watch.record(4, take(0));
        assert_eq!(watch.atemp(true, 4), Atemp::Fresh(0));
        // Running, or back without a read yet: nothing to report.
        assert_eq!(watch.atemp(false, 5), Atemp::None);
        assert_eq!(watch.atemp(true, 6), Atemp::None);
    }
}
