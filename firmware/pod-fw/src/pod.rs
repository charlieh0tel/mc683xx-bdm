//! The pod's requests carried out on the target: the BDM sequences
//! around the serial link (PLAN.md, Protocol summary) and the USB
//! requests that call them (`pod_proto::usb`).

use cpu32_target::SystemRegister;
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
use pod_proto::bdm::Command;
use pod_proto::dsclk::divider;
use pod_proto::session::Link;
use pod_proto::session::Session;
use pod_proto::usb::MAX_MEMORY_BYTES;
use pod_proto::usb::MAX_TRANSFER_WORDS;
use pod_proto::usb::PACKET;
use pod_proto::usb::Pins;
use pod_proto::usb::Request;
use pod_proto::usb::Status;
use pod_proto::usb::encode_data;
use pod_proto::usb::encode_pins;
use pod_proto::usb::encode_status;
use pod_proto::usb::encode_transfer_reply;
use pod_proto::usb::encode_value;

use crate::link::PioLink;

/// NOPs clocked while the target answers "not ready", about 17 ms at
/// 1 MHz: far longer than any access on the 68331's bus.
const NOT_READY_RETRIES: u32 = 1000;
/// How long FREEZE may take after BKPT, or after RESET releases.
const FREEZE_TIMEOUT: Duration = Duration::from_millis(100);
/// RESET held low by the pod: long against the SIM's reset timing
/// (SIMRM), short against a person.
const RESET_PULSE: Duration = Duration::from_millis(10);
/// The ADC: 12 bits against the Pico's 3.3 V reference.
const ADC_FULL_SCALE: u32 = 1 << 12;
const ADC_REFERENCE_MV: u32 = 3300;
/// VDD reaches the ADC through a 100 kOhm / 100 kOhm divider (PLAN.md,
/// Hardware): 5.5 V reads as 2.75 V.
const VDD_DIVIDER: u32 = 2;
/// The 68331 runs from 5.0 V +-10% (MC68331UM Table A-7's conditions).
const VDD_PRESENT_MV: u16 = 4500;

/// What a request produced, before it is encoded.
enum Answer {
    /// Status only.
    Done,
    /// One 32-bit value.
    Value(u32),
    /// Pin levels and VDD.
    Pins(Pins),
    /// The first `n` bytes of the memory buffer.
    Data(usize),
    /// The first `n` responses of the transfer buffer.
    Responses(usize),
}

/// The pod's side of the target connector.
pub(crate) struct Pod<'d> {
    session: Session<PioLink<'d>>,
    /// 74AHCT125 output enable for BKPT/DSCLK, active low.
    dsclk_enable: Output<'d>,
    /// 2N7002 gate: high pulls RESET low.
    reset_drive: Output<'d>,
    freeze: Input<'d>,
    reset: Input<'d>,
    ds: Input<'d>,
    adc: Adc<'d, Async>,
    vdd: Channel<'d>,
}

/// The pod's pins, bundled for [`Pod::new`].
pub(crate) struct Lines<'d> {
    /// See [`Pod`].
    pub(crate) dsclk_enable: Output<'d>,
    /// See [`Pod`].
    pub(crate) reset_drive: Output<'d>,
    /// FREEZE, high in background mode.
    pub(crate) freeze: Input<'d>,
    /// RESET, low in reset.
    pub(crate) reset: Input<'d>,
    /// DS, low during a bus cycle's data phase.
    pub(crate) ds: Input<'d>,
}

impl<'d> Pod<'d> {
    pub(crate) fn new(link: PioLink<'d>, lines: Lines<'d>, adc: Adc<'d, Async>, vdd: Channel<'d>) -> Self {
        Self {
            session: Session::new(link, NOT_READY_RETRIES),
            dsclk_enable: lines.dsclk_enable,
            reset_drive: lines.reset_drive,
            freeze: lines.freeze,
            reset: lines.reset,
            ds: lines.ds,
            adc,
            vdd,
        }
    }

    /// Carries out the request in `packet` and writes the reply into
    /// `reply`, returning its length.
    pub(crate) async fn handle(&mut self, packet: &[u8], reply: &mut [u8; PACKET]) -> usize {
        let Ok(request) = Request::parse(packet) else {
            return encode_status(Status::BadRequest, reply);
        };
        let mut responses = [0; MAX_TRANSFER_WORDS];
        let mut data = [0; MAX_MEMORY_BYTES];
        match self.carry_out(request, &mut responses, &mut data).await {
            Ok(Answer::Done) => encode_status(Status::Ok, reply),
            Ok(Answer::Value(value)) => encode_value(value, reply),
            Ok(Answer::Pins(pins)) => encode_pins(&pins, reply),
            Ok(Answer::Data(n)) => encode_data(&data[..n], reply),
            Ok(Answer::Responses(n)) => encode_transfer_reply(&responses[..n], reply),
            Err(status) => encode_status(status, reply),
        }
    }

    async fn carry_out(
        &mut self,
        request: Request<'_>,
        responses: &mut [u32; MAX_TRANSFER_WORDS],
        data: &mut [u8; MAX_MEMORY_BYTES],
    ) -> Result<Answer, Status> {
        match request {
            Request::Transfer(words) => {
                for (word, response) in words.iter().zip(responses.iter_mut()) {
                    *response = self.transfer(word).await;
                }
                Ok(Answer::Responses(words.len()))
            }
            Request::Pins => self.pins().await.map(Answer::Pins),
            Request::ResetHalt => self.reset_halt().await.map(Answer::Value),
            Request::Halt => self.halt().await.map(Answer::Value),
            Request::SetClock { dsclk_hz } => Ok(Answer::Value(self.set_clock(dsclk_hz))),
            Request::Go => self.command(&Command::Go).await.map(|_| Answer::Done),
            Request::Call(address) => self.command(&Command::Call(address)).await.map(|_| Answer::Done),
            Request::ReadRegister(register) => {
                self.command(&Command::ReadRegister(register)).await.map(Answer::Value)
            }
            Request::WriteRegister(register, value) => {
                self.command(&Command::WriteRegister(register, value)).await.map(|_| Answer::Done)
            }
            Request::ReadSystemRegister(register) => {
                self.command(&Command::ReadSystemRegister(register)).await.map(Answer::Value)
            }
            Request::WriteSystemRegister(register, value) => {
                self.command(&Command::WriteSystemRegister(register, value)).await.map(|_| Answer::Done)
            }
            Request::ReadMemory { address, size, length } => {
                self.require_frozen()?;
                self.session.read_block(address, size, &mut data[..length]).await?;
                Ok(Answer::Data(length))
            }
            Request::WriteMemory { address, size, data } => {
                self.require_frozen()?;
                self.session.write_block(address, size, data).await?;
                Ok(Answer::Done)
            }
        }
    }

    /// A BDM command, only in background mode: otherwise its clocks would
    /// be BKPT pulses.  GO and CALL go out through `Link::transfer_exit`,
    /// so the PIO drops `DSI_EN` as the word ends, and FREEZE negating
    /// gates DSI off: pin 8 is IFETCH again.
    async fn command(&mut self, command: &Command) -> Result<u32, Status> {
        self.require_frozen()?;
        Ok(self.session.execute(command).await?)
    }

    fn require_frozen(&self) -> Result<(), Status> {
        if self.freeze.is_low() {
            return Err(Status::NoFreeze);
        }
        Ok(())
    }

    async fn transfer(&mut self, word: u16) -> u32 {
        let Ok(bits) = self.session.link_mut().transfer(word).await;
        bits
    }

    async fn pins(&mut self) -> Result<Pins, Status> {
        Ok(Pins {
            freeze: self.freeze.is_high(),
            reset: self.reset.is_high(),
            ds: self.ds.is_high(),
            vdd_mv: self.vdd_mv().await?,
        })
    }

    /// Target VDD in millivolts.
    async fn vdd_mv(&mut self) -> Result<u16, Status> {
        let raw = self.adc.read(&mut self.vdd).await.map_err(|_| Status::Unexpected)?;
        let mv = u32::from(raw) * ADC_REFERENCE_MV * VDD_DIVIDER / ADC_FULL_SCALE;
        Ok(u16::try_from(mv).unwrap_or(u16::MAX))
    }

    async fn require_target(&mut self) -> Result<(), Status> {
        if self.vdd_mv().await? < VDD_PRESENT_MV {
            return Err(Status::NoTarget);
        }
        Ok(())
    }

    /// BKPT low until FREEZE; BKPT stays low after, and the first word's
    /// first rising edge releases it (PLAN.md, Protocol summary).
    /// Returns ATEMP, the entry source.
    async fn halt(&mut self) -> Result<u32, Status> {
        self.require_target().await?;
        self.bkpt_low();
        self.wait_freeze(async |_| {}).await?;
        self.read_atemp().await
    }

    /// BKPT low across a RESET pulse, so the CPU comes out of reset in
    /// background mode (MC68331UM 5.10.2.1).  Returns ATEMP.
    async fn reset_halt(&mut self) -> Result<u32, Status> {
        self.require_target().await?;
        self.session.link_mut().set_dsi_enabled(false);
        self.bkpt_low();
        self.reset_drive.set_high();
        Timer::after(RESET_PULSE).await;
        self.reset_drive.set_low();
        self.wait_freeze(async |pod| pod.reset.wait_for_high().await).await?;
        self.read_atemp().await
    }

    /// ATEMP, the entry source, read first after entry because the
    /// commands after overwrite it (CPU32RM 7.2.3, Table 7-2).
    async fn read_atemp(&mut self) -> Result<u32, Status> {
        Ok(self.session.read_system_register(SystemRegister::Atemp).await?)
    }

    fn set_clock(&mut self, dsclk_hz: u32) -> u32 {
        let divider = divider(clk_sys_freq(), dsclk_hz);
        self.session.link_mut().set_divider(divider);
        divider.dsclk_hz
    }

    fn bkpt_low(&mut self) {
        self.dsclk_enable.set_low();
        self.session.link_mut().set_dsclk(Level::Low);
    }

    /// Runs `first`, then waits for FREEZE, all within
    /// [`FREEZE_TIMEOUT`].  Then DSI goes on; on a timeout BKPT is
    /// released.
    async fn wait_freeze(&mut self, first: impl AsyncFnOnce(&mut Self)) -> Result<(), Status> {
        let frozen = with_timeout(FREEZE_TIMEOUT, async {
            first(self).await;
            self.freeze.wait_for_high().await;
        })
        .await;
        if frozen.is_ok() {
            self.session.link_mut().set_dsi_enabled(true);
            Ok(())
        } else {
            self.session.link_mut().set_dsclk(Level::High);
            Err(Status::NoFreeze)
        }
    }
}
