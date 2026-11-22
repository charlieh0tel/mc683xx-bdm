//! The pod's board: the Pico's pins, ADC and PIO link around the target
//! connector, as `mc683xx_bdm_proto::pod::Board`.  The request rules are
//! `mc683xx_bdm_proto::pod`'s.

use core::convert::Infallible;

use embassy_rp::adc::Adc;
use embassy_rp::adc::Async;
use embassy_rp::adc::Channel;
use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::gpio::Input;
use embassy_rp::gpio::Level;
use embassy_rp::gpio::Output;
use embassy_time::Duration;
use embassy_time::Timer;
use embassy_time::with_timeout;
use mc683xx_bdm_proto::dsclk::divider;
use mc683xx_bdm_proto::pod::Board;
use mc683xx_bdm_proto::pod::VddError;
use mc683xx_bdm_proto::session::Link;

use crate::freeze::FREEZE;
use crate::link::PioLink;
use crate::reset::spin;

/// How long FREEZE may take after BKPT, or after RESET releases, and
/// to fall after CALL: long against stacking's bus cycles, a bus
/// monitor's timeout included, and against FREEZE's refresh.
const FREEZE_TIMEOUT: Duration = Duration::from_millis(100);
/// RESET held low by the pod: long against the SIM's reset timing
/// (SIMRM), short against a person.
const RESET_PULSE: Duration = Duration::from_millis(10);
/// BERR held low by the pod: many bus clocks at any system clock, so the
/// CPU samples it and ends the cycle.
const BERR_PULSE: Duration = Duration::from_micros(10);
/// Each phase of a lone DSCLK edge: long against the CPU's 2-clock
/// minimum at any system clock (MC68331UM Table A-7).
const EDGE_PHASE: Duration = Duration::from_micros(10);
/// From GP10 rising to U1 driving BKPT/DSCLK: long against Q3 switching
/// and U1's output enable (SCLS378P 5.6), so the first DSCLK fall after
/// [`Board::rest`] reaches the target whole; short against a request.
const DSCLK_ENABLE_SETTLE: Duration = Duration::from_micros(1);
/// The ADC: 12 bits against the Pico's 3.3 V reference.
const ADC_FULL_SCALE: u32 = 1 << 12;
const ADC_REFERENCE_MV: u32 = 3300;
/// VDD reaches the ADC through a 100 kOhm / 100 kOhm divider (PLAN.md,
/// Hardware): 5.5 V reads as 2.75 V.
const VDD_DIVIDER: u32 = 2;

/// U1's BKPT/DSCLK output enable, shown on the Pico's LED: the two
/// pins change together, so the LED cannot drift from U1.
struct DsclkEnable<'d> {
    /// Q3's gate: high enables U1's output (its OE is pulled up to
    /// target VDD and Q3 pulls it low).
    gate: Output<'d>,
    /// Lit while U1 drives BKPT/DSCLK, when a pod restart could glitch
    /// BKPT: dark, the pod is safe to unplug.
    led: Output<'d>,
}

impl DsclkEnable<'_> {
    fn is_on(&self) -> bool {
        self.gate.is_set_high()
    }

    fn set(&mut self, on: bool) {
        let level = Level::from(on);
        self.gate.set_level(level);
        self.led.set_level(level);
    }
}

/// The pod's side of the target connector.
pub(crate) struct PicoBoard<'d> {
    link: PioLink<'d>,
    dsclk_enable: DsclkEnable<'d>,
    /// U1 turned off by [`Board::rest`] with the link driven: the next
    /// word turns it back on.
    resting: bool,
    /// Q1's gate: high pulls RESET low.
    reset_drive: Output<'d>,
    /// Q2's gate: high pulls BERR low.
    berr_drive: Output<'d>,
    reset: Input<'d>,
    ds: Input<'d>,
    berr: Input<'d>,
    adc: Adc<'d, Async>,
    vdd: Channel<'d>,
}

/// The pod's pins, bundled for [`PicoBoard::new`].
pub(crate) struct Lines<'d> {
    /// See [`DsclkEnable`].
    pub(crate) dsclk_enable: Output<'d>,
    /// See [`DsclkEnable`].
    pub(crate) led: Output<'d>,
    /// See [`PicoBoard`].
    pub(crate) reset_drive: Output<'d>,
    /// See [`PicoBoard`].
    pub(crate) berr_drive: Output<'d>,
    /// RESET, low in reset.
    pub(crate) reset: Input<'d>,
    /// DS, low during a bus cycle's data phase.
    pub(crate) ds: Input<'d>,
    /// BERR, low on a bus error.
    pub(crate) berr: Input<'d>,
}

impl<'d> PicoBoard<'d> {
    pub(crate) fn new(link: PioLink<'d>, lines: Lines<'d>, adc: Adc<'d, Async>, vdd: Channel<'d>) -> Self {
        Self {
            link,
            dsclk_enable: DsclkEnable { gate: lines.dsclk_enable, led: lines.led },
            resting: false,
            reset_drive: lines.reset_drive,
            berr_drive: lines.berr_drive,
            reset: lines.reset,
            ds: lines.ds,
            berr: lines.berr,
            adc,
            vdd,
        }
    }

    fn bkpt_low(&mut self) {
        self.enable_dsclk();
        self.link.set_dsclk(Level::Low);
    }

    /// Turns U1's BKPT/DSCLK output on, at DSCLK's level as it is, and
    /// waits for it if it was off.
    fn enable_dsclk(&mut self) {
        self.resting = false;
        if !self.dsclk_enable.is_on() {
            self.dsclk_enable.set(true);
            spin(DSCLK_ENABLE_SETTLE);
        }
    }

    /// Before a word or a lone edge: U1 back on if [`Board::rest`]
    /// turned it off, DSCLK still high from then.
    fn wake(&mut self) {
        if self.resting {
            self.enable_dsclk();
        }
    }

    /// Runs `first`, then waits for FREEZE, all within
    /// [`FREEZE_TIMEOUT`].  Then DSI goes on; on a timeout BKPT is
    /// released.
    async fn wait_freeze(&mut self, first: impl AsyncFnOnce(&mut Self)) -> bool {
        let frozen = with_timeout(FREEZE_TIMEOUT, async {
            first(self).await;
            FREEZE.wait_high().await;
        })
        .await
        .is_ok();
        if frozen {
            self.link.set_dsi_enabled(true);
        } else {
            self.link.set_dsclk(Level::High);
        }
        frozen
    }
}

impl Link for PicoBoard<'_> {
    type Error = Infallible;

    async fn transfer(&mut self, word: u16) -> Result<u32, Infallible> {
        self.wake();
        self.link.transfer(word).await
    }

    async fn transfer_exit(&mut self, word: u16) -> Result<u32, Infallible> {
        self.wake();
        self.link.transfer_exit(word).await
    }

    fn clock_us(&self) -> u32 {
        self.link.clock_us()
    }

    fn freeze_high(&self) -> bool {
        self.link.freeze_high()
    }
}

impl Board for PicoBoard<'_> {
    fn frozen(&self) -> bool {
        FREEZE.is_high()
    }

    fn freeze_edges(&self) -> u32 {
        FREEZE.edges()
    }

    fn reset_high(&self) -> bool {
        self.reset.is_high()
    }

    fn ds_high(&self) -> bool {
        self.ds.is_high()
    }

    fn berr_high(&self) -> bool {
        self.berr.is_high()
    }

    async fn vdd_mv(&mut self) -> Result<u16, VddError> {
        let raw = self.adc.read(&mut self.vdd).await.map_err(|_| VddError)?;
        let mv = u32::from(raw) * ADC_REFERENCE_MV * VDD_DIVIDER / ADC_FULL_SCALE;
        Ok(u16::try_from(mv).unwrap_or(u16::MAX))
    }

    /// Raises DSCLK only while U1 is off (since boot, or since
    /// [`Board::rest`], DSCLK already high): with U1 on after a halt
    /// BKPT is held low, and a rising edge now would be a bit to the
    /// CPU, putting every later word out of frame.
    fn drive_link(&mut self) {
        if !self.dsclk_enable.is_on() {
            self.link.set_dsclk(Level::High);
            self.enable_dsclk();
        }
        self.link.set_dsi_enabled(true);
    }

    /// R3 holds BKPT high once U1 is off; GP9 low (BKPT held low) keeps
    /// it on.
    fn rest(&mut self) {
        if self.dsclk_enable.is_on() && self.link.dsclk_high() {
            self.dsclk_enable.set(false);
            self.resting = true;
        }
    }

    /// BKPT low until FREEZE; BKPT stays low after, and the first word's
    /// first rising edge releases it (PLAN.md, Protocol summary).
    async fn halt(&mut self) -> bool {
        self.bkpt_low();
        self.wait_freeze(async |_| {}).await
    }

    async fn reset_halt(&mut self) -> bool {
        self.link.set_dsi_enabled(false);
        self.bkpt_low();
        self.reset_drive.set_high();
        Timer::after(RESET_PULSE).await;
        self.reset_drive.set_low();
        self.wait_freeze(async |board| board.reset.wait_for_high().await).await
    }

    async fn left_background(&mut self, edges: u32) -> bool {
        with_timeout(FREEZE_TIMEOUT, FREEZE.wait_left(edges)).await.is_ok()
    }

    async fn reset_run(&mut self) -> bool {
        self.link.set_dsi_enabled(false);
        self.link.set_dsclk(Level::High);
        self.enable_dsclk();
        self.reset_drive.set_high();
        Timer::after(RESET_PULSE).await;
        self.reset_drive.set_low();
        // The target may hold RESET longer; the reply waits for it, within
        // the halt's bound.
        with_timeout(FREEZE_TIMEOUT, self.reset.wait_for_high()).await.is_ok()
    }

    fn version(&self) -> &'static str {
        concat!(env!("CARGO_PKG_VERSION"), " ", env!("GIT_VERSION"))
    }

    async fn pulse_berr(&mut self) {
        self.berr_drive.set_high();
        Timer::after(BERR_PULSE).await;
        self.berr_drive.set_low();
    }

    fn hold(&mut self) {
        self.bkpt_low();
    }

    async fn clock_bit(&mut self) {
        self.wake();
        self.link.set_dsclk(Level::Low);
        Timer::after(EDGE_PHASE).await;
        self.link.set_dsclk(Level::High);
        Timer::after(EDGE_PHASE).await;
    }

    fn set_clock(&mut self, dsclk_hz: u32) -> u32 {
        let divider = divider(clk_sys_freq(), dsclk_hz);
        self.link.set_divider(divider);
        divider.dsclk_hz
    }

    fn dsclk_hz(&self) -> u32 {
        self.link.dsclk_hz()
    }
}
