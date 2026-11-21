//! picotool's reset interface (pico-sdk `reset_interface.h`): a vendor
//! interface with no endpoints whose request reboots the chip into its
//! USB bootloader, so `picotool -f` and `bdm bootsel` need no BOOTSEL
//! button.  Its release of BKPT/DSCLK before the reboot serves the
//! panic handler too (`panic`).

use cortex_m::asm::delay;
use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::pac::SIO;
use embassy_rp::rom_data::reset_to_usb_boot;
use embassy_time::Duration;
use embassy_usb::Handler;
use embassy_usb::control::OutResponse;
use embassy_usb::control::Recipient;
use embassy_usb::control::Request;
use embassy_usb::control::RequestType;
use embassy_usb::types::InterfaceNumber;

/// The interface's subclass and protocol, under the vendor class.
pub(crate) const SUBCLASS: u8 = 0x00;
pub(crate) const PROTOCOL: u8 = 0x01;
/// Reboot into the bootloader; wValue's low bits are the boot ROM's
/// interface-disable mask (0: mass storage and PICOBOOT both).
const REQUEST_BOOTSEL: u8 = 0x01;
const DISABLE_MASK: u16 = 0x7F;
/// GP10, Q3's gate: U1's BKPT/DSCLK output enable (PLAN.md, pin table).
const DSCLK_ENABLE_GPIO: u32 = 10;
/// U1's output is off in 10 ns (SCLS378P 5.6); R3 has BKPT high long
/// before this.
const DSCLK_RELEASE: Duration = Duration::from_micros(10);
const MICROS_PER_SECOND: u64 = 1_000_000;

/// Turns U1 off before a chip reset, so R3 holds BKPT high through it: as
/// the chip resets, GP9 (U1's input) may fall before GP10 turns U1 off, a
/// BKPT low pulse to a running CPU or a lone DSCLK edge to a halted one.
/// The board owns GP10; this is its last use.  A busy wait on the
/// processor's cycles, which runs with interrupts off, as in a panic.
pub(crate) fn release_dsclk() {
    SIO.gpio_out(0).value_clr().write_value(1 << DSCLK_ENABLE_GPIO);
    let cycles = u64::from(clk_sys_freq()) * DSCLK_RELEASE.as_micros() / MICROS_PER_SECOND;
    delay(u32::try_from(cycles).unwrap_or(u32::MAX));
}

/// The interface's request handler.
pub(crate) struct ResetInterface {
    /// The interface the requests name in wIndex.
    pub(crate) interface: InterfaceNumber,
}

impl ResetInterface {
    /// Until the builder numbers the interface.
    pub(crate) const fn new() -> Self {
        Self { interface: InterfaceNumber(0) }
    }
}

impl Handler for ResetInterface {
    fn control_out(&mut self, request: Request, _data: &[u8]) -> Option<OutResponse> {
        let ours = matches!(request.request_type, RequestType::Class | RequestType::Vendor)
            && request.recipient == Recipient::Interface
            && request.index == u16::from(u8::from(self.interface));
        if ours && request.request == REQUEST_BOOTSEL {
            release_dsclk();
            // Does not return: the host sees the pod leave the bus.
            reset_to_usb_boot(0, u32::from(request.value & DISABLE_MASK));
        }
        None
    }
}
