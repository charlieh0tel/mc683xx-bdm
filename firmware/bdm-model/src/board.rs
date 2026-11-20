//! A pod's board around a model [`Target`]: what `pod_proto::pod` needs
//! to answer requests with no hardware.  FREEZE is the model's state,
//! its edges counted as the model leaves and enters background mode;
//! VDD is fixed; a halt or reset puts the model in background mode with
//! a breakpoint entry.  A reset keeps registers and memory: the model
//! has no reset vectors to fetch.

use core::convert::Infallible;

use mc683xx_target::entry::Entry;
use pod_proto::dsclk::MAX_HZ;
use pod_proto::pod::Board;
use pod_proto::pod::VddError;
use pod_proto::session::Link;

use crate::Target;

/// The VDD the board reports: a 5 V target.
const VDD_MV: u16 = 5000;

/// A response with DSO high for all 17 bits.
const DSO_HIGH: u32 = 0x1_FFFF;

/// A model target behind a pod's board.
#[derive(Debug)]
pub struct ModelBoard<const RAM: usize> {
    target: Target<RAM>,
    /// FREEZE's edges, wrapping.
    edges: u32,
    /// BDM was enabled at the last reset: BKPT halts.  Without it a real
    /// CPU takes BKPT as a breakpoint exception and Halt fails.
    bdm_enabled: bool,
    /// DSO held high, so every response reads as "illegal", as with
    /// the link out of frame.
    dso_high: bool,
}

impl<const RAM: usize> ModelBoard<RAM> {
    /// A board around `target`, its BDM enabled or not.
    #[must_use]
    pub const fn new(target: Target<RAM>, bdm_enabled: bool) -> Self {
        Self { target, edges: 0, bdm_enabled, dso_high: false }
    }

    /// The model, for what a test checks or sets.
    pub const fn target(&mut self) -> &mut Target<RAM> {
        &mut self.target
    }

    /// Holds DSO high, or lets it go: the model still takes each word.
    pub const fn hold_dso_high(&mut self, high: bool) {
        self.dso_high = high;
    }

    /// The running CPU enters background mode on its own (BGND, a
    /// breakpoint, a double bus fault), as FREEZE rises.
    pub const fn enter_background(&mut self, entry: Entry) {
        if !self.target.frozen() {
            self.edges = self.edges.wrapping_add(1);
        }
        self.target.enter_background(entry);
    }

    /// Exchanges a word, counting FREEZE's fall if it ends background
    /// mode.
    fn exchange(&mut self, word: u16, exit: bool) -> u32 {
        let was_frozen = self.target.frozen();
        let bits = self.target.exchange(word, exit);
        if was_frozen && !self.target.frozen() {
            self.edges = self.edges.wrapping_add(1);
        }
        if self.dso_high { DSO_HIGH } else { bits }
    }
}

impl<const RAM: usize> Link for ModelBoard<RAM> {
    type Error = Infallible;

    async fn transfer(&mut self, word: u16) -> Result<u32, Infallible> {
        Ok(self.exchange(word, false))
    }

    async fn transfer_exit(&mut self, word: u16) -> Result<u32, Infallible> {
        Ok(self.exchange(word, true))
    }
}

impl<const RAM: usize> Board for ModelBoard<RAM> {
    fn frozen(&self) -> bool {
        self.target.frozen()
    }

    fn freeze_edges(&self) -> u32 {
        self.edges
    }

    fn reset_high(&self) -> bool {
        true
    }

    fn ds_high(&self) -> bool {
        true
    }

    fn berr_high(&self) -> bool {
        true
    }

    async fn vdd_mv(&mut self) -> Result<u16, VddError> {
        Ok(VDD_MV)
    }

    fn drive_link(&mut self) {}

    async fn halt(&mut self) -> bool {
        if self.bdm_enabled {
            self.enter_background(Entry::Breakpoint);
        }
        self.bdm_enabled
    }

    async fn reset_halt(&mut self) -> bool {
        if self.target.frozen() {
            self.edges = self.edges.wrapping_add(1);
        }
        self.bdm_enabled = true;
        self.edges = self.edges.wrapping_add(1);
        self.target.enter_background(Entry::Breakpoint);
        true
    }

    fn set_clock(&mut self, dsclk_hz: u32) -> u32 {
        dsclk_hz.clamp(1, MAX_HZ)
    }
}
