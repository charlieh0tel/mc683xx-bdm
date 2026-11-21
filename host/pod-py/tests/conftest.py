"""A stand-in pod for each test: `bdm-stand-in` (pod-host) on a Unix
socket, a model CPU32 behind it, built once per session with cargo."""

import json
import subprocess
import sys
import time
from collections.abc import Callable, Iterator
from pathlib import Path

import pytest

FIRMWARE = Path(__file__).resolve().parents[2]
# Where bdm-stand-in's model RAM starts (its default) and how big it is.
RAM_BASE = 0x100000
RAM_BYTES = 64 * 1024
# How long the stand-in may take to listen, and how often to look.
LISTEN_TIMEOUT_S = 10.0
LISTEN_POLL_S = 0.01


@pytest.fixture(scope="session")
def stand_in_binary() -> Path:
    """bdm-stand-in, built by cargo, its path from cargo's own report."""
    build = subprocess.run(
        [
            "cargo",
            "build",
            "--quiet",
            "-p",
            "pod-host",
            "--bin",
            "bdm-stand-in",
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


@pytest.fixture
def stand_in(stand_in_binary: Path, tmp_path: Path) -> Iterator[Callable[..., str]]:
    """Starts a stand-in with bdm-stand-in's options; returns its pod
    name, `unix:PATH`.  Each is stopped when the test ends."""
    processes: list[subprocess.Popen[bytes]] = []

    def start(*options: str) -> str:
        socket = tmp_path / f"pod{len(processes)}.sock"
        process = subprocess.Popen([stand_in_binary, f"unix:{socket}", *options])
        processes.append(process)
        deadline = time.monotonic() + LISTEN_TIMEOUT_S
        while not socket.exists():
            if process.poll() is not None:
                raise RuntimeError(f"bdm-stand-in exited with {process.returncode}")
            if time.monotonic() > deadline:
                raise RuntimeError(f"bdm-stand-in did not listen on {socket}")
            time.sleep(LISTEN_POLL_S)
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
