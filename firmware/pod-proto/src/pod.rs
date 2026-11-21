//! The pod's answer to each request (docs/usb-protocol.md): the order
//! of checks, the ATEMP read after each entry, the halt and reset
//! sequences' outcomes and every BDM command, over a [`Board`] that
//! supplies the hardware.  The firmware's board is the Pico's pins and
//! PIO; a model board stands in for tests.

use mc683xx_target::address::Address;
use mc683xx_target::register::SystemRegister;

use crate::bdm::COMPLETE;
use crate::bdm::Command;
use crate::bdm::NOP;
use crate::bdm::Response;
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

/// Bits in a serial word: a resync tries at most this many frames.
const WORD_BITS: usize = 17;
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

    /// After CALL's last word: waits for the CPU to leave background
    /// mode, FREEZE low or its edge count moved from `edges`, within a
    /// timeout long against stacking's bus cycles and FREEZE's own
    /// refresh.  Returns whether it left.
    async fn left_background(&mut self, edges: u32) -> bool;

    /// Pulses RESET with BKPT high, so the CPU comes out of reset
    /// running, background mode off, and waits for RESET to rise.
    async fn reset_run(&mut self);

    /// Drives BKPT low and leaves it, clocking nothing: a running CPU
    /// with background mode enabled halts, and a CPU reset meanwhile
    /// comes out of reset in background mode (BKPT low as RESET rises).
    /// The next word's first rising edge releases it.
    fn hold(&mut self);

    /// The firmware's version: crate version and git commit, ASCII,
    /// shorter than a packet.
    fn version(&self) -> &'static str;

    /// One DSCLK rising edge outside a word, DSI as the last word left
    /// it: the CPU takes it as a bit, moving its frame on by one.  For
    /// resynchronizing only.
    async fn clock_bit(&mut self);

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
    /// Text that lasts.
    Text(&'static str),
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
        // A reset makes the entry moot, and a link out of frame must not
        // stop the reset that frames it again.
        if !matches!(
            request,
            Request::ResetHalt | Request::ResetRun | Request::Resync | Request::Version | Request::Hold
        ) && let Err(status) = self.read_entry().await
        {
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
            Ok(Answer::Text(text)) => encode_data(text.as_bytes(), reply),
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
            Request::ResetRun => self.reset_run().await.map(|()| Answer::Done),
            Request::Hold => self.hold().await.map(|()| Answer::Done),
            Request::Resync => self.resync_request().await.map(Answer::Atemp),
            Request::Halt => self.halt().await.map(Answer::Atemp),
            Request::SetClock { dsclk_hz } => Ok(Answer::Value(self.session.link_mut().set_clock(dsclk_hz))),
            Request::Go => self.command(&Command::Go).await.map(|_| Answer::Done),
            Request::Call(address) => self.call(address).await.map(|()| Answer::Done),
            Request::Hello => Ok(Answer::Value(PROTOCOL_VERSION)),
            Request::Version => Ok(Answer::Text(self.session.link_mut().version())),
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

    /// CALL: the CPU leaves background mode once the return address is
    /// stacked, or stays and answers the next word with a bus error if
    /// stacking took one (CPU32RM 7.2.8.13).  FREEZE tells which before
    /// anything more is clocked: to a running CPU a clock is a BKPT
    /// pulse.
    async fn call(&mut self, address: Address) -> Result<(), Status> {
        self.require_frozen()?;
        let edges = self.session.link_mut().freeze_edges();
        self.session.execute(&Command::Call(address)).await?;
        let board = self.session.link_mut();
        if board.left_background(edges).await {
            return Ok(());
        }
        board.drive_link();
        Err(self.session.exit_refused().await.into())
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

    /// Holds BKPT low; no ATEMP read, which would release it.
    async fn hold(&mut self) -> Result<(), Status> {
        self.require_target().await?;
        self.session.link_mut().hold();
        Ok(())
    }

    /// Resets the CPU to run.
    async fn reset_run(&mut self) -> Result<(), Status> {
        self.require_target().await?;
        self.session.link_mut().reset_run().await;
        Ok(())
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
        match self.session.read_system_register(SystemRegister::Atemp).await.map_err(Status::from) {
            Ok(atemp) => {
                self.entry.record(edges, take(atemp));
                Ok(())
            }
            // Recorded, so the failure fails this request alone and not
            // every request until FREEZE moves.
            Err(status) => {
                self.entry.record(edges, Atemp::Unread);
                Err(status)
            }
        }
    }

    /// Resync on request: the CPU must be halted; ATEMP is read after,
    /// stale, since whatever the CPU took out of frame came first.
    async fn resync_request(&mut self) -> Result<Atemp, Status> {
        self.require_target().await?;
        if !self.session.link_mut().frozen() {
            return Err(Status::NoFreeze);
        }
        self.session.link_mut().drive_link();
        self.resync().await?;
        let edges = self.session.link_mut().freeze_edges();
        match self.session.read_system_register(SystemRegister::Atemp).await {
            Ok(value) => {
                self.entry.record(edges, Atemp::Stale(value));
                Ok(Atemp::Stale(value))
            }
            Err(error) => {
                self.entry.record(edges, Atemp::Unread);
                Err(error.into())
            }
        }
    }

    /// Brings a halted CPU's serial link into frame.  An idle CPU answers
    /// each NOP "command complete" (CPU32RM 7.2.8.15), a clear status
    /// bit and 16 set; out of frame, the clear bit lands elsewhere in the
    /// word.  Each lone DSCLK edge moves the CPU's frame on a bit, so
    /// within a word's bits of them a NOP reads "command complete".
    /// A command the CPU took out of frame may take the NOPs' zeros as
    /// operands.
    async fn resync(&mut self) -> Result<(), Status> {
        let board = self.session.link_mut();
        for _ in 0..WORD_BITS {
            // The first NOP ends whatever the CPU was answering.
            board.transfer(NOP).await.map_err(|_| Status::Unexpected)?;
            let answer = board.transfer(NOP).await.map_err(|_| Status::Unexpected)?;
            if Response::decode(answer) == Response::Data(COMPLETE) {
                return Ok(());
            }
            board.clock_bit().await;
        }
        Err(Status::Unexpected)
    }
}
