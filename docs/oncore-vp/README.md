# Motorola Oncore VP

A GPS receiver built around an MC68331.  Its BDM signals come out on
test points; it has no BDM header.  "To confirm" marks what came from
a photo or one buzz-out and still needs a check.

| File | What |
|---|---|
| `test-points.html` | the test points labeled on a photo, the CPU's lower-edge pins, and the wiring to a 10-pin BDM header ([rendered](https://htmlpreview.github.io/?https://github.com/charlieh0tel/mc683xx-pod/blob/main/docs/oncore-vp/test-points.html)) |
| `front-denu9621.jpg` | CPU, RTC, GPS header and test points (board DENU9621) |
| `lower-edge-devt9614.jpg` | the CPU's lower edge, its pull-ups and B5-B11, traces visible (board DEVT9614) |

## Parts

| Part | What | Source |
|---|---|---|
| MC68331CFC16 | CPU, 16 MHz, 132-pin PQFP | marking |
| TN28F010-120 ×2 | Intel flash, 128 K × 8 each, 120 ns | marking |
| MC68HC68T1 | real-time clock | marking |
| VDD | 5 V (to confirm) | |

The two 28F010s, one per byte lane, are a 16-bit boot ROM of 256 KB at
0x000000-0x03FFFF, on CSBOOT (to confirm).  CSBOOT's reset block
starts at 0 and is at least 512 KB (MC68331UM gives 512 KB and 1 MB in
different places), so the flash reads without chip-select setup and
probably repeats from 0x40000.

## Test points

A row along the board edge below the CPU's lower edge (pins 51-83, left
to right; pin 1 at the top center of the package), and a column at the
board's left edge.

| Test point | MC68331 pin | Signal | State |
|---|---|---|---|
| B1-B4 | | GPS output header | traced |
| B5 | VSS | GND | buzzed |
| B6 | 54 | IPIPE/DSO | traced |
| B7 | 55 | IFETCH/DSI | traced; 10 kOhm pull-up |
| B8 | 56 | BKPT/DSCLK | traced; 10 kOhm pull-up, same via as B7 |
| B9 | 58 | FREEZE/QUOT | traced |
| B10 | 66 | CLKOUT | traced; not VDD (buzzed) |
| B11 | 68 | RESET | buzzed; trace goes to a via, to the back |
| L6 | VDD | +5 V | buzzed |
| L1-L5, M1 | | unknown | |

B5-B11 are a programming fixture's BDM set, with CLKOUT.  BERR (pin 70)
and DS (pin 85, PE5/DS) have no test point.

The resistors below the lower edge are 10 kOhm ("103") pull-ups.  A bus
joining pins 61 (VDDSYN), 63 and 65 (VDD) pulls up pin 57 or 58 (not
clear from a photo; a pull-up on 57, TSC, would tristate the CPU, so
58).  The rest pull up 55 (DSI) and 56 (BKPT) to one via, and 69
(HALT), 70 (BERR), 71-77 (IRQ7-IRQ1, tied together) and 78 (MODCLK) to
vias; VDD presumably.  Traced on board DEVT9614, taken to share
DENU9621's layout.

Pin numbers: MC68331UM Figure 3-2.

## 10-pin BDM header

The standard CPU32 BDM header, wired from the test points:

| Header pin | Signal | From |
|---|---|---|
| 1 | DS | CPU 85, optional |
| 2 | BERR | CPU 70 |
| 3, 5 | GND | B5 |
| 4 | BKPT/DSCLK | B8 |
| 6 | FREEZE | B9 |
| 7 | RESET | B11 |
| 8 | IFETCH/DSI | B7 |
| 9 | VDD | L6 |
| 10 | IPIPE/DSO | B6 |

## Reading the flash over BDM

From reset, halted in background mode:

1. Set SIMCR (0xFFFA00) bit 14, FRZSW.  At reset the software watchdog
   is on and, with FRZSW 0, runs while the CPU is halted (MC68331UM,
   SIMCR).
2. Read 0x000000-0x03FFFF (256 KB).

With VPP at its read level the 28F010s ignore commands, so a stray
write cannot change them (Intel 28F010 datasheet).  With this repo's
`bdm`:

```
bdm reset
bdm dump 0xfffa00 2 --size word       # SIMCR
bdm write 0xfffa00 HI LO --size word  # SIMCR with FRZSW set
bdm dump 0 0x40000 --size long --output oncore-vp.bin
```
