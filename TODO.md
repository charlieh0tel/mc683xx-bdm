# To do

## Hardware

- Schematic: owner's KiCad pass on the draft (`hardware/mc683xx-pod/`).
- Carrier release (`jlcpcb-release` skill).

## Firmware

- DSCLK above 4 MHz, on a 16.78 MHz target, once the throughput test
  measures the margin there (docs/58503a/: an 8.39 MHz target fails at
  3.5 MHz).
- Throughput, parked (512 KB of longs on an 8.39 MHz target: 10.4 s
  at 1 MHz, 5.8 s at 3 MHz; about 0.4 ms a request, the rest the wire;
  PLAN.md, Bring-up checklist):
  - A ReadMemory answered with several IN packets (a protocol change):
    most of the 0.4 ms a request, about 3 s of a 512 KB read at any
    rate.
  - The next DUMP in place of a result's closing NOP: a third of a
    long's words, half a word's; care after a bus error or not-ready,
    where the CPU has taken that DUMP as a command.
  - DSO sampled just after the next rise instead of before it: DSCLK
    up to half the system clock (3 to about 4 MHz at 8.39 MHz), if the
    CPU holds DSO long enough; on a scope first.
- Report a watchdog reset to the host (WATCHDOG's REASON at start,
  say in Pins or Version, and in `bdm pins`): a protocol change, so
  `PROTOCOL_VERSION`, `docs/usb-protocol.md` and `mc683xx-bdm` with it.
- `bdm load` and `bdm init` over m683xx-emu's `mc683xx-debug` once it
  lands (PLAN.md, Host); tests against `bdm-stand-in`.  FRZSW is set
  at reset (SIMCR $60CF), but a program may clear it, and then the
  watchdog runs while the CPU is halted: an init script sets it
  again.
- `bdm gdb` on silicon with a real m68k gdb: registers, memory,
  disassembly, breakpoints, continue, Ctrl-C, a step over a branch,
  detach and kill work (docs/58503a/); still to try: `PodTarget::step`
  on a TRAP, an interrupt, user mode, code in ROM (PLAN.md, Host, gdb).
- `monitor init` and `monitor load` in `bdm gdb`, once `mc683xx-debug`
  is in (PLAN.md, Host, Loading and init).
- The next carrier revision's changes: `hardware/README.md`, Next
  revision.
- Python bindings (`mc683xx-bdm-py`): `step` (and the scratch area) now that
  `PodTarget::step` is in.
- A second Pico as a stand-in CPU32: a target-side PIO program around
  `mc683xx-bdm-model`, driving FREEZE and watching BKPT and RESET.
- Bring-up, what is left (PLAN.md, Bring-up checklist; docs/bring-up.md):
  - Watchdog (docs/bring-up.md, step 1): idle a minute without a
    restart, BOOTSEL staying up past 8 s, a probe-rs halt not reset.
  - On the scope: PIO framing and idle high; IFETCH against FREEZE
    falling and U2's (74AHCT1G125) output turning off; U1 driving BKPT
    into 1 kOhm.
  - On a target: BGND and a breakpoint after GO (fresh ATEMP 1 and 0,
    DSI driven again); NOP / not-ready; external RAM; FILL throughput,
    and DSCLK's limit on a 16.78 MHz target; a test program from RAM;
    flash programming; the smartclock harness on silicon.
  - Silicon questions for m683xx-emu: PCC after a reset double bus
    fault, BKPT out of LPSTOP, the halt monitor on a double fault in
    BDM.
  - USB serial number: `bdm list` and `lsusb -v` against `picotool
    info -d` on a Pico and a Pico 2 (byte order of the RP2350's chip
    ID); two pods at once, `--pod usb:SERIAL` and `bdm bootsel` on
    each.
