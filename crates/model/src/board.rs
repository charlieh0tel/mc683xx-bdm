//! A pod's board around a model [`Target`]: what
//! `mc683xx_bdm_proto::pod` needs to answer requests with no hardware.  FREEZE is the model's state,
//! its edges counted as the model leaves and enters background mode;
//! a resumed model runs its program at once (`program`), so a BGND it
//! reaches has FREEZE back up before the pod's next request;
//! VDD is fixed; a halt or reset puts the model in background mode with
//! a breakpoint entry.  Words clocked with the link not driven (as at
//! the pod's boot, or after GO dropped DSI) do not reach the model: DSO
//! reads high, "illegal".  A reset keeps registers and memory: the model
//! has no reset vectors to fetch.

use core::convert::Infallible;

use mc683xx_bdm_proto::bdm::NOP;
use mc683xx_bdm_proto::dsclk::MAX_HZ;
use mc683xx_bdm_proto::pod::Board;
use mc683xx_bdm_proto::pod::VddError;
use mc683xx_bdm_proto::session::Link;
use mc683xx_target::entry::Entry;

use crate::Target;

/// The VDD the board reports: a 5 V target.
const VDD_MV: u16 = 5000;

/// A response with DSO high for all 17 bits.
const DSO_HIGH: u32 = 0x1_FFFF;
/// Bits in a serial word.
const WORD_BITS: u32 = 17;

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
    /// Bits the CPU's serial frame is ahead of the pod's, 0 in frame:
    /// an idle CPU's "command complete" then reads rotated.
    skew: u32,
    /// BKPT/DSCLK and DSI driven (`Board::drive_link`): off at boot, and
    /// DSI dropped by GO's and CALL's last word and by a reset to run.
    driven: bool,
    /// The target holds RESET low: a reset never ends.
    reset_held: bool,
}

impl<const RAM: usize> ModelBoard<RAM> {
    /// A board around `target`, its BDM enabled or not.
    #[must_use]
    pub const fn new(target: Target<RAM>, bdm_enabled: bool) -> Self {
        Self { target, edges: 0, bdm_enabled, dso_high: false, skew: 0, driven: false, reset_held: false }
    }

    /// Holds RESET low from the target's side, or lets it go: the pod's
    /// resets then never end.
    pub const fn hold_reset(&mut self, held: bool) {
        self.reset_held = held;
    }

    /// The model, for what a test checks or sets.
    pub const fn target(&mut self) -> &mut Target<RAM> {
        &mut self.target
    }

    /// Holds DSO high, or lets it go: the model still takes each word.
    pub const fn hold_dso_high(&mut self, high: bool) {
        self.dso_high = high;
    }

    /// Puts the CPU's serial frame `bits` ahead of the pod's, as lone
    /// DSCLK edges do.  Out of frame the model answers a NOP as an idle
    /// CPU would and anything else "illegal".
    pub const fn put_out_of_frame(&mut self, bits: u32) {
        self.skew = bits % WORD_BITS;
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
        if !self.driven {
            return DSO_HIGH;
        }
        // The link drops DSI as an exit word ends, whether or not the CPU
        // leaves.
        self.driven = !exit;
        if self.skew != 0 {
            return if word == NOP { DSO_HIGH & !(1 << (self.skew - 1)) } else { DSO_HIGH };
        }
        let was_frozen = self.target.frozen();
        let bits = self.target.exchange(word, exit);
        if was_frozen && !self.target.frozen() {
            self.edges = self.edges.wrapping_add(1);
            self.target.run();
            if self.target.frozen() {
                self.edges = self.edges.wrapping_add(1);
            }
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
        !self.reset_held
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

    fn drive_link(&mut self) {
        self.driven = true;
    }

    async fn halt(&mut self) -> bool {
        if self.bdm_enabled {
            self.enter_background(Entry::Breakpoint);
            self.driven = true;
        }
        self.bdm_enabled
    }

    async fn reset_halt(&mut self) -> bool {
        if self.reset_held {
            return false;
        }
        if self.target.frozen() {
            self.edges = self.edges.wrapping_add(1);
        }
        self.bdm_enabled = true;
        self.edges = self.edges.wrapping_add(1);
        self.target.enter_background(Entry::Breakpoint);
        self.driven = true;
        true
    }

    async fn left_background(&mut self, edges: u32) -> bool {
        !self.target.frozen() || self.edges != edges
    }

    /// With RESET held the CPU stays in reset: it is not running, but
    /// FREEZE is low and its next run starts with background mode off.
    async fn reset_run(&mut self) -> bool {
        if self.target.frozen() {
            self.edges = self.edges.wrapping_add(1);
        }
        self.bdm_enabled = false;
        self.driven = false;
        self.target.reset_running();
        !self.reset_held
    }

    fn version(&self) -> &'static str {
        concat!("mc683xx-bdm-model ", env!("CARGO_PKG_VERSION"))
    }

    async fn pulse_berr(&mut self) {
        self.target.bus_error();
    }

    fn hold(&mut self) {
        if self.bdm_enabled {
            self.enter_background(Entry::Breakpoint);
        }
    }

    async fn clock_bit(&mut self) {
        self.skew = (self.skew + 1) % WORD_BITS;
    }

    fn set_clock(&mut self, dsclk_hz: u32) -> u32 {
        dsclk_hz.clamp(1, MAX_HZ)
    }
}
