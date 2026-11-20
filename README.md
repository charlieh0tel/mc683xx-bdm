# mc683xx-pod

A USB background debug mode (BDM) pod for Motorola/Freescale CPU32
(683xx) microcontrollers, primarily the MC68331: a Raspberry Pi Pico or
Pico 2 on a carrier board with 5 V level translators and the standard
10-pin BDM header.  PIO clocks the BDM serial link; firmware and host
are Rust.

Status: the rev 0.2 carrier (`hardware/mc683xx-pod/`, 68 x 59 mm, 4
layers; `hardware/production/20261102-0.2/`, assembled with the Pico,
J2 not fitted) passes its checks without a target: inputs, VDD sense
and a loopback through its buffers (`docs/bring-up.md`, steps 1-3).
The first target is a Motorola Oncore VP (`docs/oncore-vp/`).

## Layout

| Path | What |
|---|---|
| `PLAN.md` | the design and why it is the way it is |
| `TODO.md` | the work list |
| `docs/` | the host-pod protocol (`usb-protocol.md`), bring-up steps (`bring-up.md`), the Oncore VP target (`oncore-vp/`) |
| `hardware/` | the KiCad carrier board; `ee_tooling/`, the shared tooling (a submodule following its `main`), whose `jlcpcb-release` builds a JLCPCB order into `production/`, each release as ordered; `fastcheck.py`, a report on the BDM link's fast nets (`uv run python fastcheck.py mc683xx-pod/mc683xx-pod.kicad_pcb`) |
| `firmware/` | the Rust workspace: firmware, host CLI, shared crates |
| `third_party/` | vendor documents and their provenance (`NOTICE`) |

## Firmware

```
cd firmware
cargo test                     # pod-proto, bdm-model and pod-host, on the host

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

cargo run -p pod-host --bin bdm-stand-in -- unix:/tmp/pod.sock &      # a pod on a model CPU32
cargo run -p pod-host --bin bdm -- --pod unix:/tmp/pod.sock reset
```

Without root on Linux, the pod needs a udev rule, e.g.
`/etc/udev/rules.d/70-mc683xx-pod.rules`:

```
SUBSYSTEM=="usb", ATTRS{idVendor}=="1209", ATTRS{idProduct}=="0001", TAG+="uaccess"
```

1209:0001 is pid.codes' shared test pair, so the rule also matches
anything else using it.

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
