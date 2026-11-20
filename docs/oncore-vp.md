# Motorola Oncore VP

A GPS receiver built around an MC68331: a BDM target for the pod.
"To confirm" marks what came from a photo or one buzz-out and still
needs a check.

## Parts

| Part | What | Source |
|---|---|---|
| MC68331CFC16 | CPU, 16 MHz, 132-pin PQFP | marking |
| TN28F010-120 ×2 | Intel flash, 128 K × 8 each, 120 ns; 12 V VPP to erase or program | marking |
| MC68HC68T1 | real-time clock | marking |
| VDD | 5 V (to confirm) | |

The two 28F010s, one per byte lane, are a 16-bit boot ROM of 256 KB at
0x000000-0x03FFFF, on CSBOOT (to confirm).  CSBOOT's reset block
starts at 0 and is at least 512 KB (MC68331UM gives 512 KB and 1 MB in
different places), so the flash reads without chip-select setup and
probably repeats from 0x40000.

## BDM

No BDM header.  A test-point row along the board edge, below the CPU's
lower edge (pins 51-83, pin 1 at the top center of the package), carries
most of a programming fixture's BDM set:

| Test point | MC68331 pin | Signal | BDM header pin | State |
|---|---|---|---|---|
| B5 | VSS | GND | 3, 5 | buzzed |
| B6 | 54 | IPIPE/DSO | 10 | traced by eye |
| B7 | 55 | IFETCH/DSI | 8 | traced; 10 kOhm pull-up, rail to confirm |
| B8 | 56 | BKPT/DSCLK | 4 | traced; 10 kOhm pull-up, same via as B7 |
| B9 | 58 | FREEZE/QUOT | 6 | traced by eye |
| B10 | 66 | CLKOUT | - | traced; not VDD (buzzed) |
| B11 | 68 | RESET | 7 | to confirm; its trace goes to a via, to the back |

The resistors below the lower edge are 10 kOhm ("103") pull-ups.  A bus
joining pins 61 (VDDSYN), 63 and 65 (VDD) pulls up pin 57 or 58 (which
one is not clear from a photo; a pull-up on 57, TSC, would tristate the
CPU, so 58); the rest pull up 69 (HALT), 70 (BERR), 71-77 (IRQ7-IRQ1,
tied together) and 78 (MODCLK) to vias, VDD presumably.  Traced from a
photo of a second board (DEVT9614; the first is DENU9621), taken to
share the layout.

B1-B4, the same row's left end, run to the GPS output header.  L6, in
the column of test points at the board's left edge, is +5 V (buzzed):
BDM header pin 9.

The rest of a 10-pin BDM header:

- BERR (pin 2): pin 70, no test point.  Tack a wire, or leave it open:
  the pod's pull-down then reports BERR low.
- DS (pin 1): pin 85 (PE5/DS), optional; the pod only reports it.

Pin numbers: MC68331UM Figure 3-2.

## Reading the flash

```
bdm reset
bdm dump 0xfffa00 2 --size word       # SIMCR
bdm write 0xfffa00 HI LO --size word  # SIMCR with bit 14, FRZSW, set
bdm dump 0 0x40000 --size long --output oncore-vp.bin  # the 256 KB boot ROM
```

FRZSW first: at reset the software watchdog is on and, with FRZSW 0,
runs while the CPU is halted (MC68331UM, SIMCR).  With VPP at its read
level the 28F010s ignore commands, so a stray write cannot change them
(Intel 28F010 datasheet).
