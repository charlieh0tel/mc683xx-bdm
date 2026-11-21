"""`Pod.serve_gdb` end to end against bdm-stand-in: a client in a thread
sends gdb's remote serial protocol packets raw (the gdb manual, Remote
Protocol), as mc683xx-bdm's tests/gdb.rs does for `bdm gdb`; no gdb
needed.  The client starts once the stub listens (`ready`), on the port
it picked."""

import re
import signal
import socket
import threading
import time
from collections.abc import Callable, Iterator
from contextlib import contextmanager

import pytest

from conftest import RAM_BASE, launch
from mc683xx_bdm import BusError, Ending, Entry, Pod, TransportError

# The program, the step's scratch RAM and the supervisor stack.
PROGRAM = RAM_BASE
SCRATCH = RAM_BASE + 0x800
STACK = RAM_BASE + 0x1000
# moveq #5,d0; moveq #-1,d1; nop; bra.s to itself (CPU32RM 4).  The
# model follows the branch 1000 times, then runs on unwatched.
CODE = bytes.fromhex("700572ff4e7160fe")
NOP_AT = PROGRAM + 4
# A breakpoint the program never reaches, and the word under it.
UNREACHED = PROGRAM + 0x20
UNREACHED_WORD = bytes.fromhex("4e71")
SUPERVISOR = 0x2700
# An address no model memory answers: no room for a step's table.
UNMAPPED = 0x10
# gdb's Ctrl-C.
INTERRUPT = b"\x03"
# PC's index in a `g` reply, and hex digits in a register.
PC = 17
REGISTER_DIGITS = 8
# Run-length counts are a character's code less this.
RUN_LENGTH_BIAS = 29
# How long a session may take to end, and how long the client waits
# after a continue before the test's signal.
PROMPTLY_S = 5.0
RUNNING_S = 0.2


class Alarm(Exception):
    """What the test's signal handler raises, as Ctrl-C raises
    KeyboardInterrupt."""


def packet(body: str) -> bytes:
    return f"${body}#{sum(body.encode()) & 0xFF:02x}".encode()


def decoded(body: str) -> str:
    out: list[str] = []
    index = 0
    while index < len(body):
        if body[index] == "*":
            out.append(out[-1] * (ord(body[index + 1]) - RUN_LENGTH_BIAS))
            index += 2
        else:
            out.append(body[index])
            index += 1
    return "".join(out)


class Client:
    """gdb's end of the connection, in a thread: sends each item once
    the last is answered and keeps the replies.  An item is a packet's
    body; one ending in `!` is sent without waiting for its answer (a
    continue gdb will interrupt); `^C` sends gdb's Ctrl-C.  Then it
    calls `then` and stays connected until `done`, set once the session
    has ended."""

    def __init__(self, items: list[str], then: Callable[[], None] = lambda: None) -> None:
        self.items = items
        self.then = then
        self.replies: list[str] = []
        self.thread: threading.Thread | None = None
        self.done = threading.Event()

    def ready(self, port: int) -> None:
        self.thread = threading.Thread(target=self.converse, args=(port,))
        self.thread.start()

    def converse(self, port: int) -> None:
        with socket.create_connection(("127.0.0.1", port)) as gdb:
            gdb.settimeout(PROMPTLY_S)
            pending = b""
            for item in self.items:
                if item == "^C":
                    gdb.sendall(INTERRUPT)
                elif item.endswith("!"):
                    gdb.sendall(packet(item[:-1]))
                    continue
                else:
                    gdb.sendall(packet(item))
                while not (match := re.search(rb"\$([^#]*)#..", pending)):
                    data = gdb.recv(4096)
                    if not data:
                        return
                    pending += data
                pending = pending[match.end() :]
                gdb.sendall(b"+")
                self.replies.append(decoded(match.group(1).decode()))
            self.then()
            self.done.wait(timeout=PROMPTLY_S)

    def join(self) -> None:
        self.done.set()
        assert self.thread is not None, "ready was never called"
        self.thread.join(timeout=PROMPTLY_S)


def load(pod: Pod) -> None:
    """Halts `pod`'s target at PROGRAM with CODE there."""
    assert pod.reset_with_breakpoint() == Entry.Breakpoint()
    pod.write_memory(PROGRAM, CODE, size=2)
    pod.write_memory(UNREACHED, UNREACHED_WORD, size=2)
    pod.write_register("SR", SUPERVISOR)
    pod.write_register("SP", STACK)
    pod.write_register("PC", PROGRAM)


@pytest.fixture
def pod(stand_in) -> Iterator[Pod]:
    """A pod on a stand-in, halted at PROGRAM with CODE there."""
    with Pod(stand_in()) as pod:
        load(pod)
        yield pod


def serve(pod: Pod, client: Client) -> Ending:
    try:
        return pod.serve_gdb("127.0.0.1:0", ready=client.ready, scratch=SCRATCH)
    finally:
        client.join()


def pc(registers: str) -> int:
    at = PC * REGISTER_DIGITS
    return int(registers[at : at + REGISTER_DIGITS], 16)


@contextmanager
def alarm(first: Callable[[], None] = lambda: None) -> Iterator[Callable[[], None]]:
    """A function that sends this thread SIGALRM, its handler calling
    `first` and then raising `Alarm`, for the length of the block."""

    def handler(number: int, frame: object) -> None:
        first()
        raise Alarm

    main = threading.get_ident()
    previous = signal.signal(signal.SIGALRM, handler)
    try:
        yield lambda: signal.pthread_kill(main, signal.SIGALRM)
    finally:
        signal.signal(signal.SIGALRM, previous)


def test_step_break_continue_interrupt_and_detach(pod):
    client = Client(
        [
            "?",
            f"m{PROGRAM:x},8",
            "s",
            "g",
            f"Z0,{NOP_AT:x},2",
            "c",
            "g",
            f"z0,{NOP_AT:x},2",
            f"m{NOP_AT:x},2",
            "c!",
            "^C",
            "D",
        ]
    )
    assert serve(pod, client) == Ending.DETACHED
    replies = client.replies
    assert replies[0].startswith("T05"), "halted for gdb"
    assert replies[1] == CODE.hex()
    assert replies[2] == "S05", "a step is a trap"
    assert pc(replies[3]) == PROGRAM + 2
    assert replies[4] == "OK"
    assert replies[5] == "T05thread:01;swbreak:;", "stopped at the breakpoint"
    assert pc(replies[6]) == NOP_AT
    assert replies[7] == "OK"
    assert replies[8] == "4e71", "the NOP back"
    assert replies[9] == "S02", "Ctrl-C halts it"
    assert replies[10] == "OK"
    assert pod.background is None, "detach leaves it running"
    pod.halt()
    assert pod.read_register("D0") == 5
    assert pod.read_register("D1") == 0xFFFFFFFF


def test_kill_leaves_it_halted(pod):
    client = Client(["?", "k!"])
    assert serve(pod, client) == Ending.KILLED
    assert pod.background is not None


def test_the_pod_is_the_caller_s_again_after_a_session(pod):
    serve(pod, Client(["D"]))
    pod.halt()
    pod.write_register("PC", PROGRAM)
    assert serve(pod, Client(["g", "D"])) == Ending.DETACHED


def test_a_signal_ends_the_wait_for_gdb(pod):
    with alarm() as send, pytest.raises(Alarm):
        pod.serve_gdb("127.0.0.1:0", ready=lambda port: send(), scratch=SCRATCH)
    assert pod.read_register("PC") == PROGRAM, "untouched, and the pod free"


def test_a_signal_while_running_removes_the_breakpoints(pod):
    with alarm() as send:

        def signal_when_running() -> None:
            time.sleep(RUNNING_S)
            send()

        client = Client([f"Z0,{UNREACHED:x},2", "c!"], then=signal_when_running)
        started = time.monotonic()
        with pytest.raises(Alarm):
            serve(pod, client)
        assert time.monotonic() - started < PROMPTLY_S
    assert client.replies == ["OK"]
    assert pod.background is not None, "halted"
    assert pod.read_memory(UNREACHED, 2, size=2) == UNREACHED_WORD, "BGND gone"


def test_a_signal_whose_clean_up_fails_is_raised(stand_in_binary, tmp_path):
    socket = tmp_path / "pod.sock"
    process = launch(stand_in_binary, socket)

    def lose_the_pod() -> None:
        process.kill()
        process.wait()

    try:
        pod = Pod(f"unix:{socket}")
        load(pod)
        with alarm(first=lose_the_pod) as send:

            def signal_when_running() -> None:
                time.sleep(RUNNING_S)
                send()

            client = Client([f"Z0,{UNREACHED:x},2", "c!"], then=signal_when_running)
            with pytest.raises(Alarm) as raised:
                serve(pod, client)
        assert isinstance(raised.value.__cause__, TransportError), "the clean-up's failure"
        pod.close()
    finally:
        lose_the_pod()


def test_ready_s_exception_is_raised(pod):
    def ready(port: int) -> None:
        raise ValueError(f"not now, {port}")

    with pytest.raises(ValueError, match="not now"):
        pod.serve_gdb("127.0.0.1:0", ready=ready, scratch=SCRATCH)


def test_an_address_that_cannot_be_bound_is_an_os_error(pod):
    with pytest.raises(OSError, match=re.escape("listening for gdb on 256.0.0.1:0")):
        pod.serve_gdb("256.0.0.1:0", scratch=SCRATCH)


def test_scratch_is_required_and_a_closed_pod_refuses(pod):
    with pytest.raises(TypeError, match="scratch"):
        pod.serve_gdb("127.0.0.1:0")  # type: ignore[call-arg]
    pod.close()
    with pytest.raises(ValueError, match="closed"):
        pod.serve_gdb("127.0.0.1:0", scratch=SCRATCH)


def test_the_pod_s_error_is_raised_as_any_call_s(pod):
    client = Client(["s", "D"])
    try:
        with pytest.raises(BusError):
            pod.serve_gdb("127.0.0.1:0", ready=client.ready, scratch=UNMAPPED)
    finally:
        client.join()
    assert client.replies == []
    assert pod.background is not None, "halted"
