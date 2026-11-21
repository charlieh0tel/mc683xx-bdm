# To do

## Hardware

- Schematic: owner's KiCad pass on the draft (`hardware/mc683xx-pod/`).
- Carrier release (`jlcpcb-release` skill).

## Firmware

- Publishing to crates.io and PyPI (owner): once the repository is
  public, RELEASING.md's Setup (PyPI's pending trusted publisher, the
  `pypi` and `crates-io` environments), then the first release, its
  crates by hand, then their crates.io trusted publishers.
- DSCLK above 4 MHz, once the throughput test measures the margin.
- Report a watchdog reset to the host (WATCHDOG's REASON at start,
  say in Pins or Version, and in `bdm pins`): a protocol change, so
  `PROTOCOL_VERSION`, `docs/usb-protocol.md` and `mc683xx-bdm` with it.
- A `Link` adapter onto m683xx-emu's BDM serial exchange once it
  lands; then delete `mc683xx-bdm-model`.
- `bdm load` and `bdm init` over m683xx-emu's `mc683xx-debug` once it
  lands (PLAN.md, Host); tests against `bdm-stand-in`.  FRZSW is set
  at reset (SIMCR $60CF), but a program may clear it, and then the
  watchdog runs while the CPU is halted: an init script sets it
  again.
- `bdm gdb` on silicon with a real m68k gdb: registers, memory and
  disassembly work (docs/58503a/); still to try: breakpoints, continue,
  Ctrl-C, and `PodTarget::step` on a TRAP, an interrupt, user mode,
  code in ROM (PLAN.md, Host, gdb).
- `monitor init` and `monitor load` in `bdm gdb`, once `mc683xx-debug`
  is in (PLAN.md, Host, Loading and init).
- BKPT glitch on the reboots bootsel and the panic handler do not
  cover (docs/bring-up.md, step 5): on the next carrier, U1's enable
  falling before its input (a stronger or faster OE path than GP9's),
  so a power loss, replug or bare watchdog reset cannot glitch BKPT.
- Python bindings (`mc683xx-bdm-py`): `step` (and the scratch area) now that
  `PodTarget::step` is in.
- `mc683xx-bdm` integration test against `mc683xx-run --bdm` and its
  example image, once m683xx-emu's virtual cable lands.
- A second Pico as a stand-in CPU32: a target-side PIO program around
  `mc683xx-bdm-model`, driving FREEZE and watching BKPT and RESET.
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
  - USB serial number: `bdm list` and `lsusb -v` against `picotool
    info -d` on a Pico and a Pico 2 (byte order of the RP2350's chip
    ID); two pods at once, `--pod usb:SERIAL` and `bdm bootsel` on
    each.
