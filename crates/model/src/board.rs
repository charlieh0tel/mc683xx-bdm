//! A pod's board around a model [`Target`]: what
//! `mc683xx_bdm_proto::pod` needs to answer requests with no hardware.  FREEZE is the model's state,
//! its edges counted as the model leaves and enters background mode;
//! a resumed model runs its program at once (`program`), so a BGND it
//! reaches has FREEZE back up before the pod's next request;
//! VDD is fixed; a halt or reset puts the model in background mode with
//! a breakpoint entry.  Words clocked with the link not driven (as at
//! the pod's boot, or after GO dropped DSI) do not reach the model: DSO
//! reads high, "illegal".  A reset keeps registers and memory: the model
//! has no reset vectors to fetch.  BKPT/DSCLK's driver (U1) and DSCLK's
//! level between words are modeled as the firmware's: BKPT at the
//! target is high (its pull-up) with the driver off, else DSCLK; a
//! rising edge outside a word is a bit to a halted CPU.

use core::convert::Infallible;

use mc683xx_bdm_proto::bdm::NOP;
use mc683xx_bdm_proto::dsclk::DEFAULT_HZ;
use mc683xx_bdm_proto::dsclk::MAX_HZ;
use mc683xx_bdm_proto::dsclk::MIN_HZ;
use mc683xx_bdm_proto::pod::Board;
use mc683xx_bdm_proto::pod::VddError;
use mc683xx_bdm_proto::session::Link;
use mc683xx_target::Entry;

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
    /// BKPT/DSCLK's driver on.
    u1_on: bool,
    /// DSCLK high between words.
    dsclk_high: bool,
    /// The driver turned off by `Board::rest` with the link driven: the
    /// next word turns it back on.
    resting: bool,
    /// The target holds RESET low: a reset never ends.
    reset_held: bool,
    /// DSCLK's rate, as `Board::set_clock` set it.
    dsclk_hz: u32,
    /// The link's clock: each word takes its time at `dsclk_hz`, a lone
    /// edge [`EDGE_US`].
    clock_us: u32,
    /// Words until the target resets to run on its own (a watchdog, a
    /// reset button), FREEZE falling.
    reset_in: Option<u32>,
    /// The pod's wait for FREEZE after CALL ends before FREEZE falls, as
    /// with stacking slower than the wait.
    freeze_late: bool,
}

/// Microseconds in a second.
const MICROS_PER_SECOND: u32 = 1_000_000;
/// A lone DSCLK edge's two phases, as the pod's.
const EDGE_US: u32 = 20;

impl<const RAM: usize> ModelBoard<RAM> {
    /// A board around `target`, its BDM enabled or not.
    #[must_use]
    pub const fn new(target: Target<RAM>, bdm_enabled: bool) -> Self {
        Self {
            target,
            edges: 0,
            bdm_enabled,
            dso_high: false,
            skew: 0,
            driven: false,
            u1_on: false,
            dsclk_high: true,
            resting: false,
            reset_held: false,
            dsclk_hz: DEFAULT_HZ,
            clock_us: 0,
            reset_in: None,
            freeze_late: false,
        }
    }

    /// Resets the target to run, from its own side, once `words` more
    /// words are out: in the middle of a command, say.
    pub const fn reset_after(&mut self, words: u32) {
        self.reset_in = Some(words);
    }

    /// Makes the pod's wait for FREEZE to fall after CALL end first, as
    /// when stacking outlasts it.
    pub const fn fall_late(&mut self, late: bool) {
        self.freeze_late = late;
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

    /// BKPT/DSCLK's driver on.
    #[must_use]
    pub const fn u1_on(&self) -> bool {
        self.u1_on
    }

    /// BKPT at the target: its pull-up with the driver off, else DSCLK.
    #[must_use]
    pub const fn bkpt_high(&self) -> bool {
        !self.u1_on || self.dsclk_high
    }

    /// The pod's chip resets (a replug, a watchdog): GP9, the driver's
    /// input, falls before GP10 turns the driver off, so with the driver
    /// on BKPT falls, then its pull-up raises it, a bit to a halted CPU.
    /// Then the pod's side is as at its boot.
    pub const fn restart_pod(&mut self) {
        self.set_bkpt_pins(self.u1_on, false);
        self.set_bkpt_pins(false, false);
        self.dsclk_high = true;
        self.driven = false;
        self.resting = false;
    }

    /// Sets the driver and DSCLK between words; BKPT rising at the target
    /// is a bit to a halted CPU, moving its frame on.
    const fn set_bkpt_pins(&mut self, u1_on: bool, dsclk_high: bool) {
        let was_high = self.bkpt_high();
        self.u1_on = u1_on;
        self.dsclk_high = dsclk_high;
        if !was_high && self.bkpt_high() && self.target.frozen() {
            self.skew = (self.skew + 1) % WORD_BITS;
        }
    }

    /// BKPT low: the driver on, DSCLK low.
    const fn bkpt_low(&mut self) {
        self.resting = false;
        self.set_bkpt_pins(true, false);
    }

    /// Before a word or a lone edge: the driver back on if `Board::rest`
    /// turned it off.
    const fn wake(&mut self) {
        if self.resting {
            self.resting = false;
            self.set_bkpt_pins(true, self.dsclk_high);
        }
    }

    /// Exchanges a word, counting FREEZE's fall if it ends background
    /// mode.
    fn exchange(&mut self, word: u16, exit: bool) -> u32 {
        self.clock_us = self.clock_us.wrapping_add(WORD_BITS * MICROS_PER_SECOND / self.dsclk_hz);
        self.wake();
        if !self.driven || !self.u1_on {
            return DSO_HIGH;
        }
        // The word's first rise releases BKPT held low; its last leaves
        // DSCLK high.
        self.dsclk_high = true;
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
        let bits = if self.dso_high { DSO_HIGH } else { bits };
        self.count_down_reset();
        bits
    }

    /// One word nearer the target's own reset.
    fn count_down_reset(&mut self) {
        match self.reset_in {
            Some(0 | 1) => {
                self.reset_in = None;
                if self.target.frozen() {
                    self.edges = self.edges.wrapping_add(1);
                }
                self.target.reset_running();
            }
            Some(words) => self.reset_in = Some(words - 1),
            None => {}
        }
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

    fn clock_us(&self) -> u32 {
        self.clock_us
    }

    fn freeze_high(&self) -> bool {
        self.target.frozen()
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

    /// DSCLK raised only with the driver off, as the firmware's.
    fn drive_link(&mut self) {
        if !self.u1_on {
            self.set_bkpt_pins(false, true);
        }
        self.resting = false;
        self.set_bkpt_pins(true, self.dsclk_high);
        self.driven = true;
    }

    /// R3 holds BKPT high with the driver off; DSCLK low keeps it on.
    fn rest(&mut self) {
        if self.u1_on && self.dsclk_high {
            self.set_bkpt_pins(false, true);
            self.resting = true;
        }
    }

    /// On a timeout BKPT is released, DSCLK high.
    async fn halt(&mut self) -> bool {
        self.bkpt_low();
        if self.bdm_enabled {
            self.enter_background(Entry::Breakpoint);
            self.driven = true;
        } else {
            self.set_bkpt_pins(true, true);
        }
        self.bdm_enabled
    }

    async fn reset_halt(&mut self) -> bool {
        self.resting = false;
        if self.reset_held {
            // BKPT released after the wait for FREEZE.
            self.u1_on = true;
            self.dsclk_high = true;
            return false;
        }
        if self.target.frozen() {
            self.edges = self.edges.wrapping_add(1);
        }
        self.u1_on = true;
        self.dsclk_high = false;
        self.bdm_enabled = true;
        self.edges = self.edges.wrapping_add(1);
        self.target.enter_background(Entry::Breakpoint);
        self.driven = true;
        true
    }

    async fn left_background(&mut self, edges: u32) -> bool {
        !self.freeze_late && (!self.target.frozen() || self.edges != edges)
    }

    /// With RESET held the CPU stays in reset: it is not running, but
    /// FREEZE is low and its next run starts with background mode off.
    async fn reset_run(&mut self) -> bool {
        if self.target.frozen() {
            self.edges = self.edges.wrapping_add(1);
        }
        self.bdm_enabled = false;
        self.driven = false;
        self.resting = false;
        self.u1_on = true;
        self.dsclk_high = true;
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
        self.bkpt_low();
        if self.bdm_enabled {
            self.enter_background(Entry::Breakpoint);
        }
    }

    async fn clock_bit(&mut self) {
        self.wake();
        self.clock_us = self.clock_us.wrapping_add(EDGE_US);
        self.skew = (self.skew + 1) % WORD_BITS;
    }

    fn set_clock(&mut self, dsclk_hz: u32) -> u32 {
        self.dsclk_hz = dsclk_hz.clamp(MIN_HZ, MAX_HZ);
        self.dsclk_hz
    }

    fn dsclk_hz(&self) -> u32 {
        self.dsclk_hz
    }
}
