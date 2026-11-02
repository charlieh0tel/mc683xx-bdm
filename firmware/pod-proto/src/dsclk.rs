//! The PIO clock divider for a DSCLK rate (PLAN.md, PIO program).

/// PIO cycles per DSCLK period: 8 low, 8 high.
pub const CYCLES_PER_BIT: u32 = 16;

/// The rate the pod starts at (PLAN.md, Protocol summary).
pub const DEFAULT_HZ: u32 = 1_000_000;

/// The fastest rate the pod sets until the throughput test measures the
/// margin (PLAN.md, Protocol summary): DSO is sampled 15 PIO cycles
/// after a rise, 234 ns at 4 MHz, against at most about 160 ns: up to 2
/// CLKOUT (119 ns at 16.78 MHz) for the CPU to synchronize the rise and
/// shift DSO (CPU32RM 7.2.7.1), 25 ns DSO delay (MC68331UM Table A-7),
/// and 15 ns through both translators: 8.5 ns into 50 pF for U1 and
/// 4.6 ns for U5 (SCLS378P 5.6, SCES351Y 5.6).
pub const MAX_HZ: u32 = 4_000_000;

/// Fraction bits in the PIO clock divider (8.8 in hardware's 16.8).
const FRACTION_BITS: u32 = 8;
/// The divider's limits as raw 16.8 fixed point: 1.0 to 65535 + 255/256.
const MIN_DIVIDER: u32 = 1 << FRACTION_BITS;
const MAX_DIVIDER: u32 = (1 << 24) - 1;

/// A PIO clock divider and the DSCLK rate it gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Divider {
    /// The divider, 16.8 fixed point, as `FixedU32<U8>::from_bits` takes it.
    pub bits: u32,
    /// The DSCLK rate it gives, rounded down.
    pub dsclk_hz: u32,
}

/// The divider closest to, and no faster than, `dsclk_hz` (at most
/// [`MAX_HZ`]) from a `sys_hz` system clock, held to the divider's
/// range.
#[must_use]
pub fn divider(sys_hz: u32, dsclk_hz: u32) -> Divider {
    let per_bit = u64::from(CYCLES_PER_BIT) * u64::from(dsclk_hz.clamp(1, MAX_HZ));
    let scaled = u64::from(sys_hz) << FRACTION_BITS;
    // Round up, so the rate never exceeds the one asked for.
    let bits = scaled.div_ceil(per_bit);
    let bits = bits.clamp(u64::from(MIN_DIVIDER), u64::from(MAX_DIVIDER));
    let dsclk_hz = scaled / (u64::from(CYCLES_PER_BIT) * bits);
    Divider {
        bits: u32::try_from(bits).expect("clamped to 24 bits"),
        dsclk_hz: u32::try_from(dsclk_hz).expect("at most sys_hz / 16"),
    }
}

#[cfg(test)]
mod tests {
    use super::Divider;
    use super::MAX_HZ;
    use super::divider;

    /// The RP2040's and RP2350's default system clocks.
    const RP2040_HZ: u32 = 125_000_000;
    const RP2350_HZ: u32 = 150_000_000;

    #[test]
    fn one_megahertz() {
        // 125 MHz / 16 / 1 MHz = 7.8125, exactly 2000/256.
        assert_eq!(divider(RP2040_HZ, 1_000_000), Divider { bits: 2000, dsclk_hz: 1_000_000 });
        // 150 MHz / 16 / 1 MHz = 9.375, exactly 2400/256.
        assert_eq!(divider(RP2350_HZ, 1_000_000).bits, 2400);
    }

    #[test]
    fn never_faster_than_asked() {
        for hz in [300_000, 333_333, 1_234_567, 3_000_000, 7_000_000] {
            let d = divider(RP2040_HZ, hz);
            assert!(d.dsclk_hz <= hz, "{hz}: {d:?}");
        }
    }

    #[test]
    fn held_to_max_hz() {
        assert_eq!(divider(RP2040_HZ, 100_000_000), divider(RP2040_HZ, MAX_HZ));
        assert!(divider(RP2040_HZ, 100_000_000).dsclk_hz <= MAX_HZ);
        // 125 MHz / 16 / 4 MHz = 1.953125, exactly 500/256.
        assert_eq!(divider(RP2040_HZ, MAX_HZ), Divider { bits: 500, dsclk_hz: MAX_HZ });
    }

    #[test]
    fn held_to_the_divider_range() {
        // Faster than sys / 16 from a slow system clock: divider 1.0.
        assert_eq!(divider(32_000_000, MAX_HZ), Divider { bits: 256, dsclk_hz: 2_000_000 });
        // Slower than the largest divider allows.
        assert_eq!(divider(RP2040_HZ, 1).bits, (1 << 24) - 1);
        assert_eq!(divider(RP2040_HZ, 0).bits, (1 << 24) - 1);
    }
}
