# Status

## Works

On an MC68331 target (an HP 58503A, [58503a/](58503a/)), from a Linux
host:

- Reset into background mode, halt from running, go, and reset to run
  (`bdm reset --run`); a BKPT entry told from a BGND one (ATEMP).
- Every register read; data and address registers, RPC, SR, USP, SSP,
  VBR, SFC and DFC written.
- Memory and SIM registers read and written; 512 KB read in 5.8 s at
  3 MHz DSCLK, 10.4 s at 1 MHz.
- gdb (`bdm gdb`, gdb-multiarch): registers, memory, disassembly,
  breakpoints in RAM, continue, Ctrl-C, step (in RAM and ROM, through
  TRAP and RTE, in user mode, into an interrupt, over MOVEC to VBR),
  detach and kill.
- Flash programming: [smartclock's flash tool](https://github.com/charlieh0tel/smartclock/blob/main/tools/bdm/README.md), through the
  Python module and a routine loaded into the target's RAM, erased,
  programmed and verified two sector pairs and put them back.
- The Python module and `bdm` over one pod; `bdm hold` through a target
  reset; firmware updates with `bdm bootsel`.
- Unplugging an idle pod leaves a halted target halted and answering.
- Releases: `bdm` on crates.io, the Python module on PyPI, and a `.deb`
  (`bdm`, the udev rule, the UF2) in each GitHub Release and the APT
  repository.

## Not yet tried

A flash-programming command in `bdm` itself; the Pico 2 (RP2350)
build; DSCLK above 4 MHz (a 16.78 MHz target); a macOS or Windows host
with a pod (their wheels and `bdm` build in CI); two pods at once. The
work list is [TODO.md](../TODO.md).

## Limits

**DSCLK.** Keep it at or below about a third of the target's system
clock. The pod samples DSO 15/16 of a bit after each rise, and the CPU
may take up to 2 system clocks, plus about 40 ns of delays, to put it
out ([PLAN.md](../PLAN.md), Protocol summary). On an 8.39 MHz target
that is 3 MHz; above it the link fails, and only `bdm reset` recovers.
The pod caps DSCLK at 4 MHz whatever the target.

**Unplugging.** A pod restart (a USB replug, say) during `bdm hold` or
in the middle of a request can disturb a halted target until `bdm
reset`. Between requests it is safe ([bring-up.md](bring-up.md), step
5). The Pico's green LED is lit while it is not. The next carrier
revision fixes this ([hardware/README.md](../hardware/README.md), Next
revision).

## Hardware

The carrier is rev 0.2 (`hardware/mc683xx-pod/`, 68 x 59 mm, 4 layers);
its silkscreen reads rev 0.1 ([hardware/README.md](../hardware/README.md)).

There is no kit or store. `hardware/production/20261102-0.2/` holds the
JLCPCB order files (Gerbers and drill, BOM, placement), assembled with
the Pico, J2 (SWD) not fitted; [RELEASING.md](../RELEASING.md), Carrier
board, describes a board release. The carrier takes a Pico or a Pico 2.
