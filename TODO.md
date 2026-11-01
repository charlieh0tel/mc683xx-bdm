# To do

## Hardware

- Schematic: carrier for the Pico / Pico 2 footprint (PLAN.md,
  Hardware).
- Breadboard first: Pico + 74AHCT125 + 74LVC125 + 2N7002s.
- Carrier layout, then a release (`jlcpcb-release` skill).

## Firmware

- `pod-fw`: stack painting and a `stack` request (PLAN.md, Firmware).
- BERR sense, if a fifth input buffer is worth a part (PLAN.md,
  Firmware).
- DSCLK above 4 MHz, once the throughput test measures the margin.
- `pod-host`: `bdm` CLI over nusb; `Cpu32Target` over it.
- m683xx-emu: depend on `cpu32-target` by git and implement it.
- Bring-up, in order (PLAN.md, Bring-up checklist):
  - PIO loopback: scope DSCLK/DSI, 17-bit framing, idle high.
  - Reset into BDM; FREEZE asserts.
  - GO and a target-side reset on the scope: IFETCH driven by the CPU
    against FREEZE falling and DSI's 74AHCT125 output turning off
    (PLAN.md, Hardware).
  - RSREG ATEMP first (`0x2588`): confirms RSREG's `0x2580` (PLAN.md).
  - NOP / not-ready handling.
  - RDREG D0; RSREG RPC.
  - SIMCR (0xFFFA00) and other SIM registers against reset values.
  - Chip-select init; external RAM read/write.
  - DUMP/FILL throughput; raise DSCLK until it fails, back off.
  - Load and run a test program from RAM.
  - Flash programming (target-resident algorithm).
  - Emulator on `cpu32-target`; smartclock harness on silicon.
