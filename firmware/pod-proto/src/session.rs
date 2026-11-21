//! BDM commands carried out over a serial link: each command's words
//! out, then NOPs until its result or "command complete" comes back
//! (CPU32RM 7.2.8.2).
//!
//! Every word in returns the CPU's answer to the word before, so a
//! command's own answer comes back while the next word goes out.  The
//! session collects it with NOPs, which do nothing; a NOP's own
//! "command complete" comes back with the next command's opcode, and is
//! ignored there.

use mc683xx_target::address::Address;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;

use crate::bdm::COMPLETE;
use crate::bdm::Command;
use crate::bdm::NOP;
use crate::bdm::Reply;
use crate::bdm::Response;
use crate::bdm::byte_count;

/// The serial link to the CPU: DSCLK, DSI and DSO, in the firmware the
/// PIO state machine (PLAN.md, PIO program).
#[expect(async_fn_in_trait, reason = "the pod runs one single-threaded executor; no Send bound is wanted")]
pub trait Link {
    /// The link's own failure.
    type Error;

    /// Shifts `word` out on DSI with bit 16 clear, and returns the 17
    /// bits shifted in from DSO at the same time.
    async fn transfer(&mut self, word: u16) -> Result<u32, Self::Error>;

    /// As [`Link::transfer`], for the last word before the CPU leaves
    /// background mode (GO's opcode, CALL's last address word): the link
    /// stops driving DSI as soon as the word is out, since pin 8 is
    /// IFETCH, a CPU output, once FREEZE negates (PLAN.md, Hardware).
    async fn transfer_exit(&mut self, word: u16) -> Result<u32, Self::Error>;
}

/// Why a command did not complete.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error<L> {
    /// The link failed.
    #[error("serial link failed")]
    Link(L),
    /// The access ended in a bus or address error.
    #[error("bus error")]
    BusError,
    /// The CPU did not take the command.
    #[error("illegal command")]
    Illegal,
    /// Still not ready after the session's retries.
    #[error("not ready after {0} retries")]
    NotReady(u32),
    /// A response the command's sequence does not allow.
    #[error("unexpected response {0:?}")]
    Unexpected(Response),
    /// A block's length is not a whole number of accesses.
    #[error("length {length} is not a multiple of {size:?}")]
    Length {
        /// The block's length in bytes.
        length: usize,
        /// The access size.
        size: Size,
    },
}

/// Commands over a link to a CPU in background mode.
#[derive(Debug)]
pub struct Session<L> {
    link: L,
    /// NOPs clocked while the CPU answers "not ready" before giving up.
    retries: u32,
}

impl<L: Link> Session<L> {
    /// A session over `link`, waiting up to `retries` NOPs for each
    /// result.
    pub const fn new(link: L, retries: u32) -> Self {
        Self { link, retries }
    }

    /// The link, to give it back.
    pub fn into_link(self) -> L {
        self.link
    }

    /// The link, for what lies outside a command: holding DSCLK low for
    /// a halt, changing its rate.
    pub const fn link_mut(&mut self) -> &mut L {
        &mut self.link
    }

    /// Carries out `command`, returning its result: the value for a
    /// read, 0 for anything else.  After GO or CALL nothing more is
    /// clocked: DSCLK is BKPT again once the CPU leaves background
    /// mode.
    ///
    /// # Errors
    /// The link failed, or the CPU answered with an error.
    pub async fn execute(&mut self, command: &Command) -> Result<u32, Error<L::Error>> {
        self.send(command).await?;
        match command.reply() {
            Reply::Data { words: 1 } => Ok(u32::from(self.first_word().await?)),
            Reply::Data { .. } => {
                let high = self.first_word().await?;
                let low = self.next_word().await?;
                Ok(u32::from(high) << 16 | u32::from(low))
            }
            Reply::Complete => match self.first_word().await? {
                COMPLETE => Ok(0),
                other => Err(Error::Unexpected(Response::Data(other))),
            },
            Reply::Exit => Ok(0),
        }
    }

    /// Reads a data or address register.
    ///
    /// # Errors
    /// As [`Session::execute`].
    pub async fn read_register(&mut self, register: Register) -> Result<u32, Error<L::Error>> {
        self.execute(&Command::ReadRegister(register)).await
    }

    /// Reads a system register.
    ///
    /// # Errors
    /// As [`Session::execute`].
    pub async fn read_system_register(&mut self, register: SystemRegister) -> Result<u32, Error<L::Error>> {
        self.execute(&Command::ReadSystemRegister(register)).await
    }

    /// Reads `data.len()` bytes from `address`, a READ then DUMPs,
    /// big-endian.
    ///
    /// # Errors
    /// As [`Session::execute`], or a length that is not a multiple of
    /// `size`.
    pub async fn read_block(
        &mut self,
        address: Address,
        size: Size,
        data: &mut [u8],
    ) -> Result<(), Error<L::Error>> {
        check_length(size, data.len())?;
        for (n, chunk) in data.chunks_exact_mut(byte_count(size)).enumerate() {
            let command = if n == 0 { Command::Read(size, address) } else { Command::Dump(size) };
            let value = self.execute(&command).await?;
            chunk.copy_from_slice(&value.to_be_bytes()[4 - byte_count(size)..]);
        }
        Ok(())
    }

    /// Writes `data` to `address`, a WRITE then FILLs, big-endian.
    ///
    /// # Errors
    /// As [`Session::execute`], or a length that is not a multiple of
    /// `size`.
    pub async fn write_block(
        &mut self,
        address: Address,
        size: Size,
        data: &[u8],
    ) -> Result<(), Error<L::Error>> {
        check_length(size, data.len())?;
        for (n, chunk) in data.chunks_exact(byte_count(size)).enumerate() {
            let mut bytes = [0; 4];
            bytes[4 - byte_count(size)..].copy_from_slice(chunk);
            let value = u32::from_be_bytes(bytes);
            let command =
                if n == 0 { Command::Write(size, address, value) } else { Command::Fill(size, value) };
            self.execute(&command).await?;
        }
        Ok(())
    }

    /// After a CALL the CPU did not leave background mode for: the
    /// status it answers the next word with, a bus error if stacking
    /// the return address took one (CPU32RM 7.2.8.13).
    pub async fn exit_refused(&mut self) -> Error<L::Error> {
        match self.first_word().await {
            Ok(data) => Error::Unexpected(Response::Data(data)),
            Err(error) => error,
        }
    }

    /// Sends a command's words, the last through
    /// [`Link::transfer_exit`] if the command leaves background mode.
    /// The answer to the opcode belongs to the word before and is
    /// ignored; each operand's should be "not ready".  "Illegal" there
    /// means the opcode was refused, and the CPU now takes the next word
    /// as a command, so the rest are not sent.
    async fn send(&mut self, command: &Command) -> Result<(), Error<L::Error>> {
        let words = command.encode();
        let words = words.as_slice();
        let exits = command.reply() == Reply::Exit;
        for (n, word) in words.iter().enumerate() {
            let response = if exits && n + 1 == words.len() {
                let bits = self.link.transfer_exit(*word).await.map_err(Error::Link)?;
                Response::decode(bits)
            } else {
                self.transfer(*word).await?
            };
            match response {
                _ if n == 0 => {}
                Response::NotReady => {}
                Response::Illegal => return Err(Error::Illegal),
                other => return Err(Error::Unexpected(other)),
            }
        }
        Ok(())
    }

    /// The first word of an answer: NOPs until the CPU is ready.
    async fn first_word(&mut self) -> Result<u16, Error<L::Error>> {
        for _ in 0..=self.retries {
            match self.transfer(NOP).await? {
                Response::NotReady => {}
                Response::Data(data) => return Ok(data),
                Response::BusError => return Err(Error::BusError),
                Response::Illegal => return Err(Error::Illegal),
                other @ Response::Unknown(_) => return Err(Error::Unexpected(other)),
            }
        }
        Err(Error::NotReady(self.retries))
    }

    /// The second word of a long result, which follows the first at once.
    async fn next_word(&mut self) -> Result<u16, Error<L::Error>> {
        match self.transfer(NOP).await? {
            Response::Data(data) => Ok(data),
            other => Err(Error::Unexpected(other)),
        }
    }

    async fn transfer(&mut self, word: u16) -> Result<Response, Error<L::Error>> {
        let bits = self.link.transfer(word).await.map_err(Error::Link)?;
        Ok(Response::decode(bits))
    }
}

fn check_length<L>(size: Size, length: usize) -> Result<(), Error<L>> {
    if length.is_multiple_of(byte_count(size)) { Ok(()) } else { Err(Error::Length { length, size }) }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use embassy_futures::block_on;

    use super::Address;
    use super::Command;
    use super::Error;
    use super::Link;
    use super::Register;
    use super::Response;
    use super::Session;
    use super::Size;
    use super::SystemRegister;

    /// Responses as the CPU sends them (CPU32RM Table 7-3).
    const NOT_READY: u32 = 0x1_0000;
    const BUS_ERROR: u32 = 0x1_0001;
    const ILLEGAL: u32 = 0x1_FFFF;
    const COMPLETE: u32 = 0x0_FFFF;
    /// Whatever the CPU answers to an opcode: the previous command's.
    const STALE: u32 = COMPLETE;
    const RETRIES: u32 = 3;

    /// A link that checks each word sent against a script and answers
    /// from it.
    #[derive(Debug, Default)]
    struct Script {
        /// Each word expected, and the answer to it.
        steps: VecDeque<(u16, u32)>,
        /// The words sent with `transfer_exit`.
        exits: Vec<u16>,
    }

    impl Link for Script {
        type Error = ();

        async fn transfer(&mut self, word: u16) -> Result<u32, ()> {
            let (expected, response) = self.steps.pop_front().expect("script ended early");
            assert_eq!(word, expected, "word sent");
            Ok(response)
        }

        async fn transfer_exit(&mut self, word: u16) -> Result<u32, ()> {
            self.exits.push(word);
            self.transfer(word).await
        }
    }

    /// Runs `body` on a session over `script` and checks the script was
    /// used up.
    fn run<T>(script: &[(u16, u32)], body: impl AsyncFnOnce(&mut Session<Script>) -> T) -> T {
        run_exiting(script, &[], body)
    }

    /// As `run`, checking which words went through `transfer_exit`.
    fn run_exiting<T>(
        script: &[(u16, u32)],
        exits: &[u16],
        body: impl AsyncFnOnce(&mut Session<Script>) -> T,
    ) -> T {
        let link = Script { steps: script.iter().copied().collect(), exits: Vec::new() };
        let mut session = Session::new(link, RETRIES);
        let result = block_on(body(&mut session));
        let link = session.into_link();
        assert!(link.steps.is_empty(), "script not used up");
        assert_eq!(link.exits, exits, "words sent as the last before leaving background mode");
        result
    }

    #[test]
    fn read_register_waits_out_not_ready() {
        let result =
            run(&[(0x2180, STALE), (0x0000, NOT_READY), (0x0000, 0x1234), (0x0000, 0x5678)], async |s| {
                s.read_register(Register::D0).await
            });
        assert_eq!(result, Ok(0x1234_5678));
    }

    #[test]
    fn read_atemp_after_entry() {
        let result = run(&[(0x2588, NOT_READY), (0x0000, 0x0000), (0x0000, 0x0001)], async |s| {
            s.read_system_register(SystemRegister::Atemp).await
        });
        assert_eq!(result, Ok(1));
    }

    #[test]
    fn write_register_until_complete() {
        let result = run(
            &[(0x2081, STALE), (0xDEAD, NOT_READY), (0xBEEF, NOT_READY), (0x0000, COMPLETE)],
            async |s| s.execute(&Command::WriteRegister(Register::D1, 0xDEAD_BEEF)).await,
        );
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn read_byte_is_in_the_low_byte() {
        let result = run(
            &[
                (0x1900, STALE),
                (0x00FF, NOT_READY),
                (0xFA00, NOT_READY),
                (0x0000, NOT_READY),
                (0x0000, 0x00CF),
            ],
            async |s| s.execute(&Command::Read(Size::Byte, Address(0x00FF_FA00))).await,
        );
        assert_eq!(result, Ok(0xCF));
    }

    #[test]
    fn bus_error_on_read() {
        let result = run(
            &[(0x1940, STALE), (0x0010, NOT_READY), (0x0000, NOT_READY), (0x0000, BUS_ERROR)],
            async |s| s.execute(&Command::Read(Size::Word, Address(0x0010_0000))).await,
        );
        assert_eq!(result, Err(Error::BusError));
    }

    #[test]
    fn illegal_opcode_stops_the_operands() {
        // The CPU takes the word after an illegal opcode as a command, so
        // the second address word is never sent.
        let result = run(&[(0x1980, STALE), (0x0000, ILLEGAL)], async |s| {
            s.execute(&Command::Read(Size::Long, Address(0))).await
        });
        assert_eq!(result, Err(Error::Illegal));
    }

    #[test]
    fn not_ready_gives_up_after_the_retries() {
        let result = run(
            &[
                (0x0400, STALE),
                (0x0000, NOT_READY),
                (0x0000, NOT_READY),
                (0x0000, NOT_READY),
                (0x0000, NOT_READY),
            ],
            async |s| s.execute(&Command::ResetPeripherals).await,
        );
        assert_eq!(result, Err(Error::NotReady(RETRIES)));
    }

    #[test]
    fn write_expects_command_complete() {
        let result = run(&[(0x0000, STALE), (0x0000, 0x1234)], async |s| s.execute(&Command::Nop).await);
        assert_eq!(result, Err(Error::Unexpected(Response::Data(0x1234))));
    }

    #[test]
    fn go_clocks_nothing_after() {
        let result = run_exiting(&[(0x0C00, STALE)], &[0x0C00], async |s| s.execute(&Command::Go).await);
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn call_releases_dsi_on_its_last_word() {
        let result =
            run_exiting(&[(0x0800, STALE), (0x0004, NOT_READY), (0x1000, NOT_READY)], &[0x1000], async |s| {
                s.execute(&Command::Call(Address(0x0004_1000))).await
            });
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn a_call_that_stayed_reads_its_bus_error() {
        let result = run(&[(0x0000, NOT_READY), (0x0000, BUS_ERROR)], async |s| s.exit_refused().await);
        assert_eq!(result, Error::BusError);
    }

    #[test]
    fn a_call_that_stayed_with_data_is_unexpected() {
        let result = run(&[(0x0000, COMPLETE)], async |s| s.exit_refused().await);
        assert_eq!(result, Error::Unexpected(Response::Data(0xFFFF)));
    }

    #[test]
    fn read_block_is_read_then_dump() {
        let mut data = [0; 6];
        let result = run(
            &[
                (0x1940, STALE),
                (0x0000, NOT_READY),
                (0x1000, NOT_READY),
                (0x0000, 0x0102),
                (0x1D40, COMPLETE),
                (0x0000, 0x0304),
                (0x1D40, COMPLETE),
                (0x0000, NOT_READY),
                (0x0000, 0x0506),
            ],
            async |s| s.read_block(Address(0x1000), Size::Word, &mut data).await,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(data, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn write_block_is_write_then_fill() {
        let result = run(
            &[
                (0x1880, STALE),
                (0x0000, NOT_READY),
                (0x2000, NOT_READY),
                (0x0102, NOT_READY),
                (0x0304, NOT_READY),
                (0x0000, COMPLETE),
                (0x1C80, COMPLETE),
                (0x0506, NOT_READY),
                (0x0708, NOT_READY),
                (0x0000, COMPLETE),
            ],
            async |s| s.write_block(Address(0x2000), Size::Long, &[1, 2, 3, 4, 5, 6, 7, 8]).await,
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn block_length_must_be_whole_accesses() {
        let mut data = [0; 3];
        let result = run(&[], async |s| s.read_block(Address(0), Size::Word, &mut data).await);
        assert_eq!(result, Err(Error::Length { length: 3, size: Size::Word }));
    }

    #[test]
    fn empty_block_sends_nothing() {
        let result = run(&[], async |s| s.write_block(Address(0), Size::Byte, &[]).await);
        assert_eq!(result, Ok(()));
    }
}
