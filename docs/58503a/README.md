# HP 58503A

A GPS time and frequency reference built around an MC68331 (U33),
on board 58503-60001 Rev C.  smartclock holds the board's facts
(`board-atlas.json`) and its firmware; this page is its BDM access.

## BDM header, J12

J12, the 2 x 5, 0.100 in header beside U33, is the standard CPU32 BDM
header (MC68331UM 5.10.2.8, Figure 5-11).  The pod's cable plugs
straight in.  Buzzed out, power off:

| J12 pin | Signal | U33 pin |
|---|---|---|
| 1 | DS | 85 |
| 2 | BERR | 70 |
| 3, 5 | GND | VSS |
| 4 | BKPT/DSCLK | 56 |
| 6 | FREEZE | 58 |
| 7 | RESET | 68 |
| 8 | IFETCH/DSI | 55 |
| 9 | VDD, +5 V | VDD |
| 10 | IPIPE/DSO | 54 |

## Clock

SYNCR reads $3F08 after reset: the synthesizer from a 32.768 kHz
reference, 8.39 MHz.  DSCLK works up to 3 MHz (DSCLK).

## After a reset entry

Read over BDM right after `bdm reset`, before the first instruction:

| Register | Value |
|---|---|
| ATEMP | $00000000, breakpoint |
| RPC | $00000550, the reset vector |
| SSP | $0010FFFE |
| SR | $2714 |
| PCC | $FFFFFFFF |
| SIMCR | $60CF: FRZSW set, so the software watchdog is stopped while halted |
| SYNCR | $3F08 |
| CSBARBT, CSORBT | $0007, $7B70 |

## Reading the flash

The flash is two banks of 256 KB: CSBOOT at 0x000000 and CS1 at
0x040000 (smartclock `docs/firmware/restart.md`, chip-select table).
After a reset only CSBOOT is set, as a 1 MB block, so 0x040000 reads
the lower bank again.  Set CSBOOT and CS1 as the firmware does, then
read both:

```
bdm clock 1000000
bdm reset
bdm write 0xfffa48 0x00 0x05 --size word   # CSBARBT: 0, 256 KB
bdm write 0xfffa50 0x04 0x05 --size word   # CSBAR1: 0x40000, 256 KB
bdm write 0xfffa52 0x68 0x70 --size word   # CSOR1: both bytes, read, 1 wait
bdm dump 0 0x80000 --size long --output 58503a.bin
```

The bench unit's flash, read this way, is identical to smartclock's
`third_party/firmware/images/58503a-3704.bin`.  512 KB takes about
19 s at 1 MHz.

## Back to running

`bdm reset --run` restarts the firmware without a power cycle, with
background mode off.  Halting it afterwards still answers "halted: breakpoint",
but with RPC at the reset vector and RSR $10: with background mode off,
BKPT is a breakpoint exception, the firmware does not survive it, the
halt monitor resets the CPU, and BKPT, still held low, enables
background mode at that reset.  `bdm go` straight after `bdm reset` also starts
it from the reset vector.

## The firmware's SIM settings

The boot code at $550 (read over BDM with gdb) writes SYPCR $CC, which
turns the halt monitor on, and the chip selects in the table above.
The running firmware writes SIMCR $014A, clearing FRZSW and FRZBM, so
its software watchdog (about 8 s) keeps counting while the CPU is
halted (smartclock's reading of the 3704 image).  At a reset entry
SIMCR is still $60CF.  So: halt from a reset entry, or keep a halt
after the firmware has run under about 5 s, or set FRZSW first.

Read over BDM from a halt of the running firmware (2026-11-21, step R0
of smartclock's checklist; halt under 1 s, then GO): ATEMP $0000
(breakpoint), SIMCR $014A, SYPCR $CC, PICR $0542, PITR $0008, SYNCR
$CF88, RPC $000120FC, PCC $000120F8, RSR $80 (the last reset was the
pod's), the firmware's watchdog countdown at $100B90 7.

## S1, the switch byte at $302000

S1 (8 positions) reads at $302000 (CS5, CSBAR5 $3003, CSOR5 $7B30):
position n is bit n-1, ON 0 and OFF 1; all ON reads $00.  Turning one
position OFF at a time set exactly its bit, read twice, with the
firmware running between halts (2026-11-21, step R1 of smartclock's
checklist).  No position changed the flash longs at $3FFFC ($5E4A146A),
$7FFFC ($BCAE0C74) or $10000 ($0010FFFE): S1 does not gate flash
reads.

## The firmware's reset log

Read from a halt after the pod's reset (2026-11-21, step R2 of
smartclock's checklist): RSR $80, warm_start ($1039AF) 0, the preserved
region's checksum and restore ($100000) $DE21 and 1,
nv_settings_defaulted ($102276) 0, crash_message ($100B2E) empty.  The
EEPROM's reset records at $400200 (32 of $5C bytes) hold "Log cleared"
(2007) and four "Exception: ROOT|scpi, vector 2030, PC = ..." records
dated 2026-11-20.  $2030 is a format $2 frame at vector offset $030,
vector 12, the hardware breakpoint: BKPT taken as an exception, as a
halt with background mode off is (Back to running, above).

## Long halts

Writing SIMCR $614A (FRZSW and FRZBM) in a halt of the running firmware
stops its watchdog and PIT while the CPU is halted: a 15 s halt ended
without a reset (RSR still $80, the firmware's countdown unchanged),
and the firmware ran on after SIMCR $014A and GO (2026-11-21, step W0
of smartclock's checklist).  With FRZBM set an access nothing answers
hangs instead of taking a bus error: no unmapped addresses then.

## A watchdog reset

A halt of the running firmware without the freeze above ended in a
watchdog reset about 8 s in (SYPCR $CC: 2^18 / 32.768 kHz), and the
firmware restarted by itself (2026-11-21, step W5 of smartclock's
checklist).  BKPT was high at that reset, so background mode is off
afterwards: a halt then is a breakpoint exception.  RSR and warm_start
after it are read through the firmware's console, not BDM.  Read that
way: warm_start ($1039AF) 1, the preserved region's restore 1, and the
unit's log "Re-boot" at the reset (every reset by the pod logs "Power
on"): a watchdog reset is a warm start, an external one cold.  The
breakpoint-exception restarts in the reset log are logged "Re-boot"
too.  A 15 s halt with the watchdog frozen still logged "Holdover
started, invalid GPS 1PPS": the loop sees a long halt.

With `bdm hold` (BKPT held low, nothing clocked) after the firmware had
run, its watchdog reset the CPU into background mode at the reset
vector: ATEMP $0000, RSR $20 (the watchdog alone), SIMCR $60CF, SYPCR
$80, RPC $550, VBR 0.  After GO the firmware read warm_start 1
(2026-11-21).

S1 at reset (2026-11-21, step R12 of smartclock's checklist): with all
eight positions OFF as RESET rose, and again at a power-up into
background mode (`bdm hold` held BKPT low through the power cycle),
the reset-state SIM registers (SIMCR $60CF, SYPCR $80, CSPAR $3FFF and
$03FF, CSBARBT $0007, CSORBT $7B70), RPC $550, the vectors and the
512 KB flash contents read the same as with all ON.  S1 shows only at
$302000, which needs CS5 (CSPAR0 $2BFF, CSBAR5 $3003, CSOR5 $7B30):
$FF all OFF, $00 all ON.  The power-up entry read RSR $40, power-up
alone.

## Pod restarts

A pod restarted while the CPU is halted can glitch BKPT
(docs/bring-up.md, step 5).  Here that left the CPU refusing RSREG
ATEMP (illegal, then $0001) until `bdm reset`; nine seconds of idle
halt without a restart did not.  A restart by `bdm bootsel`, which
turns U1 off first, left it answering (2026-11-20).

With U1 off between requests (pod 0.3.0), a replug of the pod's USB
left the CPU answering, with no `bdm reset` or `bdm resync` first:
halted by `bdm reset`, and halted by `bdm halt` from running (SIMCR
$614A, so its watchdog stays frozen).  A replug during `bdm hold`
left it refusing commands (illegal command, then unexpected
responses); `bdm resync` did not clear that, `bdm reset` did
(2026-11-21).

## Register writes

D0, D7, A0, A6, USP and VBR written with $12345678 read back the same;
D0 with $DEADBEEF too.  SFC 5 and DFC 3 read back; $12345678 reads back
0 in SFC and DFC, which hold 3 bits (pod 0.3.0, 2026-11-21).

## gdb

`bdm gdb` from a reset entry, gdb-multiarch with `set architecture
m68k:cpu32` and `set endian big`: registers, memory and disassembly of
the boot code at $550 work; `detach` lets the firmware boot.
smartclock's RAM map: $106000 while the primary runs (VBR $10000); all
of RAM with the firmware stopped; not $106000 under the installer.

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
4009549 and later, 2026-11-21: `bdm gdb --scratch 0x108000` from a
reset entry, the RAM chip selects set as the firmware sets them, the
watchdog off.

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

Single-step on the running primary, halted at $133EE, scratch $106000,
SIMCR $414A for the session (2026-11-21): the first `stepi` took a
pending level-6 interrupt and stopped at its handler's entry ($12FB2,
SR $2600, SP $10FCF8); the next two stepped inside it; VBR was back at
$10000 and `detach` let the firmware run on.  With SIMCR $614A the
second step hung: FRZBM stops the bus monitor, so a stray access
(likely past the top of RAM, $10FFFF) never ends.  Use $414A (FRZSW
only) for gdb.  Since the pod's BERR abort, such a hang ends in a bus
error instead: with SIMCR $614A, a read of $110000 (unmapped) replied
"target bus error" in 43 ms and the CPU took commands after.

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
flight 13.9 s.  At 3.3 MHz a short read works and a 512 KB one failed.  The slowdown from
65f92ba to 0.3.0 came from added work on each word's path, mostly with
FREEZE before every word (20.6 s to 25.0 s), while an await for each
word's answer cost tens of microseconds; the wire's share at 1 MHz is
about 5.6 s.

## DSCLK

The 512 KB flash (`bdm dump 0 0x80000 --size long`) read the same as at
1 MHz at 2, 2.5 and 3 MHz (Throughput).  At 3.5 and 4 MHz it failed ("unexpected response from
target"), and only `bdm reset` recovered (`bdm resync` did not) (pod
0.3.0, protocol 11, 2026-11-21).  The system clock is 8.39 MHz (Clock),
so MC68331UM Table A-7 allows DSCLK up to 4.19 MHz; 3.5 MHz is about
83% of that.

## The six main-board LEDs

The DUART's OP2-OP7 drive the six LEDs: a pin taken high (ROPBC,
$20001F) lights its LED, low (SOPBC, $20001D) turns it off.  Counted
from the right: OP2 the rightmost, green; OP3, OP4, OP5, OP6 and OP7
(the leftmost) red (2026-11-21, step W1 of smartclock's checklist).
Reads of $20001D and $20001F start and stop the DUART's counter: write
them only.

## The rear Alarm output

The rear Alarm jack follows PF5 (PORTF0 bit 5, NOT led_alarm): 5 V with
PF5 high (no alarm), 0.6 mV with it low, measured at the jack (U33 pin
73 not measured; 2026-11-21, step W3 of smartclock's checklist).  The
firmware writes PF5 only when its alarm state changes, so a test that
flips it puts it back.

## /IRQ2

Made a discrete input for a moment in a halt (DDRF $F3, then PFPAR $00;
back with PFPAR $04, then DDRF $F7, so the pin is never driven), PF2,
the /IRQ2 pin, reads 1 while the DUART's ISR AND IMR is $00 and 0
while it is $02: /IRQ2 follows the DUART's enabled interrupts
(2026-11-21, step W7 of smartclock's checklist).

## The FPGA's read map

Writing the select byte to $304001 ((fconf_regs[1] & 7) | select << 3)
and reading $304000 in a halt, select $00-$1F: the high nibble is the
seconds count; the low nibble reads 0 for $00-$01, 4 for $02-$07, 0 for
$08-$11, the count's nibbles for $12-$17, and 0 for $18-$1F
(2026-11-21, step W8 of smartclock's checklist; restore the select
afterwards, as the firmware's fpga_read does).

## No outer oven

This board has no outer oven (owner, 2026-11-21).  PGP5, the outer
oven's enable on related models, reads 0 (the 3704 firmware never
raises it); what it drives here is not established.

## Programming the flash over BDM

From one reset entry (firmware never run), with the chip selects set
as the boot code sets them (CSORBT $6870 first, so no write reaches
CSBOOT; CS6 $0005/$7070 and CS7 $0405/$7070 the write selects), the
flash takes AMD's command words, one per byte lane (2026-11-21, stages
A-C of smartclock's flash procedure):

- Each chip's WE is its bank's write select (U33 pins 121 CS6 and 122
  CS7) and its OE the read select (112 CSBOOT, 114 CS1), traced by the
  owner.  So a write that asserts CSBOOT programs nothing, but drives
  the chips' outputs against the CPU's: narrow CSBOOT and make it
  read-only first.
- Autoselect ($AAAA to base+$AAAA, $5555 to base+$5554, $9090 to
  base+$AAAA): $0101 manufacturer, $2020 device at base and base+2,
  both pairs: four Am29F010s.  Sector protect ($n*8000+4): $0000, all
  sixteen sector pairs.  $F0F0 returns to read mode.
- Program ($A0A0, then the word): $5AA5 at $70000 read back at once.
- Sector erase ($8080, unlock, $3030 to the sector pair): $70000 in
  0.48 s, $78000 in 0.52 s.
- $78000's two sum words ($7FFFC $BCAE, $7FFFE $0C74) programmed
  back; the whole 512 KB then hashed to the 3704 image again, and
  `bdm reset --run` booted it.

Each `bdm` command costs about 7 ms (a process, USB open and Hello
each), so a whole image this way would take most of an hour; a
sequence request or a RAM routine is the way for that.

smartclock's flash tool takes the second way: through the Python
module, one `Pod` open per run, it writes a position-independent
routine, a vector table and each sector pair's data into RAM, GOes,
and polls `background` until the routine's BGND (`Entry.Background`,
RPC its address plus 2); the routine erases, programs and verifies at
bus speed.  Five runs at 1 MHz DSCLK (2026-11-21, pod at main
a6330e9): a dry run (512 KB read, the 3704 image, 10.4 s with the
setup); identify (four Am29F010s, $0101 and $2020, none protected);
sector pairs $70000 and $78000 erased, programmed, verified and the
whole flash hashed, 25.4 s; both restored, the flash the 3704 image
again; then `Pod.reset` booted it (*IDN? 3704-C, self test OK).
SIMCR read $60CF at each reset entry, closing the `Pod` left the CPU
halted between runs, and the pod reported no error.

## To check

- After `bdm go` and a halt with the firmware's map set, the CPU left
  background mode on its own within a few commands; RSR then read $10,
  a halt monitor reset (MC68331UM D.3.4).  Reading from a reset entry,
  as above, avoids it.  The boot code's SYPCR $CC enabling the halt
  monitor fits.
