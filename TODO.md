# To do

## Hardware

- Schematic: owner's KiCad pass on the draft (`hardware/mc683xx-pod/`).
- USB PID from Raspberry Pi (PLAN.md, Open questions); then
  `pod-proto`'s VID and PID, and the udev rule
  (`packaging/udev/70-mc683xx-pod.rules`).
- Carrier release (`jlcpcb-release` skill).

## Firmware

- Release workflow for the `mc683xx-bdm` package, calling
  deb-workflows' `rust-build-deb`, once it can build a workspace below
  the repository's root (`firmware/`).
- DSCLK above 4 MHz, once the throughput test measures the margin.
- Report a watchdog reset to the host (WATCHDOG's REASON at start,
  say in Pins or Version, and in `bdm pins`): a protocol change, so
  `PROTOCOL_VERSION`, `docs/usb-protocol.md` and `pod-host` with it.
- A `Link` adapter onto m683xx-emu's BDM serial exchange once it
  lands; then delete `bdm-model`.
- `bdm load` and `bdm init` over m683xx-emu's `mc683xx-debug` once it
  lands (PLAN.md, Host); tests against `bdm-stand-in`.  FRZSW is set
  at reset (SIMCR $60CF), but a program may clear it (the 58503A's
  does), and then the watchdog runs while the CPU is halted: an init
  script sets it again.
- gdb: `PodTarget::step` by T1 trace once `step()` is in `Cpu32Target`,
  and `bdm gdb` serving m683xx-emu's adapter over `PodTarget` (PLAN.md,
  Host, gdb).
- `pod-host` integration test against `mc683xx-run --bdm` and its
  example image, once m683xx-emu's virtual cable lands.
- A second Pico as a stand-in CPU32: a target-side PIO program around
  `bdm-model`, driving FREEZE and watching BKPT and RESET.
- `mc683xx-target` from crates.io once 0.3.0 is published, in place of
  the pinned git rev.
- Bring-up, what is left (PLAN.md, Bring-up checklist; docs/bring-up.md):
  - Watchdog (docs/bring-up.md, step 1): idle a minute without a
    restart, BOOTSEL staying up past 8 s, a probe-rs halt not reset.
  - On the scope: PIO framing and idle high; IFETCH against FREEZE
    falling and U2's (74AHCT1G125) output turning off; U1 driving BKPT
    into 1 kOhm.
  - On a target: BGND and a breakpoint after GO (fresh ATEMP 1 and 0,
    DSI driven again); NOP / not-ready; external RAM; DUMP/FILL
    throughput, raising DSCLK until it fails; a test program from RAM;
    flash programming; the smartclock harness on silicon.
  - Silicon questions for m683xx-emu: PCC after a reset double bus
    fault, BKPT out of LPSTOP, the halt monitor on a double fault in
    BDM.
  - The 58503A's halt-monitor reset (RSR $10) after GO and a halt
    (docs/58503a/README.md).
  - USB serial number: `bdm list` and `lsusb -v` against `picotool
    info -d` on a Pico and a Pico 2 (byte order of the RP2350's chip
    ID); two pods at once, `--pod usb:SERIAL` and `bdm bootsel` on
    each.
