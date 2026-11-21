//! What can go wrong in a Python host's call, before it becomes a
//! Python exception.

use mc683xx_bdm::register::UnknownRegister;
use mc683xx_bdm::transport::TransportError;

/// A call that failed: the pod's, or a host's mistake the bindings
/// refuse.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The pod, its transport or the target failed the call.
    #[error(transparent)]
    Pod(#[from] mc683xx_bdm::pod::Error),
    /// A register name that is not the pod's.
    #[error(transparent)]
    Register(#[from] UnknownRegister),
    /// An access size in bytes that is not 1, 2 or 4.
    #[error("no {0}-byte access: give 1, 2 or 4")]
    Size(u32),
    /// A function code wider than FC\[2:0\].
    #[error("no function code {0}: FC[2:0] is 0 to 7")]
    FunctionCode(u32),
    /// A value wider than its access.
    #[error("{value:#x} does not fit {bytes} bytes")]
    Width {
        /// The value.
        value: u32,
        /// The access size in bytes.
        bytes: u32,
    },
    /// A call after `close`.
    #[error("the pod is closed")]
    Closed,
}

impl From<TransportError> for Error {
    fn from(error: TransportError) -> Self {
        Self::Pod(error.into())
    }
}

/// `error` and each of its sources, joined by `: `, as `bdm` prints
/// them, so an exception's message says why a socket or USB failed.
pub(crate) fn message(error: &dyn std::error::Error) -> String {
    std::iter::successors(Some(error), |error| error.source())
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

#[cfg(test)]
mod tests {
    use mc683xx_bdm::transport::TransportError;

    use super::Error;
    use super::message;

    #[test]
    fn messages_carry_their_sources() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "no such socket");
        let error = Error::from(TransportError::Socket(io));
        assert_eq!(message(&error), "socket failed: no such socket");
        assert_eq!(message(&Error::Closed), "the pod is closed");
    }
}
