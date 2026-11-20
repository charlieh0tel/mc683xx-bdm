# Motorola Oncore VP

A GPS receiver built around an MC68331.  Its BDM signals come out on
test points; it has no BDM header.  "To confirm" marks what came from
a photo or one buzz-out and still needs a check.

| File | What |
|---|---|
| `test-points.html` | the test points labeled on a photo, their positions, the CPU's lower-edge pins, and the wiring to a 10-pin BDM header ([rendered](https://htmlpreview.github.io/?https://github.com/charlieh0tel/mc683xx-pod/blob/main/docs/oncore-vp/test-points.html)) |
| `front-denu9621.jpg` | CPU, RTC, GPS header and test points (board DENU9621) |
| `lower-edge-devt9614.jpg` | the CPU's lower edge, its pull-ups and B5-B11, traces visible (board DEVT9614) |
| `scan-devt9614-600dpi.jpg` | the whole board, component side, 600 dpi, turned to match the photos; the positions below come from it |

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

## Positions

Measured on a 600 dpi scan of board DEVT9614, component side, about
±0.2 mm.  Board held as in the photos: component side up, CPU marking
upright, test-point row along the bottom edge.  Origin: the bottom-left
mounting hole's center; x right, y up.

| Feature | x (mm) | y (mm) | x (in) | y (in) |
|---|---|---|---|---|
| Mounting hole, bottom left | 0.00 | 0.00 | 0.000 | 0.000 |
| Mounting hole, bottom right | 44.34 | 0.00 | 1.746 | 0.000 |
| Mounting hole, top left | -0.20 | 75.81 | -0.008 | 2.985 |
| Mounting hole, top right | 43.83 | 76.30 | 1.726 | 3.004 |
| Notch, bottom edge (semicircle) | 31.85 | -2.75 | 1.254 | -0.108 |
| L6 | -0.27 | 8.89 | -0.011 | 0.350 |
| B1 | 5.40 | -0.56 | 0.213 | -0.022 |
| B2 | 7.98 | -0.56 | 0.314 | -0.022 |
| B3 | 10.52 | -0.57 | 0.414 | -0.022 |
| B4 | 13.02 | -0.43 | 0.513 | -0.017 |
| B5 | 15.50 | -0.36 | 0.610 | -0.014 |
| B6 | 18.05 | -0.46 | 0.711 | -0.018 |
| B7 | 20.58 | -0.59 | 0.810 | -0.023 |
| B8 | 23.16 | -0.56 | 0.912 | -0.022 |
| B9 | 25.62 | -0.58 | 1.009 | -0.023 |
| B10 | 28.24 | -0.64 | 1.112 | -0.025 |
| B11 | 34.54 | -0.64 | 1.360 | -0.025 |

The holes fit a 1.750 × 3.000 in rectangle within the measurement.
B1-B10 fit a 0.100 in pitch, B11 0.250 in past B10, the row about
0.5 mm below the bottom holes' line.  L6 is on the left holes' line.
The notch's center is on the board edge; its radius, about 1.7 mm, is
from a photo.

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
