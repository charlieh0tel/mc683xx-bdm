//! The production build's panic (without `rtt`): U1 off first, as before
//! bootsel's reboot (`reset`), so the chip reset that follows cannot
//! glitch BKPT low; then a halt, which starves the watchdog, and the chip
//! resets (PLAN.md, Firmware, Watchdog).  With `rtt`, panic-probe's
//! instead, for a probe to inspect.

use core::panic::PanicInfo;
use core::sync::atomic::Ordering;
use core::sync::atomic::compiler_fence;

use cortex_m::interrupt;

use crate::reset::release_dsclk;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // No interrupt handler runs again, to touch the pins or wake a task.
    interrupt::disable();
    release_dsclk();
    loop {
        compiler_fence(Ordering::SeqCst);
    }
}
