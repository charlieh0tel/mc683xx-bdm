//! picotool's reset interface (pico-sdk `reset_interface.h`): a vendor
//! interface with no endpoints whose request reboots the chip into its
//! USB bootloader, so `picotool -f` and `bdm bootsel` need no BOOTSEL
//! button.

use embassy_rp::rom_data::reset_to_usb_boot;
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
            // Does not return: the host sees the pod leave the bus.
            reset_to_usb_boot(0, u32::from(request.value & DISABLE_MASK));
        }
        None
    }
}
