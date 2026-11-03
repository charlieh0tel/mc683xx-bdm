//! The BDM serial link on PIO0's state machine 0 (PLAN.md, PIO
//! program): 17-bit full-duplex words, DSCLK idling high, and `DSI_EN`,
//! which the program drops itself after the last word before the CPU
//! leaves background mode.  FREEZE gates DSI in hardware as well
//! (PLAN.md, Hardware).

use core::convert::Infallible;

use embassy_rp::Peri;
use embassy_rp::gpio::Level;
use embassy_rp::peripherals::PIN_2;
use embassy_rp::peripherals::PIN_3;
use embassy_rp::peripherals::PIN_7;
use embassy_rp::peripherals::PIN_11;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::Common;
use embassy_rp::pio::Config;
use embassy_rp::pio::Direction;
use embassy_rp::pio::Pin;
use embassy_rp::pio::ShiftConfig;
use embassy_rp::pio::ShiftDirection;
use embassy_rp::pio::StateMachine;
use fixed::FixedU32;
use pod_proto::dsclk::Divider;
use pod_proto::session::Link;

/// Bits in a serial word.
const WORD_BITS: u32 = 17;
/// The OSR shifts left, so a word goes in left-justified.
const LEFT_JUSTIFY: u32 = u32::BITS - WORD_BITS;
/// The bit after a word in the OSR: set to drop `DSI_EN` once the word is
/// out.
const RELEASE_DSI: u32 = 1 << (LEFT_JUSTIFY - 1);
/// The 17 bits shifted in.
const WORD_MASK: u32 = (1 << WORD_BITS) - 1;
/// The OSR and ISR are 32 bits; the program pulls and pushes itself.
const SHIFT_THRESHOLD: u8 = 32;

/// The PIO's pins (PLAN.md, Hardware): DSCLK (BKPT) on GP7, DSI on GP3,
/// DSO on GP2, and `DSI_EN` on GP11, high to let FREEZE enable DSI.
pub(crate) struct PioLink<'d> {
    sm: StateMachine<'d, PIO0, 0>,
    dsclk: Pin<'d, PIO0>,
    dsi_enable: Pin<'d, PIO0>,
}

/// The pins [`PioLink::new`] takes over.
pub(crate) struct LinkPins<'d> {
    /// BKPT/DSCLK.
    pub(crate) dsclk: Peri<'d, PIN_7>,
    /// IFETCH/DSI.
    pub(crate) dsi: Peri<'d, PIN_3>,
    /// IPIPE/DSO.
    pub(crate) dso: Peri<'d, PIN_2>,
    /// `DSI_EN`.
    pub(crate) dsi_enable: Peri<'d, PIN_11>,
}

impl<'d> PioLink<'d> {
    /// Loads the program and starts the state machine with DSCLK high
    /// and DSI off.
    pub(crate) fn new(
        common: &mut Common<'d, PIO0>,
        mut sm: StateMachine<'d, PIO0, 0>,
        pins: LinkPins<'d>,
        divider: Divider,
    ) -> Self {
        // Each phase is 8 cycles; DSO is sampled on the low phase's last
        // cycle, just before DSCLK rises.  The high phase's delay is on
        // the loop's back edge only, so after the 17th rise the OSR's
        // next bit, RELEASE_DSI, is checked at once: if set, `set pins, 0`
        // drops DSI_EN 3 cycles after the rise
        // (PLAN.md, PIO program).
        let program = pio::pio_asm!(
            ".side_set 1 opt",
            ".wrap_target",
            "    pull block",
            "    set x, 16",
            "bit:",
            "    out pins, 1     side 0 [6]",
            "    in  pins, 1",
            "    jmp !x last     side 1",
            "    jmp x-- bit            [6]",
            "last:",
            "    out y, 1",
            "    jmp !y keep",
            "    set pins, 0",
            "keep:",
            "    push block",
            ".wrap",
        );
        let dsclk = common.make_pio_pin(pins.dsclk);
        let dsi = common.make_pio_pin(pins.dsi);
        let dso = common.make_pio_pin(pins.dso);
        let dsi_enable = common.make_pio_pin(pins.dsi_enable);
        let shift =
            ShiftConfig { threshold: SHIFT_THRESHOLD, direction: ShiftDirection::Left, auto_fill: false };
        let mut config = Config::default();
        config.use_program(&common.load_program(&program.program), &[&dsclk]);
        config.set_out_pins(&[&dsi]);
        config.set_in_pins(&[&dso]);
        config.set_set_pins(&[&dsi_enable]);
        config.shift_out = shift;
        config.shift_in = shift;
        config.clock_divider = FixedU32::from_bits(divider.bits);
        sm.set_config(&config);
        sm.set_pins(Level::High, &[&dsclk]);
        sm.set_pins(Level::Low, &[&dsi_enable]);
        sm.set_pin_dirs(Direction::Out, &[&dsclk, &dsi, &dsi_enable]);
        sm.set_pin_dirs(Direction::In, &[&dso]);
        sm.set_enable(true);
        Self { sm, dsclk, dsi_enable }
    }

    /// Drives DSCLK (BKPT) between words: low to request a halt.
    pub(crate) fn set_dsclk(&mut self, level: Level) {
        self.sm.set_pins(level, &[&self.dsclk]);
    }

    /// Sets `DSI_EN`: DSI is driven while it is high and FREEZE is.
    pub(crate) fn set_dsi_enabled(&mut self, enabled: bool) {
        let level = if enabled { Level::High } else { Level::Low };
        self.sm.set_pins(level, &[&self.dsi_enable]);
    }

    /// Changes DSCLK's rate.
    pub(crate) fn set_divider(&mut self, divider: Divider) {
        self.sm.set_clock_divider(FixedU32::from_bits(divider.bits));
    }

    async fn shift(&mut self, osr: u32) -> u32 {
        self.sm.tx().wait_push(osr).await;
        self.sm.rx().wait_pull().await & WORD_MASK
    }
}

impl Link for PioLink<'_> {
    type Error = Infallible;

    async fn transfer(&mut self, word: u16) -> Result<u32, Infallible> {
        Ok(self.shift(u32::from(word) << LEFT_JUSTIFY).await)
    }

    async fn transfer_exit(&mut self, word: u16) -> Result<u32, Infallible> {
        Ok(self.shift(u32::from(word) << LEFT_JUSTIFY | RELEASE_DSI).await)
    }
}
