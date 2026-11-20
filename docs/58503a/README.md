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
reference, 8.39 MHz.  DSCLK at 1 MHz works.

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
background mode off.  `bdm go` straight after `bdm reset` also starts
it from the reset vector.

## To check

- After `bdm go` and a halt with the firmware's map set, the CPU left
  background mode on its own within a few commands; RSR then read $10,
  a halt monitor reset (MC68331UM D.3.4).  Reading from a reset entry,
  as above, avoids it.
