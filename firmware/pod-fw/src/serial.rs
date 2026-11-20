//! The pod's USB serial number: the chip's unique ID in hex
//! (`pod_proto::usb::serial_number`), so a host can tell pods apart.

use embassy_rp::Peri;
#[cfg(feature = "rp2040")]
use embassy_rp::flash::Blocking;
#[cfg(feature = "rp2040")]
use embassy_rp::flash::Flash;
#[cfg(feature = "rp235xa")]
use embassy_rp::otp;
use embassy_rp::peripherals::FLASH;
use pod_proto::usb::SERIAL_NUMBER_CHARS;
use pod_proto::usb::UNIQUE_ID_BYTES;
use static_cell::ConstStaticCell;

/// The Pico's flash, 2 MB: only the driver's type needs it, as the
/// unique ID is read by command, not from the array.
#[cfg(feature = "rp2040")]
const PICO_FLASH_BYTES: usize = 2 * 1024 * 1024;

/// The serial number, or `None` if the ID cannot be read.  Call once,
/// at boot, before anything else runs: on the RP2040 the flash leaves
/// execute-in-place while its ID is read.
pub(crate) fn serial_number(flash: Peri<'static, FLASH>) -> Option<&'static str> {
    static TEXT: ConstStaticCell<[u8; SERIAL_NUMBER_CHARS]> = ConstStaticCell::new([0; SERIAL_NUMBER_CHARS]);
    let id = unique_id(flash)?;
    Some(pod_proto::usb::serial_number(&id, TEXT.take()))
}

/// The flash chip's unique ID (SPI command 0x4B).
#[cfg(feature = "rp2040")]
fn unique_id(flash: Peri<'static, FLASH>) -> Option<[u8; UNIQUE_ID_BYTES]> {
    let mut id = [0; UNIQUE_ID_BYTES];
    Flash::<_, Blocking, PICO_FLASH_BYTES>::new_blocking(flash).blocking_unique_id(&mut id).ok()?;
    Some(id)
}

/// The OTP chip ID, rows 0-3, most significant byte first.
#[cfg(feature = "rp235xa")]
fn unique_id(_flash: Peri<'static, FLASH>) -> Option<[u8; UNIQUE_ID_BYTES]> {
    otp::get_chipid().ok().map(u64::to_be_bytes)
}
