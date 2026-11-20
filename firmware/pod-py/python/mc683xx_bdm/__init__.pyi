"""The mc683xx CPU32 BDM pod, for Python scripts: one open pod across
calls (PLAN.md, Host)."""

from types import TracebackType
from typing import final

PROTOCOL_VERSION: int

class BusError(Exception):
    """Raised by a memory access the target ended with a bus error, as
    the emulator's mc683xx.BusError."""

class TargetError(RuntimeError):
    """Raised when the pod or the target refuses a call: no target power,
    the CPU not in background mode for a call that needs it (or not
    entering it, for halt), an illegal command, the target not ready,
    or a response the command does not allow.  As the emulator's
    mc683xx.TargetError."""

class TransportError(OSError):
    """Raised when the pod cannot be reached or the packets do not get
    through: no pod on USB, several and none named, none with the
    serial number named, a socket that cannot be reached, a failed or
    timed-out transfer."""

class ProtocolError(RuntimeError):
    """Raised when the pod and the host do not understand each other:
    another protocol version, a request the pod cannot parse, or a
    malformed reply."""

class Entry:
    """Why the CPU entered background mode (CPU32RM Table 7-2), as the
    emulator's mc683xx.Entry."""

    def __hash__(self) -> int: ...
    @final
    class Breakpoint(Entry):
        """BKPT asserted."""

        def __init__(self) -> None: ...

    @final
    class Background(Entry):
        """A BGND instruction."""

        def __init__(self) -> None: ...

    @final
    class DoubleBusFault(Entry):
        """A double bus fault, with the second fault's SSW."""

        def __init__(self, ssw: int) -> None: ...
        @property
        def ssw(self) -> int: ...

    @final
    class Unknown(Entry):
        """An ATEMP Table 7-2 does not list, or one the pod read when it
        had not seen the entry."""

        def __init__(self, atemp: int) -> None: ...
        @property
        def atemp(self) -> int: ...

class Atemp:
    """ATEMP as the pod read it for the CPU's current stay in background
    mode."""

    def __hash__(self) -> int: ...
    @final
    class Running(Atemp):
        """The CPU is not in background mode."""

        def __init__(self) -> None: ...

    @final
    class Fresh(Atemp):
        """Read first after entry: the entry source."""

        def __init__(self, value: int) -> None: ...
        @property
        def value(self) -> int: ...

    @final
    class Stale(Atemp):
        """Read when the pod had not seen the entry: not an entry source."""

        def __init__(self, value: int) -> None: ...
        @property
        def value(self) -> int: ...

    @final
    class Unread(Atemp):
        """In background mode, but the read after entry failed."""

        def __init__(self) -> None: ...

@final
class Pins:
    """The target's pins as the pod reads them: each level true when
    high."""

    @property
    def freeze(self) -> bool: ...
    @property
    def reset(self) -> bool: ...
    @property
    def ds(self) -> bool: ...
    @property
    def berr(self) -> bool: ...
    @property
    def vdd_mv(self) -> int: ...
    @property
    def atemp(self) -> Atemp: ...

@final
class Pod:
    """The pod, open from construction until close.  One thread at a
    time."""

    def __init__(self, pod: str = "usb") -> None: ...
    def close(self) -> None: ...
    @property
    def closed(self) -> bool: ...
    def __enter__(self) -> Pod: ...
    def __exit__(
        self,
        kind: type[BaseException] | None,
        value: BaseException | None,
        traceback: TracebackType | None,
    ) -> bool: ...
    def hello(self) -> int: ...
    def version(self) -> str: ...
    def pins(self) -> Pins: ...
    def halt(self) -> Entry: ...
    def reset_with_breakpoint(self) -> Entry: ...
    def reset(self) -> None: ...
    def go(self) -> None: ...
    def call(self, address: int) -> None: ...
    def set_clock(self, dsclk_hz: int) -> int: ...
    def resync(self) -> Atemp: ...
    @property
    def background(self) -> Entry | None: ...
    def read_register(self, name: str) -> int: ...
    def write_register(self, name: str, value: int) -> None: ...
    def registers(self) -> dict[str, int]: ...
    def read_memory(
        self, address: int, length: int, *, size: int = 1, function_code: int = 5
    ) -> bytes: ...
    def write_memory(
        self,
        address: int,
        data: bytes | bytearray,
        *,
        size: int = 1,
        function_code: int = 5,
    ) -> None: ...
    def bus_read(self, address: int, size: int, function_code: int = 5) -> int: ...
    def bus_write(self, address: int, size: int, value: int, function_code: int = 5) -> None: ...
    def restore_function_codes(self) -> None: ...
    def transfer(self, words: list[int]) -> list[int]: ...
