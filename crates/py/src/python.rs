//! The Python module `mc683xx_bdm`, and the bindings' errors as Python
//! exceptions.

use mc683xx_bdm::gdb::ServeError;
use mc683xx_bdm::pod;
use mc683xx_bdm::transport;
use mc683xx_bdm_proto::usb::PROTOCOL_VERSION;
use mc683xx_bdm_proto::usb::ReplyError;
use mc683xx_bdm_proto::usb::Status;
use pyo3::Bound;
use pyo3::PyErr;
use pyo3::PyResult;
use pyo3::Python;
use pyo3::exceptions::PyException;
use pyo3::exceptions::PyOSError;
use pyo3::exceptions::PyRuntimeError;
use pyo3::exceptions::PyValueError;
use pyo3::pymodule;
use pyo3::types::PyModule;
use pyo3::types::PyModuleMethods;

use crate::error::Error;
use crate::error::message;
use crate::pod::PodView;
use crate::types::Atemp;
use crate::types::Ending;
use crate::types::Entry;
use crate::types::Pins;

pyo3::create_exception!(
    mc683xx_bdm,
    BusError,
    PyException,
    "Raised by a memory access the target ended with a bus error, as \
     the emulator's mc683xx.BusError."
);

pyo3::create_exception!(
    mc683xx_bdm,
    TargetError,
    PyRuntimeError,
    "Raised when the pod or the target refuses a call: no target power, \
     the CPU not in background mode for a call that needs it (or not \
     entering it, for halt), an illegal command, the target not ready, \
     or a response the command does not allow.  As the emulator's \
     mc683xx.TargetError."
);

pyo3::create_exception!(
    mc683xx_bdm,
    TransportError,
    PyOSError,
    "Raised when the pod cannot be reached or the packets do not get \
     through: no pod on USB, several and none named, none with the \
     serial number named, a socket that cannot be reached, a failed or \
     timed-out transfer."
);

pyo3::create_exception!(
    mc683xx_bdm,
    ProtocolError,
    PyRuntimeError,
    "Raised when the pod and the host do not understand each other: \
     another protocol version, a request the pod cannot parse, or a \
     malformed reply."
);

impl From<Error> for PyErr {
    fn from(error: Error) -> Self {
        let text = message(&error);
        match error {
            Error::Pod(pod::Error::Transport(transport::TransportError::BadAddress(_))) => {
                PyValueError::new_err(text)
            }
            Error::Pod(pod::Error::Transport(_)) => TransportError::new_err(text),
            Error::Pod(pod::Error::Reply(ReplyError::Failed(Status::BusError))) => BusError::new_err(text),
            Error::Pod(
                pod::Error::Reply(ReplyError::Failed(Status::BadRequest) | ReplyError::Malformed)
                | pod::Error::Protocol(_),
            ) => ProtocolError::new_err(text),
            Error::Pod(pod::Error::Reply(ReplyError::Failed(_)) | pod::Error::Step(_)) => {
                TargetError::new_err(text)
            }
            Error::Pod(pod::Error::Request(_) | pod::Error::Length { .. })
            | Error::Register(_)
            | Error::Size(_)
            | Error::FunctionCode(_)
            | Error::Width { .. }
            | Error::Closed => PyValueError::new_err(text),
        }
    }
}

/// A gdb session's error as Python's: the host's own exception (Ctrl-C's
/// `KeyboardInterrupt`, `ready`'s) raised again, with a failure cleaning
/// up after it as its `__cause__`; the pod's as any call's, a listener
/// that cannot be bound an `OSError`, the rest `TargetError`.
pub(crate) fn served(py: Python<'_>, error: ServeError<PyErr>) -> PyErr {
    let text = message(&error);
    match error {
        ServeError::Host(error) => error,
        ServeError::Uncleaned { error, cleanup } if matches!(*error, ServeError::Host(_)) => {
            let host = served(py, *error);
            host.set_cause(py, Some(served(py, *cleanup)));
            host
        }
        ServeError::Target(error) => Error::Pod(error).into(),
        ServeError::Listen { .. } => PyOSError::new_err(text),
        _ => TargetError::new_err(text),
    }
}

/// The mc683xx CPU32 BDM pod, for Python scripts: one open pod across
/// calls (`Pod`).  Named in Python only: in Rust, `mc683xx_bdm` is the
/// host crate.
#[pymodule(name = "mc683xx_bdm")]
fn python_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PodView>()?;
    module.add_class::<Entry>()?;
    module.add_class::<Atemp>()?;
    module.add_class::<Pins>()?;
    module.add_class::<Ending>()?;
    module.add("BusError", module.py().get_type::<BusError>())?;
    module.add("TargetError", module.py().get_type::<TargetError>())?;
    module.add("TransportError", module.py().get_type::<TransportError>())?;
    module.add("ProtocolError", module.py().get_type::<ProtocolError>())?;
    module.add("PROTOCOL_VERSION", PROTOCOL_VERSION)?;
    Ok(())
}
