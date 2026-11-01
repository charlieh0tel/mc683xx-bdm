# m683xx-pod

A USB background debug mode (BDM) pod for Motorola/Freescale CPU32
(683xx) microcontrollers, primarily the MC68331: a Raspberry Pi Pico or
Pico 2 on a carrier board with 5 V level translators and the standard
10-pin BDM header.  PIO clocks the BDM serial link; firmware and host
are Rust.

Status: firmware written, not yet run on a Pico; no schematic or host
CLI yet.

## Layout

| Path | What |
|---|---|
| `PLAN.md` | the design and why it is the way it is |
| `TODO.md` | the work list |
| `docs/` | the host-pod protocol (`usb-protocol.md`) |
| `hardware/` | the KiCad carrier board, and `release.py`, which builds a JLCPCB order |
| `production/` | each release as ordered |
| `firmware/` | the Rust workspace: firmware, host CLI, shared crates |
| `third_party/` | vendor documents and their provenance (`NOTICE`) |

## Firmware

```
cd firmware
cargo test                     # cpu32-target, pod-proto and bdm-model, on the host

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
```

Without root on Linux, the pod needs a udev rule, e.g.
`/etc/udev/rules.d/70-m683xx-pod.rules`:

```
SUBSYSTEM=="usb", ATTRS{idVendor}=="1209", ATTRS{idProduct}=="0001", TAG+="uaccess"
```

1209:0001 is pid.codes' shared test pair, so the rule also matches
anything else using it.

## License

GPL-3.0-only (`LICENSE`), the project's own work: firmware,
hardware design and documentation.  The documents in `third_party/` are
their owners' and not covered (`third_party/NOTICE`).

## Contributing

`make hooks` once per clone; `AGENTS.md` has the rules.
