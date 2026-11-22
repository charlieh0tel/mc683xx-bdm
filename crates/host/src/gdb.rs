//! One gdb session over a [`PodTarget`]: mc683xx-gdb's `Bdm` adapter
//! on a TCP connection, for `bdm gdb` and the Python bindings alike
//! (PLAN.md, Host, gdb).  The host (`bdm`, or Python) is asked between
//! waits, on gdb or on a target running, whether to go on: its error
//! ends the session as an error of its own would, gdb's breakpoints
//! removed and the target halted, and is returned.

use mc683xx_target::Unsupported;
use std::fmt;
use std::io;
use std::net::SocketAddr;

use mc683xx_gdb::SessionError;
use mc683xx_gdb::bdm::Bdm;
use mc683xx_gdb::connection::PollError;
use mc683xx_gdb::connection::Polled;
use mc683xx_gdb::connection::Waiter;
use mc683xx_gdb::connection::accept_polled;
use mc683xx_target::Cpu32Target;
use mc683xx_target::address::Address;
use mc683xx_target::entry::Entry;
use mc683xx_target::function_code::FunctionCode;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;
use mc683xx_target::size::Size;

use crate::pod;
use crate::target::PodTarget;
use crate::transport::Transport;

/// Where to listen for gdb unless told: gdb's usual port, on this host
/// only, since the session controls the target (mc683xx's `serve_gdb`
/// default).
pub const LISTEN: &str = "127.0.0.1:1234";

/// How a session ended, as gdb asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// gdb detached: the target runs on.
    Detached,
    /// gdb killed the target: it stays halted.
    Killed,
}

/// The pod's error, or the host's reason to end the session: the
/// target's error as the session sees it.
#[derive(Debug, thiserror::Error)]
pub enum Stopped<E> {
    /// The pod, its transport or the target failed.
    #[error(transparent)]
    Target(pod::Error),
    /// The host ended the session.
    #[error(transparent)]
    Host(E),
}

impl<E> From<Unsupported> for Stopped<E> {
    fn from(unsupported: Unsupported) -> Self {
        Self::Target(unsupported.into())
    }
}

/// Why a session ended other than as gdb asked.
#[derive(Debug, thiserror::Error)]
pub enum ServeError<E> {
    /// The listener could not be bound, or gdb's connection taken.
    #[error("listening for gdb on {address}")]
    Listen {
        /// Where it was to listen.
        address: String,
        /// Why it could not.
        source: io::Error,
    },
    /// The host ended the session: before gdb connected, the target
    /// untouched; after, gdb's breakpoints removed and the target
    /// halted.
    #[error(transparent)]
    Host(E),
    /// The pod, its transport or the target failed.
    #[error("target: {0}")]
    Target(pod::Error),
    /// gdb's connection failed, or gdb closed it.
    #[error("connection: {0}")]
    Connection(io::Error),
    /// gdb said something the stub could not take.
    #[error("protocol: {0}")]
    Protocol(String),
    /// Breakpoints planted that could not be put back, at these
    /// addresses: the target is left halted.
    #[error("breakpoints still planted at {}", listed(.0))]
    Unrestored(Vec<Address>),
    /// The session failed, and so did cleaning up after it: the target
    /// is left halted.
    #[error("{}; cleaning up: {}", shown(error), shown(cleanup))]
    Uncleaned {
        /// The session's failure.
        error: Box<Self>,
        /// Cleaning up's.
        cleanup: Box<Self>,
    },
    /// An ending or failure newer than this host knows, as mc683xx-gdb
    /// shows it.
    #[error("gdb session: {0}")]
    Unknown(String),
}

/// `error` as its message: a nested error, which thiserror would
/// otherwise bound on its own `Display`, an unprovable cycle.
fn shown<E: fmt::Display>(error: &ServeError<E>) -> String {
    error.to_string()
}

/// `addresses`, comma-separated.
fn listed(addresses: &[Address]) -> String {
    addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

impl<E: fmt::Debug> From<PollError<E>> for ServeError<E> {
    fn from(error: PollError<E>) -> Self {
        match error {
            PollError::Connection(error) => Self::Connection(error),
            PollError::Host(error) => Self::Host(error),
            other => Self::Unknown(format!("{other:?}")),
        }
    }
}

impl<E: fmt::Debug> From<SessionError<Stopped<E>, PollError<E>>> for ServeError<E> {
    fn from(error: SessionError<Stopped<E>, PollError<E>>) -> Self {
        match error {
            SessionError::Target { error: Stopped::Target(error) } => Self::Target(error),
            SessionError::Target { error: Stopped::Host(error) } => Self::Host(error),
            SessionError::Connection { error } => error.into(),
            SessionError::Protocol(text) => Self::Protocol(text),
            SessionError::Unrestored(addresses) => Self::Unrestored(addresses),
            SessionError::Uncleaned { error, cleanup } => {
                Self::Uncleaned { error: Box::new((*error).into()), cleanup: Box::new((*cleanup).into()) }
            }
            other => Self::Unknown(format!("{other:?}")),
        }
    }
}

/// The host as the session waits: asked after each wait in which gdb
/// sent nothing.
struct Asked<'c, C>(&'c C);

impl<C> fmt::Debug for Asked<'_, C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Asked").finish_non_exhaustive()
    }
}

impl<E, C: Fn() -> Result<(), E>> Waiter for Asked<'_, C> {
    type Error = E;

    fn wait<R: Send>(&mut self, wait: impl FnOnce() -> R + Send) -> R {
        wait()
    }

    fn idle(&mut self) -> Result<(), E> {
        (self.0)()
    }
}

/// gdb's connection, taken by [`accept`], for [`serve`].
pub struct Connection<'c, C>(Polled<Asked<'c, C>>);

impl<C> fmt::Debug for Connection<'_, C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Connection").field(&self.0).finish()
    }
}

/// The target as the session drives it: the host asked each time the
/// session looks at a target running.
#[derive(Debug)]
struct Checked<'t, 'c, T, C> {
    target: &'t mut PodTarget<T>,
    check: &'c C,
}

impl<T: Transport, E: std::error::Error + 'static, C: Fn() -> Result<(), E>> Cpu32Target
    for Checked<'_, '_, T, C>
{
    type Error = Stopped<E>;

    fn reset_halted(&mut self) -> Result<Entry, Self::Error> {
        self.target.reset_halted().map_err(Stopped::Target)
    }

    fn halt(&mut self) -> Result<Entry, Self::Error> {
        self.target.halt().map_err(Stopped::Target)
    }

    fn halted(&mut self) -> Result<Option<Entry>, Self::Error> {
        (self.check)().map_err(Stopped::Host)?;
        self.target.halted().map_err(Stopped::Target)
    }

    fn go(&mut self) -> Result<(), Self::Error> {
        self.target.go().map_err(Stopped::Target)
    }

    fn step(&mut self) -> Result<Entry, Self::Error> {
        self.target.step().map_err(Stopped::Target)
    }

    fn read_register(&mut self, register: Register) -> Result<u32, Self::Error> {
        self.target.read_register(register).map_err(Stopped::Target)
    }

    fn write_register(&mut self, register: Register, value: u32) -> Result<(), Self::Error> {
        self.target.write_register(register, value).map_err(Stopped::Target)
    }

    fn read_system_register(&mut self, register: SystemRegister) -> Result<u32, Self::Error> {
        self.target.read_system_register(register).map_err(Stopped::Target)
    }

    fn write_system_register(&mut self, register: SystemRegister, value: u32) -> Result<(), Self::Error> {
        self.target.write_system_register(register, value).map_err(Stopped::Target)
    }

    fn read_memory(
        &mut self,
        address: Address,
        size: Size,
        function_code: FunctionCode,
        data: &mut [u8],
    ) -> Result<(), Self::Error> {
        self.target.read_memory(address, size, function_code, data).map_err(Stopped::Target)
    }

    fn write_memory(
        &mut self,
        address: Address,
        size: Size,
        function_code: FunctionCode,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        self.target.write_memory(address, size, function_code, data).map_err(Stopped::Target)
    }
}

/// Listens at `address` for one gdb, tells `ready` where (a port of 0
/// picks one), and takes its connection, asking `check` every
/// [`POLL_INTERVAL`](mc683xx_gdb::connection::POLL_INTERVAL) meanwhile.
///
/// # Errors
///
/// [`ServeError::Listen`] if the listener cannot be bound or the
/// connection taken; [`ServeError::Host`] with `ready`'s or `check`'s
/// error.
pub fn accept<'c, E: fmt::Debug, C: Fn() -> Result<(), E>>(
    address: &str,
    ready: impl FnOnce(SocketAddr) -> Result<(), E>,
    check: &'c C,
) -> Result<Connection<'c, C>, ServeError<E>> {
    accept_polled(address, ready, Asked(check)).map(Connection).map_err(|error| match error {
        PollError::Connection(source) => ServeError::Listen { address: address.to_owned(), source },
        other => other.into(),
    })
}

/// Serves gdb on `connection` over `target` until gdb detaches (the
/// target resumed) or kills it (left halted), with `scratch` for its
/// steps ([`PodTarget::set_scratch`], cleared when the session ends).
/// `check` is asked whenever the
/// session waits on gdb or looks at the target running.
///
/// # Errors
///
/// [`ServeError::Host`] with `check`'s error; the others as
/// mc683xx-gdb's `serve` fails.  However it fails, gdb's breakpoints are
/// removed, if they can be, and the target halted.
pub fn serve<T: Transport, E: std::error::Error + 'static, C: Fn() -> Result<(), E>>(
    target: &mut PodTarget<T>,
    scratch: Address,
    connection: Connection<'_, C>,
    check: &C,
) -> Result<Ending, ServeError<E>> {
    target.set_scratch(Some(scratch));
    let served =
        mc683xx_gdb::serve(&mut Bdm::new(&mut Checked { target: &mut *target, check }), connection.0);
    // The scratch is this session's: a later step must name its own.
    target.set_scratch(None);
    match served? {
        mc683xx_gdb::Ending::Detached => Ok(Ending::Detached),
        mc683xx_gdb::Ending::Killed => Ok(Ending::Killed),
        other => Err(ServeError::Unknown(format!("{other:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use mc683xx_gdb::SessionError;
    use mc683xx_gdb::connection::PollError;
    use mc683xx_target::address::Address;

    use super::ServeError;
    use super::Stopped;
    use super::accept;

    /// A host's reason to end a session.
    #[derive(Debug, thiserror::Error)]
    #[error("stop")]
    struct Stop;

    type Session = SessionError<Stopped<Stop>, PollError<Stop>>;

    #[test]
    fn the_host_s_error_comes_back_from_the_target_or_the_connection() {
        let from_target = ServeError::from(Session::Target { error: Stopped::Host(Stop) });
        assert!(matches!(from_target, ServeError::Host(Stop)));
        let from_connection = ServeError::from(Session::Connection { error: PollError::Host(Stop) });
        assert!(matches!(from_connection, ServeError::Host(Stop)));
    }

    #[test]
    fn a_failed_clean_up_keeps_both_errors() {
        let error = ServeError::from(Session::Uncleaned {
            error: Box::new(Session::Connection {
                error: PollError::Connection(io::ErrorKind::UnexpectedEof.into()),
            }),
            cleanup: Box::new(Session::Unrestored(vec![Address(0x0010_0010), Address(0x0010_0020)])),
        });
        assert_eq!(
            error.to_string(),
            "connection: unexpected end of file; cleaning up: breakpoints still planted at 0x00100010, 0x00100020"
        );
        assert!(matches!(error, ServeError::Uncleaned { error, cleanup }
            if matches!(*error, ServeError::Connection(_)) && matches!(*cleanup, ServeError::Unrestored(_))));
    }

    #[test]
    fn a_check_failing_ends_the_wait_for_gdb() {
        let check = || Err(Stop);
        let error = accept("127.0.0.1:0", |_| Ok(()), &check).unwrap_err();
        assert!(matches!(error, ServeError::Host(Stop)));
    }

    #[test]
    fn an_address_that_cannot_be_bound_is_named() {
        let check = || Ok::<(), Stop>(());
        let error = accept("256.0.0.1:0", |_| Ok(()), &check).unwrap_err();
        assert!(matches!(&error, ServeError::Listen { address, .. } if address == "256.0.0.1:0"));
    }
}
