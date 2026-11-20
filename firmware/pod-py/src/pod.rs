//! The pod as a Python object: one open connection across calls, each
//! call a `PodTarget` or `Pod` call with the GIL released while it
//! waits on the pod.

use std::fmt;
use std::sync::Mutex;
use std::sync::PoisonError;

use mc683xx_target::Cpu32Target;
use mc683xx_target::address::Address;
use mc683xx_target::register::Register;
use pod_host::pod;
use pod_host::pod::Pod;
use pod_host::register::Named;
use pod_host::register::SYSTEM_REGISTERS;
use pod_host::target::PodTarget;
use pod_host::transport::PodAddress;
use pod_host::transport::Transport;
use pod_proto::bdm::byte_count;
use pyo3::Bound;
use pyo3::PyAny;
use pyo3::PyResult;
use pyo3::Python;
use pyo3::pybacked::PyBackedBytes;
use pyo3::pyclass;
use pyo3::pymethods;
use pyo3::types::PyBytes;
use pyo3::types::PyDict;
use pyo3::types::PyDictMethods;

use crate::access;
use crate::error::Error;
use crate::types::Atemp;
use crate::types::Entry;
use crate::types::Pins;

/// The target through the pod, on whichever transport opened it.
type Target = PodTarget<Box<dyn Transport + Send>>;

/// The default function code: supervisor data (CPU32RM Table 5-1), as
/// `bdm`'s and the emulator's.
const SUPERVISOR_DATA: u32 = 5;

/// The pod (Rust's `PodTarget`), open from construction until `close`.
/// One thread at a time: a call from another thread while one waits on
/// the pod raises `RuntimeError`.
#[pyclass(name = "Pod", module = "mc683xx_bdm")]
pub(crate) struct PodView {
    /// What the pod was opened as: `usb`, `usb:SERIAL`, `unix:PATH`,
    /// `tcp:HOST:PORT`.
    name: String,
    /// The target, `None` once closed.  A mutex only so the object is
    /// `Sync`, as Python objects must be; calls take it mutably.
    target: Mutex<Option<Target>>,
}

impl fmt::Debug for PodView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("PodView").field("name", &self.name).finish_non_exhaustive()
    }
}

impl PodView {
    /// Runs `call` on the open target with the GIL released.
    fn with<R: Send>(
        &mut self,
        py: Python<'_>,
        call: impl Send + FnOnce(&mut Target) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let target = self.target.get_mut().unwrap_or_else(PoisonError::into_inner);
        let target = target.as_mut().ok_or(Error::Closed)?;
        py.detach(|| call(target))
    }

    /// Runs `call` on the pod's own requests, as [`Self::with`].
    fn with_pod<R: Send>(
        &mut self,
        py: Python<'_>,
        call: impl Send + FnOnce(&mut Pod<Box<dyn Transport + Send>>) -> Result<R, pod::Error>,
    ) -> Result<R, Error> {
        self.with(py, |target| Ok(call(target.pod())?))
    }
}

#[pymethods]
impl PodView {
    /// Opens `pod`, as `bdm --pod` names it: `usb`, the only pod on
    /// USB, `usb:SERIAL`, the one with that serial number, or a stand-in
    /// on `unix:PATH` or `tcp:HOST:PORT`.  Says Hello and raises
    /// `ProtocolError` if the pod speaks another protocol.
    #[new]
    #[pyo3(signature = (pod = "usb"))]
    fn new(py: Python<'_>, pod: &str) -> Result<Self, Error> {
        let address: PodAddress = pod.parse()?;
        let target = py.detach(|| -> Result<Target, Error> {
            let mut target = PodTarget::new(Pod::new(address.connect()?));
            target.pod().check_protocol()?;
            Ok(target)
        })?;
        Ok(Self { name: pod.to_owned(), target: Mutex::new(Some(target)) })
    }

    /// Puts back the program's SFC and DFC where a memory call changed
    /// them, as `bdm` does before it exits, and closes the pod.  Closing
    /// a closed pod does nothing.
    fn close(&mut self, py: Python<'_>) -> Result<(), Error> {
        let target = self.target.get_mut().unwrap_or_else(PoisonError::into_inner).take();
        let Some(mut target) = target else { return Ok(()) };
        py.detach(move || target.restore_function_codes())?;
        Ok(())
    }

    /// Whether `close` has been called.
    #[getter]
    fn closed(&mut self) -> bool {
        self.target.get_mut().unwrap_or_else(PoisonError::into_inner).is_none()
    }

    const fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    /// Closes the pod; an exception from the block goes on.
    fn __exit__(
        &mut self,
        py: Python<'_>,
        _kind: &Bound<'_, PyAny>,
        _value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> Result<bool, Error> {
        self.close(py)?;
        Ok(false)
    }

    fn __repr__(&mut self) -> String {
        let state = if self.closed() { "closed" } else { "open" };
        format!("<Pod {:?} {state}>", self.name)
    }

    /// The pod's protocol version (Hello).
    fn hello(&mut self, py: Python<'_>) -> Result<u32, Error> {
        self.with_pod(py, Pod::hello)
    }

    /// The pod firmware's version: crate version and git commit.
    fn version(&mut self, py: Python<'_>) -> Result<String, Error> {
        self.with_pod(py, Pod::version)
    }

    /// FREEZE, RESET, DS and BERR levels, target VDD, and ATEMP.
    fn pins(&mut self, py: Python<'_>) -> Result<Pins, Error> {
        Ok(self.with_pod(py, Pod::pins)?.into())
    }

    /// Stops the CPU in background mode, if it is not already, and its
    /// `Entry`.  Raises `TargetError` if it does not enter.
    fn halt(&mut self, py: Python<'_>) -> Result<Entry, Error> {
        Ok(self.with(py, |target| Ok(target.halt()?))?.into())
    }

    /// RESET with BKPT held: background mode enabled and the CPU in it
    /// before its first instruction, and its `Entry`.
    fn reset_with_breakpoint(&mut self, py: Python<'_>) -> Result<Entry, Error> {
        Ok(self.with(py, |target| Ok(target.reset_halted()?))?.into())
    }

    /// RESET alone: the CPU runs, background mode off.
    fn reset(&mut self, py: Python<'_>) -> Result<(), Error> {
        self.with(py, |target| Ok(target.reset_running()?))
    }

    /// GO: resumes at RPC, the program's SFC and DFC put back first.
    fn go(&mut self, py: Python<'_>) -> Result<(), Error> {
        self.with(py, |target| Ok(target.go()?))
    }

    /// CALL: stacks RPC and runs from `address`, SFC and DFC put back
    /// first.
    fn call(&mut self, py: Python<'_>, address: u32) -> Result<(), Error> {
        self.with(py, |target| Ok(target.call(Address(address))?))
    }

    /// Sets DSCLK's rate, and the rate set.
    fn set_clock(&mut self, py: Python<'_>, dsclk_hz: u32) -> Result<u32, Error> {
        self.with_pod(py, |pod| pod.set_clock(dsclk_hz))
    }

    /// Brings a halted CPU's serial link back into frame, after a pod
    /// restart, without a reset; ATEMP, stale.
    fn resync(&mut self, py: Python<'_>) -> Result<Atemp, Error> {
        Ok(self.with(py, |target| Ok(target.resync()?))?.into())
    }

    /// The `Entry` while the CPU is in background mode, `None` while it
    /// runs: asks the pod each time until it sees a halt.
    #[getter]
    fn background(&mut self, py: Python<'_>) -> Result<Option<Entry>, Error> {
        Ok(self.with(py, |target| Ok(target.halted()?))?.map(Entry::from))
    }

    /// Register `name`, in any case: `D0`-`D7`, `A0`-`A7`, `SP` (A7),
    /// `PC` (RPC), or a system register (`RPC`, `PCC`, `ATEMP`, `FAR`,
    /// `VBR`, `SR`, `USP`, `SSP`, `SFC`, `DFC`).  SFC and DFC are the
    /// program's, whatever a memory call set.
    fn read_register(&mut self, py: Python<'_>, name: &str) -> Result<u32, Error> {
        let register = Named::parse(name)?;
        self.with(py, |target| Ok(register.read(target)?))
    }

    /// Sets register `name` to `value`.
    fn write_register(&mut self, py: Python<'_>, name: &str, value: u32) -> Result<(), Error> {
        let register = Named::parse(name)?;
        self.with(py, |target| Ok(register.write(target, value)?))
    }

    /// Every data, address and system register but ATEMP, by name:
    /// `D0`-`A7`, then `RPC`, `PCC`, `SR`, `USP`, `SSP`, `VBR`, `SFC`,
    /// `DFC`, `FAR`.
    fn registers<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let values = self.with(py, |target| {
            let mut values = Vec::with_capacity(Register::ALL.len() + SYSTEM_REGISTERS.len());
            for register in Register::ALL {
                values.push((register.to_string(), target.read_register(register)?));
            }
            for register in SYSTEM_REGISTERS {
                values.push((register.to_string(), target.read_system_register(register)?));
            }
            Ok(values)
        })?;
        let dictionary = PyDict::new(py);
        for (name, value) in values {
            dictionary.set_item(name.to_uppercase(), value)?;
        }
        Ok(dictionary)
    }

    /// `length` bytes from `address` in accesses of `size` bytes (1, 2
    /// or 4), in `function_code`'s space (5, supervisor data, by
    /// default).  Raises `BusError` if the target's bus faults.
    #[pyo3(signature = (address, length, *, size = 1, function_code = SUPERVISOR_DATA))]
    fn read_memory<'py>(
        &mut self,
        py: Python<'py>,
        address: u32,
        length: usize,
        size: u32,
        function_code: u32,
    ) -> Result<Bound<'py, PyBytes>, Error> {
        let (size, function_code) = (access::size(size)?, access::function_code(function_code)?);
        let data = self.with(py, |target| {
            let mut data = vec![0; length];
            target.read_memory(Address(address), function_code, size, &mut data)?;
            Ok(data)
        })?;
        Ok(PyBytes::new(py, &data))
    }

    /// Writes `data` at `address`, as `read_memory` reads.
    #[pyo3(signature = (address, data, *, size = 1, function_code = SUPERVISOR_DATA))]
    fn write_memory(
        &mut self,
        py: Python<'_>,
        address: u32,
        data: PyBackedBytes,
        size: u32,
        function_code: u32,
    ) -> Result<(), Error> {
        let (size, function_code) = (access::size(size)?, access::function_code(function_code)?);
        self.with(py, |target| Ok(target.write_memory(Address(address), function_code, size, &data)?))
    }

    /// One access of `size` bytes (1, 2 or 4) at `address` in
    /// `function_code`'s space, its value; as the emulator's `bus_read`.
    #[pyo3(signature = (address, size, function_code = SUPERVISOR_DATA))]
    fn bus_read(
        &mut self,
        py: Python<'_>,
        address: u32,
        size: u32,
        function_code: u32,
    ) -> Result<u32, Error> {
        let (size, function_code) = (access::size(size)?, access::function_code(function_code)?);
        let bytes = self.with(py, |target| {
            let mut bytes = [0; 4];
            let access = &mut bytes[..byte_count(size)];
            target.read_memory(Address(address), function_code, size, access)?;
            Ok(bytes)
        })?;
        Ok(access::decode(&bytes[..byte_count(size)]))
    }

    /// Writes `value`, which must fit, as `bus_read` reads.
    #[pyo3(signature = (address, size, value, function_code = SUPERVISOR_DATA))]
    fn bus_write(
        &mut self,
        py: Python<'_>,
        address: u32,
        size: u32,
        value: u32,
        function_code: u32,
    ) -> Result<(), Error> {
        let (size, function_code) = (access::size(size)?, access::function_code(function_code)?);
        let bytes = access::encode(size, value)?;
        self.with(py, |target| {
            let access = &bytes[..byte_count(size)];
            Ok(target.write_memory(Address(address), function_code, size, access)?)
        })
    }

    /// Puts back the program's SFC and DFC where a memory call changed
    /// them, without resuming.
    fn restore_function_codes(&mut self, py: Python<'_>) -> Result<(), Error> {
        self.with(py, |target| Ok(target.restore_function_codes()?))
    }

    /// Raw serial words, each answered with the 17 bits shifted in.
    /// Unguarded: with the CPU running the clocks are BKPT pulses, and
    /// what they do to the CPU is not tracked until the next reset.
    fn transfer(&mut self, py: Python<'_>, words: Vec<u16>) -> Result<Vec<u32>, Error> {
        self.with_pod(py, |pod| pod.transfer(&words))
    }
}
