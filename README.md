# m683xx-pod

A USB background debug mode (BDM) pod for Motorola/Freescale CPU32
(683xx) microcontrollers, primarily the MC68331: a Raspberry Pi Pico or
Pico 2 on a carrier board with 5 V level translators and the standard
10-pin BDM header.  PIO clocks the BDM serial link; firmware and host
are Rust.

Status: design.  No schematic, firmware or host code yet.

## Layout

| Path | What |
|---|---|
| `PLAN.md` | the design and why it is the way it is |
| `TODO.md` | the work list |
| `hardware/` | the KiCad carrier board, and `release.py`, which builds a JLCPCB order |
| `production/` | each release as ordered |
| `firmware/` | the Rust workspace: firmware, host CLI, shared crates |
| `third_party/` | vendor documents and their provenance (`NOTICE`) |

## License

GPL-3.0-only (`LICENSE`), the project's own work: firmware,
hardware design and documentation.  The documents in `third_party/` are
their owners' and not covered (`third_party/NOTICE`).

## Contributing

`make hooks` once per clone; `AGENTS.md` has the rules.
