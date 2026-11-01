//! The pod: a Pico or Pico 2 between USB and a CPU32's BDM connector
//! (PLAN.md).  Requests come in on a vendor-class bulk endpoint and are
//! answered on another (`pod_proto::usb`).

#![no_std]
#![no_main]

mod freeze;
mod link;
mod pod;

#[cfg(feature = "rtt")]
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_rp::adc;
use embassy_rp::adc::Adc;
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::gpio::Input;
use embassy_rp::gpio::Level;
use embassy_rp::gpio::Output;
use embassy_rp::gpio::Pull;
use embassy_rp::peripherals::PIO0;
use embassy_rp::peripherals::USB;
use embassy_rp::pio;
use embassy_rp::pio::Pio;
use embassy_rp::usb;
use embassy_rp::usb::Driver;
use embassy_usb::Builder;
use embassy_usb::driver::EndpointError;
use embassy_usb::driver::EndpointIn;
use embassy_usb::driver::EndpointOut;
#[cfg(not(feature = "rtt"))]
use panic_halt as _;
#[cfg(feature = "rtt")]
use panic_probe as _;
use pod_proto::dsclk::DEFAULT_HZ;
use pod_proto::dsclk::divider;
use pod_proto::usb::PACKET;
use pod_proto::usb::PID;
use pod_proto::usb::Status;
use pod_proto::usb::VID;
use pod_proto::usb::encode_status;
use static_cell::ConstStaticCell;

use crate::link::LinkPins;
use crate::link::PioLink;
use crate::pod::Lines;
use crate::pod::Pod;

#[cfg(all(feature = "rp2040", feature = "rp235xa"))]
compile_error!("choose one chip feature, not both");
#[cfg(not(any(feature = "rp2040", feature = "rp235xa")))]
compile_error!("choose a chip feature: rp2040 or rp235xa");

/// A `'static` buffer of `$size` bytes, zeroed at compile time: never on
/// the stack, as `StaticCell::init`'s argument would be.
macro_rules! buffer {
    ($size:expr) => {{
        static BUFFER: ConstStaticCell<[u8; $size]> = ConstStaticCell::new([0; $size]);
        BUFFER.take()
    }};
}

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => usb::InterruptHandler<USB>;
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    ADC_IRQ_FIFO => adc::InterruptHandler;
});

/// Vendor-specific interface class.
const VENDOR_CLASS: u8 = 0xFF;
/// Bus power drawn: the Pico and the translators' 3.3 V side.
const MAX_POWER_MA: u16 = 100;
/// Descriptor and control buffers, as large as embassy-usb's examples
/// give a single vendor interface.
const DESCRIPTOR_BYTES: usize = 256;
const CONTROL_BYTES: usize = 64;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // The translators' outputs stay off until a halt (PLAN.md, Hardware),
    // and RESET and BERR stay released.
    let lines = Lines {
        dsclk_enable: Output::new(p.PIN_10, Level::High),
        reset_drive: Output::new(p.PIN_8, Level::Low),
        reset: Input::new(p.PIN_6, Pull::None),
        ds: Input::new(p.PIN_7, Pull::None),
    };
    let freeze = Input::new(p.PIN_5, Pull::Down);
    let _berr_drive = Output::new(p.PIN_9, Level::Low);
    let _led = Output::new(p.PIN_25, Level::High);

    let Pio { mut common, sm0, .. } = Pio::new(p.PIO0, Irqs);
    let pins = LinkPins { dsclk: p.PIN_2, dsi: p.PIN_3, dso: p.PIN_4, dsi_enable: p.PIN_11 };
    let link = PioLink::new(&mut common, sm0, pins, divider(clk_sys_freq(), DEFAULT_HZ));
    let adc = Adc::new(p.ADC, Irqs, adc::Config::default());
    let vdd = adc::Channel::new_pin(p.PIN_26, Pull::None);
    let mut pod = Pod::new(link, lines, adc, vdd);

    let mut config = embassy_usb::Config::new(VID, PID);
    config.manufacturer = Some("m683xx-pod");
    config.product = Some("CPU32 BDM pod");
    config.max_power = MAX_POWER_MA;
    config.max_packet_size_0 = 64;
    let mut builder = Builder::new(
        Driver::new(p.USB, Irqs),
        config,
        buffer!(DESCRIPTOR_BYTES),
        buffer!(DESCRIPTOR_BYTES),
        buffer!(0),
        buffer!(CONTROL_BYTES),
    );
    let mut function = builder.function(VENDOR_CLASS, 0, 0);
    let mut interface = function.interface();
    let mut alt = interface.alt_setting(VENDOR_CLASS, 0, 0, None);
    let packet = u16::try_from(PACKET).expect("a packet fits u16");
    let mut from_host = alt.endpoint_bulk_out(None, packet);
    let mut to_host = alt.endpoint_bulk_in(None, packet);
    drop(function);
    let mut usb = builder.build();

    #[cfg(feature = "rtt")]
    defmt::info!("m683xx-pod up");
    // The watcher first, so FREEZE's level is in before a request.
    join3(freeze::watch(freeze), usb.run(), serve(&mut pod, &mut from_host, &mut to_host)).await;
}

/// Answers each request packet with one reply packet, for as long as
/// the host is connected, and again after it reconnects.
async fn serve(pod: &mut Pod<'_>, from_host: &mut impl EndpointOut, to_host: &mut impl EndpointIn) {
    let mut request = [0; PACKET];
    let mut reply = [0; PACKET];
    loop {
        from_host.wait_enabled().await;
        loop {
            let length = match from_host.read(&mut request).await {
                Ok(n) => pod.handle(&request[..n], &mut reply).await,
                Err(EndpointError::BufferOverflow) => encode_status(Status::BadRequest, &mut reply),
                Err(EndpointError::Disabled) => break,
            };
            if to_host.write(&reply[..length]).await.is_err() {
                break;
            }
        }
    }
}
