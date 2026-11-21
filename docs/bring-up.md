# Bring-up

The steps from a bare Pico to a debugged 68331, in order, each with how
to run it and what to expect.  `PLAN.md`'s bring-up checklist is the
list; this is how to do each item.  Write what each step found into
`PLAN.md` (a figure, a confirmed or refuted assumption) in the commit
that records it.

`bdm` below is `cargo run -p mc683xx-bdm --bin bdm --` from the
repository's root.

## Before the hardware

The host side runs now against a stand-in pod on a model CPU32, which
answers as the pod does and runs only NOP, MOVEQ, BRA.S, TRAP, ILLEGAL
and BGND from its RAM (`mc683xx-bdm-model`'s `program`):

```
cargo run -p mc683xx-bdm --bin bdm-stand-in -- unix:/tmp/pod.sock &
bdm --pod unix:/tmp/pod.sock reset      # halted: breakpoint
bdm --pod unix:/tmp/pod.sock regs
```

Use it to rehearse a step's commands; the expected outputs below are the
pod's.

## 1. Bare Pico: firmware and USB

Set up once: the udev rule (`README.md`), and the Raspberry Pi Debug
Probe on the Pico's SWD pads (J2 on the carrier).

```
cd firmware/pod-fw
cargo run --release              # flashes over SWD, then shows defmt logs
```

Expect `mc683xx-pod up` in the log, and the Pico's LED (GP25) on; an
RP2040-Zero has none there (its RGB LED on GP16 stays dark).

Without a probe, flash a build without RTT over USB.  A blank Pico is
in BOOTSEL already.  A flashed one reboots into it with `bdm bootsel`
(or `picotool reboot -u`, `picotool load -f`), then takes the UF2
copied onto its RPI-RP2 drive or `picotool load`; firmware older than
the reset interface needs BOOTSEL held as USB is plugged in:

```
cargo build --release --no-default-features -F rp2040
picotool load -x -t elf target/thumbv6m-none-eabi/release/pod-fw
```

It logs nothing; the pod enumerating as 1209:0001 (`lsusb`) stands in
for the log line.  A rev 0.2 carrier has J2 not fitted: flash it this
way.

```
bdm pins
```

Expect FREEZE, RESET, DS and BERR low (no carrier: GP5 is pulled down,
GP6, GP8 and GP12 float and may read either way).  VDD reads wherever GP26 floats
without the carrier's divider: 1.06-1.14 V on an RP2040-Zero.  `bdm halt`
fails with "no target power" (NoTarget): VDD is under 4.5 V.

On the carrier, step 3's readings apply.

The watchdog (PLAN.md, Firmware, Watchdog), production build:

- Leave the pod idle a minute, then `bdm pins`: it answers, and the
  kernel log (`journalctl -k`) shows no new enumeration.
- `bdm bootsel`, then wait 30 s: the RPI-RP2 drive stays mounted, and
  `picotool load` still takes the UF2.
- With the `rtt` build under probe-rs, halt the core for over 8 s and
  resume: the pod is not reset.

## 2. Bare Pico: PIO loopback

Jumper GP3 (DSI) to GP2 (DSO).  No carrier, no target.  On an
RP2040-Zero they are pads 4 and 3, side by side.

On the carrier, not this jumper: U5 drives GP2, and the jumper would
fight its output.  Loop back at J1 instead, through U2 and U5:

- 5 V on pin 9 (VDD), GND on pin 3: U1-U3 run from target VDD.
- Pin 8 (DSI) to pin 10 (DSO).
- Pin 6 (FREEZE) to pin 9: U3 enables U2 only while FREEZE is high.
- `bdm halt` first, whatever it answers: FREEZE already high, the pod
  sets DSI_EN and Q3 (DSCLK to pin 4) to read ATEMP.  A raw transfer
  sets neither.

Scope pins 4 (DSCLK) and 8 (DSI) instead of GP9 and GP3.

```
bdm loopback --rounds 100
```

Expect `loopback: 3600 words ok`.  Each word comes back as sent: DSO is
sampled at the end of DSCLK's low phase, after DSI changed at the fall
(PLAN.md, PIO program).

Scope GP9 (DSCLK) and GP3 (DSI) during `bdm loopback`:

- DSCLK idles high, and each word is 17 clocks.
- The rate is 1 MHz (the default): low and high about 500 ns each.
- DSI changes only after DSCLK falls; the first bit is the control bit,
  low.

Then raise the rate and repeat:

```
bdm clock 4000000                # replies with the rate set, at most 4 MHz
bdm loopback --rounds 100
```

The loopback's reported time over the word count gives the USB-limited
transfer rate; record it.

## 3. Carrier, no target

The Pico on the carrier, the BDM cable unplugged.

```
bdm pins
```

Expect FREEZE, RESET, DS and BERR low (the 100 kOhm pull-downs on the
U4-U8 inputs) and VDD near 0 V.  With a 5 V bench supply on J1 pin 9
(VDD) and pin 3 (GND), VDD should read 5.0 V within a few percent (the
100k/100k divider and the Pico's 3.3 V reference).  Record the error.

With the supply on, 5 V on each input in turn reads high: pin 1 (DS),
pin 2 (BERR), pin 6 (FREEZE), pin 7 (RESET).

## 4. Target, pod unpowered

Plug the cable into a running target with the pod's USB unplugged.

- The target runs normally: R3 holds BKPT high, and U1's and U2's
  outputs are off (PLAN.md, Hardware).
- Scope BKPT and DSI at the header: no glitches as the cable goes in.

## 5. Reset into background mode

USB plugged in, target powered.

```
bdm pins                         # FREEZE low, RESET and BERR high, VDD about 5 V
bdm reset                        # halted: breakpoint
bdm pins                         # FREEZE high, ATEMP $00000000 (breakpoint)
```

- FREEZE asserts within 100 ms of RESET rising, or `bdm reset` fails
  with "target not in background mode" (NoFreeze).
- ATEMP fresh and 0 after a reset entry confirms RSREG's encoding,
  0x2580 (PLAN.md, Protocol summary): CPU32RM's figure prints 0x2480,
  which is WSREG.  Record the result in PLAN.md.

```
bdm regs
bdm get rpc
bdm get ssp
```

Record RPC and SSP against the reset vectors in the target's ROM at
address 0 (initial SSP, then initial PC).

A pod restarted while the target is halted can glitch BKPT: as the
Pico resets, GP9 (U1's input) falls before GP10 turns U1 off, and the
halted CPU takes the pulse as a DSCLK clock.  On a target that left
the CPU refusing commands (RSREG ATEMP illegal) until `bdm reset`;
idle time alone does not.  `bdm bootsel` turns U1 off before its
reboot, and a restart that way leaves the CPU answering
(docs/58503a/); the production build's panic handler does the same
before the watchdog resets the pod.  A power loss, a replug, a panic
in the `rtt` build or a watchdog reset with no panic (no software runs
before it) still can.  After such a restart: `bdm reset` (or
`bdm resync`, if only the link's framing is off).  A watchdog reset
of the pod should be rare (PLAN.md, Firmware, Watchdog).

To leave the target running: `bdm go` straight after `bdm reset`
starts it from the reset vector; `bdm reset --run` resets it to run
with background mode off, also from a target out of frame or already
running.

## 6. GO and IFETCH against FREEZE

Scope pin 6 (FREEZE) and pin 8 (IFETCH/DSI) at the header, triggered on
FREEZE falling:

```
bdm go
```

- DSI's driver turns off within 20 ns of FREEZE falling (PLAN.md,
  Hardware: 74AHCT1G00 then 74AHCT1G125).
- IFETCH starts toggling only after that; any overlap shows as a step
  across R2 (220 Ohm).

Repeat with a reset from the target's side (its reset button) while in
background mode: FREEZE falls, and DSI goes off the same way.

## 7. Halting a running target

After step 5's reset (BDM enabled) and `bdm go`:

```
bdm halt                         # halted: breakpoint
bdm pins                         # ATEMP $00000000 (breakpoint), fresh
```

After a power-on or reset without the pod holding BKPT, BDM is not
enabled: `bdm halt` answers NoFreeze and the program takes vector 12
(PLAN.md, Protocol summary).  Check both.  After the pod's own
`bdm reset --run` it knows, and refuses before touching BKPT: `bdm
halt` says "background mode is off since bdm reset --run; use bdm
reset" and the program runs on undisturbed.

## 8. Commands

```
bdm set d0 0x12345678
bdm get d0                       # $12345678
bdm set a3 0xdeadbeef
bdm regs
```

Not-ready handling shows on slow accesses (step 9): the pod retries up
to 1000 NOPs, then takes the access to be hung, pulses BERR to end it
and answers BusError (docs/usb-protocol.md).

## 9. SIM registers

```
bdm dump 0xfffa00 128 --size word
```

Compare SIMCR (0xFFFA00) and the other SIM registers with their reset
values in MC68331UM's SIM register table, and with what the target's
firmware sets if it has run.  A bus error here means the access did not
reach the SIM: check SFC (`bdm get sfc`; dump uses fc 5 by default).

## 10. Chip selects and external RAM

The 68331 has no on-chip RAM, so loading and running code needs the
board's chip selects set (SIMRM, chip selects).  By hand until
`bdm init` lands (TODO.md):

```
bdm write ADDRESS BYTE... --size word      # CSPAR, CSBAR, CSOR per the board, big-endian bytes
bdm write RAM 1 2 3 4
bdm dump RAM 4 --size byte                 # 01 02 03 04
```

FRZSW is set at reset (SIMCR $60CF on an MC68331), so the watchdog
stops while the CPU is halted.  A program may clear it: after GO and
a halt, set FRZSW again, or the watchdog resets the CPU.

## 11. A self-entry: BGND

With RAM at `RAM` (step 10): BGND is $4AFA.

```
bdm write RAM 0x4a 0xfa
bdm set rpc RAM
bdm go
bdm pins                         # FREEZE high, ATEMP $00000001 (background), fresh
bdm get d0                       # commands work: DSI is driven again
```

The pod reads ATEMP first after the CPU's own entry (docs/usb-protocol.md,
Semantics).  `fresh` and 1 confirm that path.

## 12. Throughput

```
time bdm dump RAM 4096 --size long --output /dev/null
```

Record bytes per second.  Raise `bdm clock` until DUMPs fail (wrong
data, Unexpected), then back off; record the margin in PLAN.md.  The pod
caps DSCLK at 4 MHz until this test says otherwise (`dsclk::MAX_HZ`).

## 13. Silicon questions

PLAN.md's bring-up checklist lists questions the manuals leave open
(PCC after a reset double bus fault, BKPT out of STOP/LPSTOP, the halt
monitor on a double fault in BDM).  Answer each and tell m683xx-emu,
whose model takes one answer for each.

## 14. Stack peak

After steps 5, 8, 9 and 12 (a reset into BDM, a register dump, a block
DUMP), with the Debug Probe attached and no other probe-rs session:

```
cd firmware/pod-fw
./stack-peak                                     # RP2040 release build
./stack-peak target/thumbv8m.main-none-eabihf/release/pod-fw RP235x
```

It prints the peak against the stack's size.  Record it in PLAN.md
with the chip's SRAM, as AGENTS.md asks.
