# To do

## Hardware

- Schematic: owner's KiCad pass on the draft (`hardware/mc683xx-pod/`).
- USB PID from Raspberry Pi (PLAN.md, Open questions); then
  `pod-proto`'s VID and PID, and the README's udev rule.
- Breadboard first: Pico + 74AHCT1G125s + 74LVC1G17s + BSS138BKs.
- Carrier release (`jlcpcb-release` skill).

## Firmware

- BERR sense: read GP6 (U8) and report it in Pins (PLAN.md, Firmware;
  docs/usb-protocol.md).
- DSCLK above 4 MHz, once the throughput test measures the margin.
- A `Link` adapter onto m683xx-emu's BDM serial exchange once it
  lands; then delete `bdm-model`.
- `bdm load` and `bdm init` over m683xx-emu's `mc683xx-debug` once it
  lands (PLAN.md, Host); tests against `bdm-stand-in`.  A board's init
  script sets SIMCR FRZSW or disables the watchdog: with FRZSW 0, the
  68331's reset value, the watchdog runs while the CPU is halted.
- gdb: `PodTarget::step` by T1 trace once `step()` is in `Cpu32Target`,
  and `bdm gdb` serving m683xx-emu's adapter over `PodTarget` (PLAN.md,
  Host, gdb).
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
    against FREEZE falling and U2's (74AHCT1G125) output turning off
    (PLAN.md, Hardware).
  - RSREG ATEMP first (`0x2588`): confirms RSREG's `0x2580` (PLAN.md).
  - BGND and a breakpoint after GO: `bdm pins` gives a fresh ATEMP of 1
    and 0, and commands work (DSI driven again).
  - Silicon questions for m683xx-emu (PLAN.md, Bring-up checklist): PCC
    after a reset double bus fault, BKPT out of LPSTOP, the halt
    monitor on a double fault in BDM.
  - NOP / not-ready handling.
  - RDREG D0; RSREG RPC.
  - SIMCR (0xFFFA00) and other SIM registers against reset values.
  - Chip-select init; external RAM read/write.
  - DUMP/FILL throughput; raise DSCLK until it fails, back off.
  - Load and run a test program from RAM.
  - Flash programming (target-resident algorithm).
  - Emulator on `mc683xx-target`; smartclock harness on silicon.
