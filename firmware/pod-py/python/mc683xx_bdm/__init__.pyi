"""The mc683xx CPU32 BDM pod, for Python scripts: one open pod across
calls (PLAN.md, Host).  `make lint` checks these stubs against the
module with mypy's stubtest."""

from collections.abc import Callable
from types import TracebackType
from typing import ClassVar, final

from typing_extensions import disjoint_base

__all__ = [
    "PROTOCOL_VERSION",
    "Atemp",
    "BusError",
    "Ending",
    "Entry",
    "Pins",
    "Pod",
    "ProtocolError",
    "TargetError",
    "TransportError",
]

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

@disjoint_base
class Entry:
    """Why the CPU entered background mode (CPU32RM Table 7-2), as the
    emulator's mc683xx.Entry."""

    def __hash__(self) -> int: ...
    @final
    class Breakpoint(Entry):
        """BKPT asserted."""

        __match_args__ = ()
        def __new__(cls) -> Entry.Breakpoint: ...

    @final
    class Background(Entry):
        """A BGND instruction."""

        __match_args__ = ()
        def __new__(cls) -> Entry.Background: ...

    @final
    class DoubleBusFault(Entry):
        """A double bus fault, with the second fault's SSW."""

        __match_args__ = ("ssw",)
        def __new__(cls, ssw: int) -> Entry.DoubleBusFault: ...
        @property
        def ssw(self) -> int: ...

    @final
    class Unknown(Entry):
        """An ATEMP Table 7-2 does not list, or one the pod read when it
        had not seen the entry."""

        __match_args__ = ("atemp",)
        def __new__(cls, atemp: int) -> Entry.Unknown: ...
        @property
        def atemp(self) -> int: ...

@disjoint_base
class Atemp:
    """ATEMP as the pod read it for the CPU's current stay in background
    mode."""

    def __hash__(self) -> int: ...
    @final
    class Running(Atemp):
        """The CPU is not in background mode."""

        __match_args__ = ()
        def __new__(cls) -> Atemp.Running: ...

    @final
    class Fresh(Atemp):
        """Read first after entry: the entry source."""

        __match_args__ = ("value",)
        def __new__(cls, value: int) -> Atemp.Fresh: ...
        @property
        def value(self) -> int: ...

    @final
    class Stale(Atemp):
        """Read when the pod had not seen the entry: not an entry source."""

        __match_args__ = ("value",)
        def __new__(cls, value: int) -> Atemp.Stale: ...
        @property
        def value(self) -> int: ...

    @final
    class Unread(Atemp):
        """In background mode, but the read after entry failed."""

        __match_args__ = ()
        def __new__(cls) -> Atemp.Unread: ...

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
class Ending:
    """How a gdb session ended, as `serve_gdb` returns it; as the
    emulator's mc683xx.Ending."""

    DETACHED: ClassVar[Ending]
    KILLED: ClassVar[Ending]
    def __hash__(self) -> int: ...
    def __int__(self) -> int: ...

@final
class Pod:
    """The pod, open from construction until close.  One thread at a
    time."""

    def __new__(cls, pod: str = "usb") -> Pod: ...
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
        self, address: int, length: int, *, size: int = 1, function_code: int = ...
    ) -> bytes: ...
    def write_memory(
        self,
        address: int,
        data: bytes | bytearray,
        *,
        size: int = 1,
        function_code: int = ...,
    ) -> None: ...
    def bus_read(self, address: int, size: int, function_code: int = ...) -> int: ...
    def bus_write(self, address: int, size: int, value: int, function_code: int = ...) -> None: ...
    def restore_function_codes(self) -> None: ...
    def serve_gdb(
        self,
        address: str = ...,
        ready: Callable[[int], object] | None = None,
        *,
        scratch: int,
    ) -> Ending: ...
    def transfer(self, words: list[int]) -> list[int]: ...
