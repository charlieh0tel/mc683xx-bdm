# To do

## Sources

- CPU32RM/AD and the MC68331 User's Manual into `third_party/`, with
  `third_party/NOTICE` (provenance, SHA-256).
- From CPU32RM: DSCLK maximum against CLKOUT, DSI setup/hold, the
  DSCLK edge on which DSO changes, and the opcode table (PLAN.md,
  Protocol summary).
- 74AHCT125 datasheet: inputs at 3.3 V with VCC = 0 (PLAN.md,
  Hardware).

## Hardware

- Schematic: carrier for the Pico / Pico 2 footprint (PLAN.md,
  Hardware).
- Breadboard first: Pico + 74AHCT125 + 74LVC125 + 2N7002s.
- Carrier layout, then a release (`jlcpcb-release` skill).

## Firmware

- Workspace in `firmware/`: `cpu32-target`, `pod-proto`, `pod-fw`
  (features `rp2040`, `rp235xa`), `pod-host` (PLAN.md, Repo layout).
- `make lint` and CI then run clippy on them.
- Bring-up, in order (PLAN.md, Bring-up checklist):
  - PIO loopback: scope DSCLK/DSI, 17-bit framing, idle high.
  - Reset into BDM; FREEZE asserts.
  - NOP / not-ready handling.
  - RREG D0; RSREG PC.
  - SIMCR (0xFFFA00) and other SIM registers against reset values.
  - Chip-select init; external RAM read/write.
  - DUMP/FILL throughput; raise DSCLK until it fails, back off.
  - Load and run a test program from RAM.
  - Flash programming (target-resident algorithm).
  - Emulator on `cpu32-target`; smartclock harness on silicon.
