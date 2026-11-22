# HP 58503A

What the pod did on one HP 58503A: an MC68331, an 8.39 MHz system
clock, its BDM header J12.  The unit's own facts are in
[smartclock](https://github.com/charlieh0tel/smartclock).

## Board facts

In smartclock's
[58503a-bdm.md](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md)
unless noted:

- [J12](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md#j12):
  its pinout, buzzed out.  The pod's cable plugs straight in.
- [The clock](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md#the-clock):
  crystal, system clock, SYNCR.
- [At a reset entry](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md#at-a-reset-entry):
  the SIM registers, SYPCR and SIMCR read there.
- [Halting the running firmware](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md#halting-the-running-firmware):
  the SIMCR freeze values ($014A, $614A, $414A).
- [With background mode off](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md#with-background-mode-off):
  what a halt does then.
- [Single steps](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md#single-steps)
  on silicon.
- [The chip selects](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md#the-chip-selects),
  and the order that works.
- [Reading the flash](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md#reading-the-flash):
  the read matches `58503a-3704.bin`.
- [Programming the flash](https://github.com/charlieh0tel/smartclock/blob/main/docs/58503a-bdm.md#programming-the-flash):
  stages A-C, by hand.
- [hardware-investigations.md](https://github.com/charlieh0tel/smartclock/blob/main/docs/hardware-investigations.md):
  no outer oven, the LEDs, the Alarm jack, /IRQ2, PGP5, the watchdog
  reset records, S1.
- [58503a-z3801a-trace.md](https://github.com/charlieh0tel/smartclock/blob/main/docs/firmware/58503a-z3801a-trace.md),
  Register port: the FPGA's read map.

## Commands used

- `bdm reset`: a reset entry, before the first instruction.
- `bdm dump 0 0x80000 --size long --output FILE`: the 512 KB flash,
  with CSBOOT and CS1 set as the firmware sets them.
- `bdm reset --run`: restarts the firmware without a power cycle, with
  background mode off.  `bdm go` straight after `bdm reset` also
  starts it from the reset vector.
- `bdm halt`: halts the running firmware.

## Pod restarts

A pod restarted while the CPU is halted can glitch BKPT
([bring-up.md](../bring-up.md), step 5).  Here that left the CPU
refusing RSREG ATEMP (illegal, then $0001) until `bdm reset`; nine
seconds of idle halt without a restart did not.  A restart by `bdm
bootsel`, which turns U1 off first, left it answering (2026-11-20).

With U1 off between requests (pod 0.3.0), a replug of the pod's USB
left the CPU answering, with no `bdm reset` or `bdm resync` first:
halted by `bdm reset`, and halted by `bdm halt` from running (SIMCR
$614A, so its watchdog stays frozen).  A replug during `bdm hold` left
it refusing commands (illegal command, then unexpected responses);
`bdm resync` did not clear that, `bdm reset` did (2026-11-21).

With `bdm hold` (BKPT held low, nothing clocked) after the firmware
had run, its watchdog reset the CPU into background mode at the reset
vector (RSR $20, the watchdog alone); after GO the firmware ran on
(2026-11-21).

## Register writes

D0, D7, A0, A6, USP and VBR written with $12345678 read back the same;
D0 with $DEADBEEF too.  SFC 5 and DFC 3 read back; $12345678 reads back
0 in SFC and DFC, which hold 3 bits (pod 0.3.0, 2026-11-21).

## gdb

`bdm gdb` from a reset entry, gdb-multiarch with `set architecture
m68k:cpu32` and `set endian big`: registers, memory and disassembly of
the boot code at $550 work; `detach` lets the firmware boot.  Scratch,
by smartclock's RAM map: $106000 while the primary runs (VBR $10000);
all of RAM with the firmware stopped; not $106000 under the installer.

Breakpoints and steps in RAM, pod 0.3.0 (protocol 11), 2026-11-21:
`bdm gdb --scratch 0x108000` from a reset entry, the RAM chip selects
(CS0, CS2, CS3) set as the firmware sets them, the watchdog off, and a
loop at $100000: `addq.l #1,d0; addq.l #1,d1; bra.s`.

- A breakpoint at $100004 hit twice with `continue`, D0 and D1
  counting.
- `stepi` stepped over the `bra.s` and off a breakpoint.
- A second breakpoint at $100002 hit; `delete` then `kill` left the
  target halted, the loop's bytes intact.
- Ctrl-C in gdb during `continue` halted the target ("Program received
  signal SIGINT"); a `stepi` then worked; `detach` left it running.

Steps in ROM, through TRAP and RTE, and in user mode, pod at main
4009549 and later, 2026-11-21, set up as above without the loop:

- `stepi` in ROM at $550 over `move.w #$2700,sr` and `movea.l #0,a0`
  worked.
- `stepi` onto TRAP #0, VBR $101000, vector 32 to $100100: stopped at
  the handler's first instruction, SSP down 8.
- `stepi` over the handler's MOVEQ and RTE: back at the instruction
  after the TRAP, SSP restored.
- User mode (SR $0000, USP $10F000): steps kept SR user, SP USP.
- TRAP #0 stepped from user mode: stopped at the handler, S set, on
  SSP.
- `stepi` on the boot code's `movec a0,vbr` (4E7B 8801), before the
  host emulated MOVEC to VBR: "target: step: the step did not stop;
  halted at 0x001005b8", the session ended, and VBR was back at
  $101000, the program's write lost.  With the host's MOVEC to VBR
  emulation (PLAN.md, Host, gdb): `stepi` from $550 over MOVE to SR,
  MOVEA, `movec a0,vbr`, `movea.l (a0),sp` and the SYPCR write, VBR
  $00000000 after, SSP $10FFFE from the vector table.

On the running primary, halted at $133EE, scratch $106000, SIMCR $414A
for the session (2026-11-21): the first `stepi` took a pending level-6
interrupt and stopped at its handler's entry ($12FB2, SR $2600, SP
$10FCF8); the next two stepped inside it; VBR was back at $10000 and
`detach` let the firmware run on.  With SIMCR $614A the second step
hung: FRZBM stops the bus monitor, so a stray access never ends.  Use
$414A (FRZSW only) for gdb.  Since the pod's BERR abort, such a hang
ends in a bus error: with SIMCR $614A, a read of $110000 (unmapped)
replied "target bus error" in 43 ms and the CPU took commands after.

## Throughput

The 512 KB flash, `bdm dump 0 0x80000 --size long --output FILE`,
each read the same as the others (2026-11-21):

| Pod | 1 MHz | 2 MHz | 3 MHz |
|---|---|---|---|
| 65f92ba | 19.0 s | | |
| 0.3.0 (U1 off between requests) | 24.0 s | 20.8 s | |
| a short word's answer spun for (PLAN.md, Firmware) | 12.7 s | 9.0 s | 7.9 s |
| and 4 read requests in flight (PLAN.md, USB) | 10.4 s | 7.0 s | 5.8 s |

With the spin, `--size word` at 1 MHz takes 16.3 s; with requests in
flight 13.9 s.  At 3.3 MHz a short read works and a 512 KB one failed.
The slowdown from 65f92ba to 0.3.0 came from added work on each word's
path, mostly FREEZE before every word (20.6 s to 25.0 s); an await for
each word's answer cost tens of microseconds.  The wire's share at
1 MHz is about 5.6 s.

## DSCLK

The 512 KB flash (`bdm dump 0 0x80000 --size long`) read the same as at
1 MHz at 2, 2.5 and 3 MHz (Throughput).  At 3.5 and 4 MHz it failed
("unexpected response from target"), and only `bdm reset` recovered
(`bdm resync` did not) (pod 0.3.0, protocol 11, 2026-11-21).  At the
8.39 MHz system clock MC68331UM Table A-7 allows DSCLK up to 4.19 MHz;
3.5 MHz is about 83% of that.

## smartclock's flash tool

The tool and its own record of this trial: [tools/bdm](https://github.com/charlieh0tel/smartclock/blob/main/tools/bdm/README.md#a-bench-trial).
Through the Python module, one `Pod` open per run, the tool writes a
position-independent routine, a vector table and each sector pair's
data into RAM, GOes, and polls `background` until the routine's BGND
(`Entry.Background`, RPC its address plus 2); the routine erases,
programs and verifies at bus speed.  Five runs at 1 MHz DSCLK
(2026-11-21, pod at main a6330e9): a dry run (512 KB read, 10.4 s with
the setup); identify; sector pairs $70000 and $78000 erased,
programmed, verified and the whole flash hashed, 25.4 s; both
restored; then `Pod.reset` booted the firmware.  Closing the `Pod`
left the CPU halted between runs, and the pod reported no error.

Programming by single `bdm` commands costs about 7 ms a command (a
process, USB open and Hello each), so a whole image that way would
take most of an hour.
