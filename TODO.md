# To do

## Hardware

- Schematic: owner's KiCad pass on the draft (`hardware/m683xx-pod/`).
- Layout: the Pico's USB end and J2 (SWD) at the carrier's edge.
- USB PID from Raspberry Pi (PLAN.md, Open questions); then
  `pod-proto`'s VID and PID, and the README's udev rule.
- Breadboard first: Pico + 74AHCT125 + 74LVC125 + 2N7002s.
- Carrier layout, then a release (`jlcpcb-release` skill).

## Firmware

- `pod-fw`: stack painting and a `stack` request (PLAN.md, Firmware).
- BERR sense, if a fifth input buffer is worth a part (PLAN.md,
  Firmware).
- DSCLK above 4 MHz, once the throughput test measures the margin.
- A `Link` adapter onto m683xx-emu's BDM serial exchange once it
  lands; then delete `bdm-model`.
- `bdm load <srec/elf>`; a target init script run after each reset.
  It sets SIMCR FRZSW or disables the watchdog: with FRZSW 0, the
  68331's reset value, the watchdog runs while the CPU is halted.
- `pod-host` integration test against `mc683xx-run --bdm` and its
  example image, once m683xx-emu's virtual cable lands.
- A second Pico as a stand-in CPU32: a target-side PIO program around
  `bdm-model`, driving FREEZE and watching BKPT and RESET.
- `mc683xx-target` from crates.io once 0.3.0 is published, in place of
  the pinned git rev.
- Bring-up, in order (PLAN.md, Bring-up checklist):
  - PIO loopback: scope DSCLK/DSI, 17-bit framing, idle high.
  - Reset into BDM; FREEZE asserts.
  - GO and a target-side reset on the scope: IFETCH driven by the CPU
    against FREEZE falling and DSI's 74AHCT125 output turning off
    (PLAN.md, Hardware).
  - RSREG ATEMP first (`0x2588`): confirms RSREG's `0x2580` (PLAN.md).
  - BGND and a breakpoint after GO: `bdm pins` gives a fresh ATEMP of 1
    and 0, and commands work (DSI driven again).
  - NOP / not-ready handling.
  - RDREG D0; RSREG RPC.
  - SIMCR (0xFFFA00) and other SIM registers against reset values.
  - Chip-select init; external RAM read/write.
  - DUMP/FILL throughput; raise DSCLK until it fails, back off.
  - Load and run a test program from RAM.
  - Flash programming (target-resident algorithm).
  - Emulator on `mc683xx-target`; smartclock harness on silicon.
