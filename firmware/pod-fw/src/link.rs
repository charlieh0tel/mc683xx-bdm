//! The BDM serial link on PIO0's state machine 0 (PLAN.md, PIO
//! program): 17-bit full-duplex words, DSCLK idling high.

use core::convert::Infallible;

use embassy_rp::Peri;
use embassy_rp::gpio::Level;
use embassy_rp::peripherals::PIN_2;
use embassy_rp::peripherals::PIN_3;
use embassy_rp::peripherals::PIN_4;
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
/// The 17 bits shifted in.
const WORD_MASK: u32 = (1 << WORD_BITS) - 1;
/// The OSR and ISR are 32 bits; the program pulls and pushes itself.
const SHIFT_THRESHOLD: u8 = 32;

/// DSCLK (BKPT) on GP2, DSI on GP3, DSO on GP4 (PLAN.md, Pins).
pub(crate) struct PioLink<'d> {
    sm: StateMachine<'d, PIO0, 0>,
    dsclk: Pin<'d, PIO0>,
}

impl<'d> PioLink<'d> {
    /// Loads the program and starts the state machine with DSCLK high.
    pub(crate) fn new(
        common: &mut Common<'d, PIO0>,
        mut sm: StateMachine<'d, PIO0, 0>,
        dsclk: Peri<'d, PIN_2>,
        dsi: Peri<'d, PIN_3>,
        dso: Peri<'d, PIN_4>,
        divider: Divider,
    ) -> Self {
        // Each phase is 8 cycles; DSO is sampled on the low phase's last
        // cycle, just before DSCLK rises (PLAN.md, PIO program).
        let program = pio::pio_asm!(
            ".side_set 1 opt",
            ".wrap_target",
            "    pull block",
            "    set x, 16",
            "bit:",
            "    out pins, 1     side 0 [6]",
            "    in  pins, 1",
            "    jmp x-- bit     side 1 [7]",
            "    push block",
            ".wrap",
        );
        let dsclk = common.make_pio_pin(dsclk);
        let dsi = common.make_pio_pin(dsi);
        let dso = common.make_pio_pin(dso);
        let shift =
            ShiftConfig { threshold: SHIFT_THRESHOLD, direction: ShiftDirection::Left, auto_fill: false };
        let mut config = Config::default();
        config.use_program(&common.load_program(&program.program), &[&dsclk]);
        config.set_out_pins(&[&dsi]);
        config.set_in_pins(&[&dso]);
        config.set_set_pins(&[&dsclk]);
        config.shift_out = shift;
        config.shift_in = shift;
        config.clock_divider = FixedU32::from_bits(divider.bits);
        sm.set_config(&config);
        sm.set_pins(Level::High, &[&dsclk]);
        sm.set_pin_dirs(Direction::Out, &[&dsclk, &dsi]);
        sm.set_pin_dirs(Direction::In, &[&dso]);
        sm.set_enable(true);
        Self { sm, dsclk }
    }

    /// Drives DSCLK (BKPT) between words: low to request a halt.
    pub(crate) fn set_dsclk(&mut self, level: Level) {
        self.sm.set_pins(level, &[&self.dsclk]);
    }

    /// Changes DSCLK's rate.
    pub(crate) fn set_divider(&mut self, divider: Divider) {
        self.sm.set_clock_divider(FixedU32::from_bits(divider.bits));
    }
}

impl Link for PioLink<'_> {
    type Error = Infallible;

    async fn transfer(&mut self, word: u16) -> Result<u32, Infallible> {
        self.sm.tx().wait_push(u32::from(word) << LEFT_JUSTIFY).await;
        Ok(self.sm.rx().wait_pull().await & WORD_MASK)
    }
}
