//! The pod's board: the Pico's pins, ADC and PIO link around the target
//! connector, as `pod_proto::pod::Board`.  The request rules are
//! `pod_proto::pod`'s.

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
use pod_proto::dsclk::divider;
use pod_proto::pod::Board;
use pod_proto::pod::VddError;
use pod_proto::session::Link;

use crate::freeze::FREEZE;
use crate::link::PioLink;

/// How long FREEZE may take after BKPT, or after RESET releases.
const FREEZE_TIMEOUT: Duration = Duration::from_millis(100);
/// RESET held low by the pod: long against the SIM's reset timing
/// (SIMRM), short against a person.
const RESET_PULSE: Duration = Duration::from_millis(10);
/// Each phase of a lone DSCLK edge: long against the CPU's 2-clock
/// minimum at any system clock (MC68331UM Table A-7).
const EDGE_PHASE: Duration = Duration::from_micros(10);
/// The ADC: 12 bits against the Pico's 3.3 V reference.
const ADC_FULL_SCALE: u32 = 1 << 12;
const ADC_REFERENCE_MV: u32 = 3300;
/// VDD reaches the ADC through a 100 kOhm / 100 kOhm divider (PLAN.md,
/// Hardware): 5.5 V reads as 2.75 V.
const VDD_DIVIDER: u32 = 2;

/// The pod's side of the target connector.
pub(crate) struct PicoBoard<'d> {
    link: PioLink<'d>,
    /// Q3's gate: high enables U1's BKPT/DSCLK output (its OE
    /// is pulled up to target VDD and Q3 pulls it low).
    dsclk_enable: Output<'d>,
    /// Q1's gate: high pulls RESET low.
    reset_drive: Output<'d>,
    reset: Input<'d>,
    ds: Input<'d>,
    berr: Input<'d>,
    adc: Adc<'d, Async>,
    vdd: Channel<'d>,
}

/// The pod's pins, bundled for [`PicoBoard::new`].
pub(crate) struct Lines<'d> {
    /// See [`PicoBoard`].
    pub(crate) dsclk_enable: Output<'d>,
    /// See [`PicoBoard`].
    pub(crate) reset_drive: Output<'d>,
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
            dsclk_enable: lines.dsclk_enable,
            reset_drive: lines.reset_drive,
            reset: lines.reset,
            ds: lines.ds,
            berr: lines.berr,
            adc,
            vdd,
        }
    }

    fn bkpt_low(&mut self) {
        self.dsclk_enable.set_high();
        self.link.set_dsclk(Level::Low);
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
        self.link.transfer(word).await
    }

    async fn transfer_exit(&mut self, word: u16) -> Result<u32, Infallible> {
        self.link.transfer_exit(word).await
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

    /// Raises DSCLK only while the pod is not driving it: after a halt
    /// it is held low, and a rising edge now would be a bit to the CPU,
    /// putting every later word out of frame.
    fn drive_link(&mut self) {
        if self.dsclk_enable.is_set_low() {
            self.link.set_dsclk(Level::High);
            self.dsclk_enable.set_high();
        }
        self.link.set_dsi_enabled(true);
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

    async fn reset_run(&mut self) {
        self.link.set_dsi_enabled(false);
        self.link.set_dsclk(Level::High);
        self.dsclk_enable.set_high();
        self.reset_drive.set_high();
        Timer::after(RESET_PULSE).await;
        self.reset_drive.set_low();
        // The target may hold RESET longer; the reply waits for it, within
        // the halt's bound.
        let _ = with_timeout(FREEZE_TIMEOUT, self.reset.wait_for_high()).await;
    }

    fn version(&self) -> &'static str {
        concat!(env!("CARGO_PKG_VERSION"), " ", env!("GIT_VERSION"))
    }

    async fn clock_bit(&mut self) {
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
}
