# mc683xx-bdm

A USB background debug mode (BDM) pod for Motorola/Freescale CPU32
(683xx) microcontrollers, primarily the MC68331: a Raspberry Pi Pico or
Pico 2 on a carrier board with 5 V level translators and the standard
10-pin BDM header.  PIO clocks the BDM serial link; firmware and host
are Rust.

Status: the rev 0.2 carrier (`hardware/mc683xx-pod/`, 68 x 59 mm, 4
layers; `hardware/production/20261102-0.2/`, assembled with the Pico,
J2 not fitted) passes its checks without a target: inputs, VDD sense
and a loopback through its buffers (`docs/bring-up.md`, steps 1-3).  On
an MC68331 target it resets into background mode, reads
registers and SIM registers, and reads 512 KB of flash correctly.

## Layout

| Path | What |
|---|---|
| `PLAN.md` | the design and why it is the way it is |
| `TODO.md` | the work list |
| `RELEASING.md` | cutting a firmware, `bdm` or board release |
| `docs/` | the host-pod protocol (`usb-protocol.md`), bring-up steps (`bring-up.md`); `oncore-vp/`, notes on the Motorola Oncore VP's BDM test points; `58503a/`, the HP 58503A's BDM header and reading its flash |
| `hardware/` | the KiCad carrier board; `ee_tooling/`, the shared tooling (a submodule pinned at 318c266, which built rev 0.2; not updated), whose `jlcpcb-release` builds a JLCPCB order into `production/`, each release as ordered; `fastcheck.py`, a report on the BDM link's fast nets (`uv run python fastcheck.py mc683xx-pod/mc683xx-pod.kicad_pcb`) |
| `firmware/` | the Rust workspace: firmware, host CLI, shared crates |
| `third_party/` | vendor documents and their provenance (`NOTICE`) |

## Firmware

```
cd firmware
cargo test                     # pod-proto, bdm-model, pod-host and pod-py, on the host

cd pod-fw
cargo run --release            # Pico, defmt over RTT, through an SWD probe
cargo build --release --no-default-features -F rp2040   # production
cargo build --release --no-default-features -F rp235xa \
    --target thumbv8m.main-none-eabihf                   # Pico 2

cd ../..
make lint                      # clippy on every build, before each commit
```

## Host

```
cd firmware
cargo run -p pod-host --bin bdm -- pins     # or halt, regs, dump 0xfffa00 64, ...
cargo run -p pod-host --bin bdm -- --pod unix:/path/to/socket regs   # a stand-in pod
cargo run -p pod-host --bin bdm -- list     # each pod on USB: serial number, bus-ports
cargo run -p pod-host --bin bdm -- --pod usb:E6613852833B2A2F regs    # one pod of several

cargo run -p pod-host --bin bdm-stand-in -- unix:/tmp/pod.sock &      # a pod on a model CPU32
cargo run -p pod-host --bin bdm -- --pod unix:/tmp/pod.sock reset
```

`--pod usb`, the default, is the only pod connected; with more than
one, `bdm` lists their serial numbers and stops.  `--pod usb:SERIAL`
picks one by the serial number `bdm list` shows (the Pico's unique ID,
as picotool shows it; case ignored), for `bdm bootsel` too.  `bdm list`
does not open the pods, so it needs no udev rule, and takes no `--pod`.

gdb on the target: `bdm gdb --scratch ADDRESS` listens on
127.0.0.1:1234 (`--listen HOST:PORT` for another) for one session;
then, in an m68k gdb (`gdb-multiarch`):

```
set architecture m68k:cpu32
set endian big
target remote localhost:1234
```

Without an executable, gdb does not know the CPU32 is big-endian and
shows every register and word byte-reversed.  `--scratch` names 1026 bytes of RAM, even, that the
program does not use: a step's exception table and BGND stub.
Breakpoints are BGND written into memory, so RAM only.  Detach leaves
the target running, kill leaves it halted.

Without root on Linux, the pod needs the udev rule
`packaging/udev/70-mc683xx-pod.rules`: copy it into
`/etc/udev/rules.d/`, or install the Debian package, which carries it.
It gives the pod to the user at the machine's seat (`TAG+="uaccess"`)
and to group `plugdev`, mode 0660, since uaccess does nothing for a
login over ssh: add a remote user with `sudo adduser USER plugdev`.
Replug the pod after installing the rule.  1209:0001 is pid.codes'
shared test pair, so the rule also matches anything else using it.

### Python

`firmware/pod-py` is the Python module `mc683xx_bdm`, PyO3 bindings
over `pod-host`: one open pod across calls, for scripts with loops and
polling.  uv builds it with maturin into the crate's `.venv`:

```
cd firmware/pod-py
uv sync                        # builds and installs the module
uv run pytest                  # against bdm-stand-in, no hardware
uv build --wheel               # a wheel, to pip install elsewhere
```

```python
import time
import mc683xx_bdm

with mc683xx_bdm.Pod("usb") as pod:  # or "usb:SERIAL", "unix:/tmp/pod.sock"
    print(pod.reset_with_breakpoint())      # breakpoint
    pod.write_register("PC", 0x100000)
    pod.write_memory(0x100000, bytes.fromhex("4e714afa"), size=2)
    pod.go()
    while pod.background is None:           # poll until it halts
        time.sleep(0.01)
    print(pod.registers())
    print(hex(pod.bus_read(0xFFFA00, 2)))   # SIMCR
```

The names follow the emulator's bindings (`mc683xx`, in m683xx-emu's
`crates/cpu32-py`): `halt`, `go`, `reset_with_breakpoint`, `background`,
`read_register("D0")`, `registers()`, `bus_read(address, size,
function_code=5)`, `Entry`, `BusError`, `TargetError`.  Blocks are
`read_memory(address, length, *, size=1, function_code=5)` and
`write_memory(address, data, ...)`; `pins`, `version`, `hello`,
`set_clock`, `resync`, `call`, `reset` (to run) and `transfer` are the
pod's own.  Leaving the `with` block, or `close()`, puts back the
program's SFC and DFC, as `bdm` does.  Exceptions: `TransportError`
(an `OSError`), `ProtocolError`, `TargetError`, `BusError`, and
`ValueError` for a bad name, size or function code.  Types:
`python/mc683xx_bdm/__init__.pyi`.

### Debian package

`make deb` builds `mc683xx-bdm`: `bdm`, the udev rule and the pod's
firmware (`/usr/share/mc683xx-bdm/pod-fw-rp2040.uf2`, needs picotool
to build), with
cargo-deb 3.8.0 (deb-workflows' `rust-build-deb` pin):
`firmware/target/debian/mc683xx-bdm_VERSION-1_ARCH.deb`.  Metadata:
`[package.metadata.deb]` in `firmware/pod-host/Cargo.toml`; changelog:
`packaging/debian/changelog`.  No release workflow yet:
`rust-build-deb` runs cargo at the repository's root, and the
workspace is `firmware/`.

## License

- Hardware: CERN-OHL-S-2.0 (`hardware/LICENSE`): the KiCad designs in
  `hardware/` and the release packages in `hardware/production/`.
- Everything else of the project's own, firmware and documentation:
  GPL-3.0-only (`LICENSE`).
- The documents in `third_party/` are their owners' and not covered
  (`third_party/NOTICE`).

## Contributing

Once per clone: `git submodule update --init` (ee_tooling, which the
commit hook runs from), then `make hooks`.  For the hardware tools,
`cd hardware && uv venv --system-site-packages --python /usr/bin/python3
&& uv sync`.  `AGENTS.md` has the rules.
