"""A stand-in pod for each test: `bdm-stand-in` (mc683xx-bdm-stand-in) on a Unix
socket, a model CPU32 behind it, built once per session with cargo."""

import json
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Protocol

import pytest

FIRMWARE = Path(__file__).resolve().parents[2]
# Where bdm-stand-in's model RAM starts (its default) and how big it is.
RAM_BASE = 0x100000
RAM_BYTES = 64 * 1024
# How long the stand-in may take to listen, and how often to look.
LISTEN_TIMEOUT_S = 10.0
LISTEN_POLL_S = 0.01
# Where the tests' Unix sockets go: a socket's path is at most 104 bytes
# on macOS (SUN_LEN), shorter than its per-user temporary directory's
# tmp_path.
SOCKET_ROOT = "/tmp"


class StandIn(Protocol):
    """The `stand_in` fixture: starts a stand-in with bdm-stand-in's
    options and returns its pod name."""

    def __call__(self, *options: str) -> str: ...


@pytest.fixture
def socket_dir() -> Iterator[Path]:
    """A directory, removed after the test, for Unix sockets: short
    enough for a socket's path on every Unix."""
    with tempfile.TemporaryDirectory(prefix="bdm-", dir=SOCKET_ROOT) as name:
        yield Path(name)


@pytest.fixture(scope="session")
def stand_in_binary() -> Path:
    """bdm-stand-in, built by cargo, its path from cargo's own report."""
    build = subprocess.run(
        [
            "cargo",
            "build",
            "--quiet",
            "-p",
            "mc683xx-bdm-stand-in",
            "--message-format=json-render-diagnostics",
        ],
        cwd=FIRMWARE,
        check=True,
        capture_output=True,
        text=True,
    )
    for line in build.stdout.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact" and message.get("executable"):
            if message["target"]["name"] == "bdm-stand-in":
                return Path(message["executable"])
    raise RuntimeError("cargo built no bdm-stand-in")


def launch(binary: Path, socket: Path, *options: str) -> subprocess.Popen[bytes]:
    """Starts bdm-stand-in on `socket` with `options`, once it listens;
    the caller stops it."""
    process = subprocess.Popen([binary, f"unix:{socket}", *options])
    deadline = time.monotonic() + LISTEN_TIMEOUT_S
    while not socket.exists():
        if process.poll() is not None:
            raise RuntimeError(f"bdm-stand-in exited with {process.returncode}")
        if time.monotonic() > deadline:
            process.kill()
            process.wait()
            raise RuntimeError(f"bdm-stand-in did not listen on {socket}")
        time.sleep(LISTEN_POLL_S)
    return process


@pytest.fixture
def stand_in(stand_in_binary: Path, socket_dir: Path) -> Iterator[StandIn]:
    """Starts a stand-in with bdm-stand-in's options; returns its pod
    name, `unix:PATH`.  Each is stopped when the test ends."""
    processes: list[subprocess.Popen[bytes]] = []

    def start(*options: str) -> str:
        socket = socket_dir / f"pod{len(processes)}.sock"
        processes.append(launch(stand_in_binary, socket, *options))
        return f"unix:{socket}"

    yield start
    for process in processes:
        process.terminate()
        try:
            process.wait(timeout=LISTEN_TIMEOUT_S)
        except subprocess.TimeoutExpired:
            print(f"bdm-stand-in {process.pid} did not stop; killing it", file=sys.stderr)
            process.kill()
            process.wait()
