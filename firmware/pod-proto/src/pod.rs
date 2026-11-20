//! The pod's answer to each request (docs/usb-protocol.md): the order
//! of checks, the ATEMP read after each entry, the halt and reset
//! sequences' outcomes and every BDM command, over a [`Board`] that
//! supplies the hardware.  The firmware's board is the Pico's pins and
//! PIO; a model board stands in for tests.

use mc683xx_target::register::SystemRegister;

use crate::bdm::Command;
use crate::entry::EntryWatch;
use crate::session::Link;
use crate::session::Session;
use crate::usb::Atemp;
use crate::usb::MAX_MEMORY_BYTES;
use crate::usb::MAX_TRANSFER_WORDS;
use crate::usb::PACKET;
use crate::usb::PROTOCOL_VERSION;
use crate::usb::Pins;
use crate::usb::Request;
use crate::usb::Status;
use crate::usb::encode_atemp;
use crate::usb::encode_data;
use crate::usb::encode_pins;
use crate::usb::encode_status;
use crate::usb::encode_transfer_reply;
use crate::usb::encode_value;

/// NOPs clocked while the target answers "not ready", about 17 ms at
/// 1 MHz: far longer than any access on the 68331's bus.
pub const NOT_READY_RETRIES: u32 = 1000;
/// Below this target VDD the pod refuses to halt or reset: the 68331
/// runs from 5.0 V +-10% (MC68331UM Table A-7's conditions).
pub const VDD_PRESENT_MV: u16 = 4500;

/// The hardware around the serial link: the target connector's other
/// lines, and the link's own drivers.
#[expect(async_fn_in_trait, reason = "the pod runs one single-threaded executor; no Send bound is wanted")]
pub trait Board: Link {
    /// FREEZE high: the CPU is in background mode.
    fn frozen(&self) -> bool;

    /// FREEZE's edges since the board started, wrapping: a count that
    /// moved means the CPU left or entered background mode.
    fn freeze_edges(&self) -> u32;

    /// RESET high: not in reset.
    fn reset_high(&self) -> bool;

    /// DS high: no data strobe.
    fn ds_high(&self) -> bool;

    /// BERR high: no bus error.
    fn berr_high(&self) -> bool;

    /// Target VDD in millivolts, saturating.
    ///
    /// # Errors
    /// It could not be read.
    async fn vdd_mv(&mut self) -> Result<u16, VddError>;

    /// Drives BKPT/DSCLK high and DSI, for commands to a CPU already in
    /// background mode, which may have entered on its own with DSI off
    /// since GO.  Makes no DSCLK edge: BKPT held low by a halt stays
    /// low, for the first word's first rising edge to release.
    fn drive_link(&mut self);

    /// Holds BKPT low until FREEZE asserts, within the halt timeout;
    /// then drives DSI.  Returns whether FREEZE asserted; if not, BKPT
    /// is released.
    async fn halt(&mut self) -> bool;

    /// Holds BKPT low across a RESET pulse, so the CPU comes out of
    /// reset in background mode (MC68331UM 5.10.2.1), and waits for
    /// FREEZE as [`Board::halt`] does.
    async fn reset_halt(&mut self) -> bool;

    /// Sets DSCLK's rate, at most `dsclk_hz` where it can; returns the
    /// rate set (docs/usb-protocol.md, `SetClock`).
    fn set_clock(&mut self, dsclk_hz: u32) -> u32;
}

/// Target VDD could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("VDD could not be read")]
pub struct VddError;

/// What a request produced, before it is encoded.
enum Answer {
    /// Status only.
    Done,
    /// One 32-bit value.
    Value(u32),
    /// ATEMP for the current stay in background mode.
    Atemp(Atemp),
    /// Pin levels, VDD and ATEMP.
    Pins(Pins),
    /// The first `n` bytes of the memory buffer.
    Data(usize),
    /// The first `n` responses of the transfer buffer.
    Responses(usize),
}

/// The pod: requests in, replies out, over a board.
#[derive(Debug)]
pub struct Pod<B> {
    session: Session<B>,
    /// ATEMP, read first after each entry.
    entry: EntryWatch,
}

impl<B: Board> Pod<B> {
    /// The pod on `board`.
    pub const fn new(board: B) -> Self {
        Self { session: Session::new(board, NOT_READY_RETRIES), entry: EntryWatch::new() }
    }

    /// The board, for what tests do to the target between requests.
    pub const fn board_mut(&mut self) -> &mut B {
        self.session.link_mut()
    }

    /// Carries out the request in `packet` and writes the reply into
    /// `reply`, returning its length.
    pub async fn handle(&mut self, packet: &[u8], reply: &mut [u8; PACKET]) -> usize {
        let Ok(request) = Request::parse(packet) else {
            return encode_status(Status::BadRequest, reply);
        };
        if let Err(status) = self.read_entry().await {
            return encode_status(status, reply);
        }
        let mut responses = [0; MAX_TRANSFER_WORDS];
        let mut data = [0; MAX_MEMORY_BYTES];
        match self.carry_out(request, &mut responses, &mut data).await {
            Ok(Answer::Done) => encode_status(Status::Ok, reply),
            Ok(Answer::Value(value)) => encode_value(value, reply),
            Ok(Answer::Atemp(atemp)) => encode_atemp(atemp, reply),
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
                    *response =
                        self.session.link_mut().transfer(word).await.map_err(|_| Status::Unexpected)?;
                }
                Ok(Answer::Responses(words.len()))
            }
            Request::Pins => self.pins().await.map(Answer::Pins),
            Request::ResetHalt => self.reset_halt().await.map(Answer::Atemp),
            Request::Halt => self.halt().await.map(Answer::Atemp),
            Request::SetClock { dsclk_hz } => Ok(Answer::Value(self.session.link_mut().set_clock(dsclk_hz))),
            Request::Go => self.command(&Command::Go).await.map(|_| Answer::Done),
            Request::Call(address) => self.command(&Command::Call(address)).await.map(|_| Answer::Done),
            Request::Hello => Ok(Answer::Value(PROTOCOL_VERSION)),
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
    /// so the link drops DSI as the word ends.
    async fn command(&mut self, command: &Command) -> Result<u32, Status> {
        self.require_frozen()?;
        Ok(self.session.execute(command).await?)
    }

    fn require_frozen(&mut self) -> Result<(), Status> {
        if self.session.link_mut().frozen() { Ok(()) } else { Err(Status::NoFreeze) }
    }

    /// FREEZE as of the ATEMP read: if it moves between the two, the
    /// reply stays consistent and the next request catches up.
    async fn pins(&mut self) -> Result<Pins, Status> {
        self.read_entry().await?;
        let atemp = self.current_atemp();
        let board = self.session.link_mut();
        Ok(Pins {
            freeze: atemp != Atemp::None,
            reset: board.reset_high(),
            ds: board.ds_high(),
            berr: board.berr_high(),
            vdd_mv: board.vdd_mv().await.map_err(|_| Status::Unexpected)?,
            atemp,
        })
    }

    async fn require_target(&mut self) -> Result<(), Status> {
        let vdd_mv = self.session.link_mut().vdd_mv().await.map_err(|_| Status::Unexpected)?;
        if vdd_mv < VDD_PRESENT_MV { Err(Status::NoTarget) } else { Ok(()) }
    }

    /// Stops the CPU, unless it is already in background mode; returns
    /// ATEMP, already read if it was.
    async fn halt(&mut self) -> Result<Atemp, Status> {
        if !self.session.link_mut().frozen() {
            self.require_target().await?;
            if !self.session.link_mut().halt().await {
                return Err(Status::NoFreeze);
            }
        }
        self.entry_atemp().await
    }

    /// Resets the CPU into background mode; returns ATEMP.
    async fn reset_halt(&mut self) -> Result<Atemp, Status> {
        self.require_target().await?;
        if !self.session.link_mut().reset_halt().await {
            return Err(Status::NoFreeze);
        }
        self.entry_atemp().await
    }

    /// ATEMP for this stay in background mode, read now if not yet.
    async fn entry_atemp(&mut self) -> Result<Atemp, Status> {
        self.read_entry().await?;
        match self.current_atemp() {
            Atemp::None => Err(Status::NoFreeze),
            atemp => Ok(atemp),
        }
    }

    fn current_atemp(&mut self) -> Atemp {
        let board = self.session.link_mut();
        let (frozen, edges) = (board.frozen(), board.freeze_edges());
        self.entry.atemp(frozen, edges)
    }

    /// Reads ATEMP if the CPU has entered background mode since the last
    /// read, before any other command: ATEMP holds the entry source only
    /// until the first command after entry (CPU32RM 7.2.3, Table 7-2).
    async fn read_entry(&mut self) -> Result<(), Status> {
        let board = self.session.link_mut();
        let edges = board.freeze_edges();
        let Some(take) = self.entry.read_needed(board.frozen(), edges) else {
            return Ok(());
        };
        board.drive_link();
        match self.session.read_system_register(SystemRegister::Atemp).await {
            Ok(atemp) => {
                self.entry.record(edges, take(atemp));
                Ok(())
            }
            // Recorded, so the failure fails this request alone and not
            // every request until FREEZE moves.
            Err(error) => {
                self.entry.record(edges, Atemp::Unread);
                Err(error.into())
            }
        }
    }
}
