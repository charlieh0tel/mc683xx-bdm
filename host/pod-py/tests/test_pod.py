"""The pod from Python, end to end against bdm-stand-in: one open pod
across calls, the target's halts, registers and memory, and the errors
as exceptions."""

import socket
import struct
import threading

import pytest

import mc683xx_bdm
from conftest import RAM_BASE, RAM_BYTES
from mc683xx_bdm import Atemp, Entry, Pod

# Function codes (CPU32RM Table 5-1).
USER_DATA = 1
SUPERVISOR_DATA = 5
# An address no model memory answers.
UNMAPPED = 0x10
# The Hello request's code and the OK status (docs/usb-protocol.md).
HELLO = 0x0E
OK = 0


def test_a_session(stand_in):
    with Pod(stand_in()) as pod:
        assert pod.hello() == mc683xx_bdm.PROTOCOL_VERSION
        assert pod.reset_with_breakpoint() == Entry.Breakpoint()
        pod.write_register("D3", 0xDEADBEEF)
        assert pod.read_register("d3") == 0xDEADBEEF
        pod.write_register("sp", 0x1000)
        assert pod.read_register("A7") == 0x1000
        pod.write_register("PC", RAM_BASE)
        assert pod.read_register("RPC") == RAM_BASE
        registers = pod.registers()
        assert list(registers)[:16] == [f"D{n}" for n in range(8)] + [f"A{n}" for n in range(8)]
        assert list(registers)[16:] == [
            "RPC",
            "PCC",
            "SR",
            "USP",
            "SSP",
            "VBR",
            "SFC",
            "DFC",
            "FAR",
        ]
        assert registers["D3"] == 0xDEADBEEF
        assert registers["RPC"] == RAM_BASE


def test_the_pod_stays_open_across_calls(stand_in):
    pod = Pod(stand_in())
    pod.reset_with_breakpoint()
    for value in range(64):
        pod.write_register("D0", value)
        assert pod.read_register("D0") == value
    assert not pod.closed
    pod.close()
    assert pod.closed
    pod.close()
    with pytest.raises(ValueError, match="closed"):
        pod.hello()


def test_memory_in_every_size(stand_in):
    with Pod(stand_in()) as pod:
        pod.reset_with_breakpoint()
        data = bytes(range(200))
        pod.write_memory(RAM_BASE, data, size=2)
        assert pod.read_memory(RAM_BASE, len(data)) == data
        assert pod.read_memory(RAM_BASE, len(data), size=4) == data
        pod.write_memory(RAM_BASE + 1, bytearray(b"\xaa"), size=1)
        assert pod.read_memory(RAM_BASE, 2, size=2) == b"\x00\xaa"
        with pytest.raises(ValueError, match="multiple"):
            pod.read_memory(RAM_BASE, 3, size=2)
        with pytest.raises(ValueError, match="3-byte"):
            pod.read_memory(RAM_BASE, 3, size=3)


def test_single_accesses_are_big_endian(stand_in):
    with Pod(stand_in()) as pod:
        pod.reset_with_breakpoint()
        pod.bus_write(RAM_BASE, 4, 0x12345678)
        assert pod.read_memory(RAM_BASE, 4) == b"\x12\x34\x56\x78"
        assert pod.bus_read(RAM_BASE, 4) == 0x12345678
        assert pod.bus_read(RAM_BASE + 2, 2) == 0x5678
        assert pod.bus_read(RAM_BASE + 1, 1) == 0x34
        with pytest.raises(ValueError, match="does not fit"):
            pod.bus_write(RAM_BASE, 1, 0x100)


def test_function_codes_are_restored(stand_in):
    pod_name = stand_in()
    with Pod(pod_name) as pod:
        pod.reset_with_breakpoint()
        pod.write_register("SFC", USER_DATA)
        pod.write_register("DFC", USER_DATA)
        pod.write_memory(RAM_BASE, b"\x01\x02", function_code=SUPERVISOR_DATA)
        assert pod.read_memory(RAM_BASE, 2, function_code=SUPERVISOR_DATA) == b"\x01\x02"
        assert pod.read_register("sfc") == USER_DATA, "the program's"
        with pytest.raises(ValueError, match="function code 8"):
            pod.read_memory(RAM_BASE, 2, function_code=8)
    with Pod(pod_name) as pod:
        assert pod.halt() == Entry.Breakpoint()
        assert pod.read_register("SFC") == USER_DATA, "put back by close"
        assert pod.read_register("DFC") == USER_DATA, "put back by close"


def test_go_then_poll_then_halt(stand_in):
    with Pod(stand_in()) as pod:
        assert pod.reset_with_breakpoint() == Entry.Breakpoint()
        assert pod.background == Entry.Breakpoint()
        pod.go()
        assert pod.background is None
        assert pod.pins().atemp == Atemp.Running()
        with pytest.raises(mc683xx_bdm.TargetError, match="background mode"):
            pod.read_register("D0")
        assert pod.halt() == Entry.Breakpoint()
        assert pod.background == Entry.Breakpoint()
        pod.write_register("SP", RAM_BASE + RAM_BYTES)
        pod.call(RAM_BASE)
        assert pod.background is None
        pod.reset()
        assert pod.background is None


def test_pins_and_version(stand_in):
    with Pod(stand_in()) as pod:
        pod.reset_with_breakpoint()
        pins = pod.pins()
        assert pins.freeze
        assert pins.reset
        assert pins.vdd_mv > 0
        assert pins.atemp == Atemp.Fresh(value=0)
        assert "Atemp.Fresh(value=0x00000000)" in repr(pins)
        assert pod.version()


def test_a_bus_error_raises_bus_error(stand_in):
    with Pod(stand_in()) as pod:
        pod.reset_with_breakpoint()
        with pytest.raises(mc683xx_bdm.BusError):
            pod.read_memory(UNMAPPED, 2, size=2)
        assert pod.read_memory(RAM_BASE + RAM_BYTES - 2, 2) == b"\x00\x00", "works after"


def test_halt_without_background_mode_enabled(stand_in):
    with Pod(stand_in("--no-bdm")) as pod:
        with pytest.raises(mc683xx_bdm.TargetError):
            pod.halt()
        assert pod.reset_with_breakpoint() == Entry.Breakpoint()


def test_a_halt_the_pod_did_not_see_is_unknown(stand_in):
    with Pod(stand_in("--halted")) as pod:
        entry = pod.background
        assert isinstance(entry, Entry.Unknown)
        assert pod.pins().atemp == Atemp.Stale(value=entry.atemp)
        assert isinstance(pod.resync(), Atemp.Stale)


def test_clock_and_raw_transfer(stand_in):
    with Pod(stand_in()) as pod:
        pod.reset_with_breakpoint()
        assert pod.set_clock(1_000_000) > 0
        assert len(pod.transfer([0x0000] * 20)) == 20
        with pytest.raises(OverflowError):
            pod.transfer([0x10000])


def test_entries_read_as_the_emulators():
    assert str(Entry.Breakpoint()) == "breakpoint"
    assert repr(Entry.DoubleBusFault(ssw=0x1234)) == "Entry.DoubleBusFault(ssw=0x1234)"
    assert Entry.Unknown(atemp=3) == Entry.Unknown(atemp=3)
    assert len({Entry.Background(), Entry.Background()}) == 1


def test_bad_names_are_value_errors(stand_in):
    with pytest.raises(ValueError, match="usb, usb:SERIAL, unix:PATH or tcp:HOST:PORT"):
        Pod("serial:/dev/ttyACM0")
    with Pod(stand_in()) as pod:
        pod.reset_with_breakpoint()
        with pytest.raises(ValueError, match="no register D8"):
            pod.read_register("D8")


def test_no_pod_is_a_transport_error(tmp_path):
    with pytest.raises(mc683xx_bdm.TransportError) as raised:
        Pod(f"unix:{tmp_path / 'none.sock'}")
    assert isinstance(raised.value, OSError)
    assert "socket failed" in str(raised.value)


def hello_reply() -> bytes:
    """A Hello reply, OK and the protocol version, as a stream message
    (docs/usb-protocol.md, Transport)."""
    reply = bytes([OK]) + struct.pack("<I", mc683xx_bdm.PROTOCOL_VERSION)
    return bytes([len(reply)]) + reply


def test_the_gil_is_released_while_waiting_on_the_pod(tmp_path):
    """A pod answered by a Python thread: it can answer only if the
    calls release the GIL while they wait, or they time out."""
    path = str(tmp_path / "python.sock")
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(path)
    listener.listen(1)
    requests = []

    def serve():
        connection, _ = listener.accept()
        with connection:
            while length := connection.recv(1):
                requests.append(connection.recv(length[0]))
                connection.sendall(hello_reply())

    server = threading.Thread(target=serve)
    server.start()
    try:
        with Pod(f"unix:{path}") as pod:
            assert pod.hello() == mc683xx_bdm.PROTOCOL_VERSION
    finally:
        server.join()
        listener.close()
    assert requests == [bytes([HELLO]), bytes([HELLO])]
