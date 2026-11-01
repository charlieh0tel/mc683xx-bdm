# m683xx-pod

USB background debug mode (BDM) pod for Motorola/Freescale CPU32
(683xx) parts, primarily the MC68331. Raspberry Pi Pico (RP2040) or
Pico 2 (RP2350) + PIO for the serial link, 5 V level shifting, Rust firmware (Embassy) and host.

Goals:
- Halt, inspect, and modify a running 68331 (registers, memory,
  SIM/QSM).
- Load and run code in RAM; program external flash.
- Share a `Cpu32Target` trait with the `cpu32`/`mc68331` emulator
  crates so the smartclock harness can run against silicon or the
  emulator.  The trait is this repo's `cpu32-target` crate; the
  emulator depends on it by git.

Non-goals (for now): other BDM families (ColdFire, HCS08), GDB stub,
real-time trace.

License: GPL-3.0-only (`LICENSE`).

---

## Target connector

10-pin, 2×5, 0.1" (standard CPU32 BDM):

| Pin | Signal      | Pin | Signal      |
|-----|-------------|-----|-------------|
| 1   | DS          | 2   | BERR        |
| 3   | GND         | 4   | BKPT/DSCLK  |
| 5   | GND         | 6   | FREEZE      |
| 7   | RESET       | 8   | IFETCH/DSI  |
| 9   | VDD         | 10  | IPIPE/DSO   |

---

## Hardware

| Function | Part | Notes |
|---|---|---|
| MCU | Pico (RP2040) or Pico 2 (RP2350), socketed | USB to host; same footprint and pinout |
| 3.3 V → 5 V outputs | 74AHCT125, VCC = target VDD | BKPT/DSCLK, DSI; series 47–100 Ω; each its own OE |
| AHCT125 OE pull-up, BKPT/DSCLK | 10 kΩ to target VDD | Output tristated while the pod is unpowered or booting |
| DSI enable gate | 74AHCT1G00, VCC = target VDD | Drives DSI's 74AHCT125 OE: NAND(FREEZE, DSI_EN), so DSI is driven only while FREEZE is high and the Pico enables it |
| DSI_EN pull-down | 100 kΩ to GND | DSI off while the pod is unpowered or booting |
| 3.3 V → 5 V outputs, spare | 74AHCT125, 2 channels | Unused; inputs tied to GND |
| 5 V → 3.3 V inputs | 74LVC125, VCC = 3.3 V | IPIPE/DSO, FREEZE, RESET sense, DS; inputs to 5.5 V at any VCC. IPIPE is pin 10 outside BDM, so its channel reads both |
| RESET drive | 2N7002, open-drain | Target also drives RESET; sense separately |
| BERR drive | 2N7002, open-drain | Abort hung bus cycles |
| 2N7002 gate pull-downs | 100 kΩ to GND | RESET and BERR not asserted while the pod is unpowered or booting |
| VDD sense | 100 kΩ / 100 kΩ divider → ADC0 | 2.75 V at 5.5 V; high value to limit current into an unpowered RP2040. Halt and reset refuse below 4.5 V |
| BKPT pull-up | 10 kΩ to target VDD | Target runs normally with pod unpowered |

Two output enables: BKPT must be driven to request a halt while DSI
stays tristated, because pin 8 is IFETCH, a CPU output, whenever
FREEZE is negated (gotcha below).  DSI's is gated in hardware by
FREEZE itself, so DSI goes off whatever ends background mode: GO,
CALL, a reset from the target's side, a watchdog, a raw Transfer.
From FREEZE falling to DSI off is at most 20 ns: 10 ns through the
74AHCT1G00 and 10 ns for the 74AHCT125 to disable, both at 50 pF over
temperature (SCLS316P 5.6, SCLS264R 5.5).  The manuals give no bound
on how soon IFETCH is driven after FREEZE negates (MC68331UM Table A-7,
Figure A-14); the series resistor limits any overlap's current.

The Pico's DSI_EN is a PIO pin as well: the program drops it 3 PIO
cycles after the last rising edge of GO's opcode or CALL's last word,
about 190 ns at 1 MHz DSCLK (PIO program), which may beat FREEZE
negating.  The firmware raises it once FREEZE asserts.

Pico pins (GPn):

| GP | Signal | Direction |
|---|---|---|
| 2 | BKPT/DSCLK | PIO side-set and set |
| 3 | DSI | PIO out |
| 4 | IPIPE/DSO | PIO in |
| 5 | FREEZE | in, pull-down |
| 6 | RESET sense | in |
| 7 | DS | in |
| 8 | RESET drive (2N7002 gate) | out, high asserts RESET |
| 9 | BERR drive (2N7002 gate) | out, high asserts BERR |
| 10 | 74AHCT125 OE, BKPT/DSCLK | out, low enables |
| 11 | DSI_EN, to the 74AHCT1G00 | PIO set, high enables |
| 25 | LED (on the Pico) | out |
| 26 | VDD sense (ADC0) | analog |

Board: a 2-layer carrier with the translators and the BDM header; the
Pico plugs in. A custom RP2040 PCB only if the carrier proves too
bulky.

Translators stay on either Pico: the RP2350's GPIOs are "5 V-tolerant
(powered) and 3.3 V-failsafe (unpowered)" (RP2350 datasheet, chapter 1
features), and the pod must sit harmlessly on a running target while
unpowered.

Unpowered sides (TI datasheets in `third_party/ti/`):
- Target off, pod on: 74AHCT125 inputs at 3.3 V with VCC = 0 are within
  ratings: VI to 7 V at any VCC, clamp only below 0 V (SCLS264R, 5.1).
- Target off, pod on: the 74AHCT1G00's DSI_EN input at 3.3 V with
  VCC = 0 is within ratings: VI to 7 V at any VCC (SCLS316P, 5.1).
- Pod off, target on: 74LVC125A inputs at 5 V with VCC = 0 are within
  ratings: VI to 6.5 V, 5.5 V recommended, at any VCC (SCAS290T, 5.1,
  5.3).  Its outputs face the unpowered Pico; no Ioff is specified,
  and none is needed with both sides off.

**Gotcha:** pin 8 is IFETCH (an output) when the CPU is not in BDM. Keep the DSI driver tristated until FREEZE is asserted.

---

## Protocol summary

- BKPT/DSCLK idles **high**. Low while running requests a halt.
- **Enable BDM:** BKPT low at the rising edge of RESET; BDM stays enabled until the next reset. BKPT low at least 2 clocks before RESET negates (MC68331UM 5.10.2.1).
- **Enter BDM from reset:** BKPT low → pulse RESET → release RESET → wait for FREEZE.
- **Halt:** BKPT low → wait for FREEZE.
- **BKPT stays low after FREEZE.** The CPU takes each DSCLK rising edge as a bit, so a separate release would be a spurious clock. The first transfer's first rising edge releases it, as CPU32RM's sample circuit does (7.2.7.2, Figures 7-8, 7-9). DSCLK is left high after the last bit of each transfer, so the prefetch at the end of a session is not tagged.
- **First command after entry:** RSREG ATEMP, which holds the entry source and which later commands overwrite (MC68331UM 5.10.2.3, Table 5-4).
- **Transfer:** 17 bits, MSB first, full duplex. The CPU acts only on DSCLK rising edges, synchronized to CLKOUT: it takes DSI, then half a CLKOUT later puts the next bit on DSO, which need not stay stable until the falling edge (CPU32RM 7.2.7.1). The host changes DSI after the falling edge and samples DSO at the end of the low phase, just before the next rise. MC68331UM 5.10.2.7's "transitions on the falling edge" is the host's side; CPU32RM governs.
- **DSI hold:** without a clock shared with the target, at least one CLKOUT period after the DSCLK rise (CPU32RM 7.2.7.1). DSI changes only at the next fall, a whole high phase later.
- **Timing** (MC68331UM Table A-7): DSCLK from DC to half the system clock; cycle ≥ 2 tcyc, low ≥ 1 tcyc; DSI setup 15 ns, hold 10 ns; DSO delay ≤ 25 ns. At 16.78 MHz that is ≤ 8.39 MHz; start at 1 MHz and raise it (Bring-up checklist).
- **Blocks:** DUMP follows a READ, which sets the address; FILL follows a WRITE (MC68331UM Table 5-5).

Response bit 16 (MC68331UM Table 5-6):

| Bit 16 | Data   | Meaning |
|--------|--------|---------|
| 0      | xxxx   | Valid data |
| 0      | FFFF   | Command complete; status OK |
| 1      | 0000   | Not ready; clock NOP and retry |
| 1      | 0001   | BERR-terminated bus cycle |
| 1      | FFFF   | Illegal command |

Opcodes (b/w/l), CPU32RM 7.2.8.4–7.2.8.15.  RSREG's figure there
(7.2.8.6) prints `0x2480`, the same as WSREG's: a misprint.  `0x2580`
is from Pisa's driver documentation (References) and is checked on
silicon at bring-up (RSREG ATEMP after entry).

| Command | Opcode |
|---|---|
| RDREG/RAREG (D0–D7 = 0–7, A0–A7 = 8–F) | `0x2180 \| r` |
| WDREG/WAREG | `0x2080 \| r` |
| RSREG | `0x2580 \| s` |
| WSREG | `0x2480 \| s` |
| READ | `0x1900` / `0x1940` / `0x1980` |
| WRITE | `0x1800` / `0x1840` / `0x1880` |
| DUMP | `0x1D00` / `0x1D40` / `0x1D80` |
| FILL | `0x1C00` / `0x1C40` / `0x1C80` |
| GO | `0x0C00` |
| CALL | `0x0800` |
| RST | `0x0400` |
| NOP | `0x0000` |

System register select codes `s` (CPU32RM 7.2.8.6): RPC 0, PCC 1,
ATEMP 8, FAR 9, VBR A, SR B, USP C, SSP D, SFC E, DFC F.

DUMP and FILL are valid only after a READ/DUMP or WRITE/FILL; NOP pads
between them without losing the address (CPU32RM 7.2.8.10, 7.2.8.11).

---

## PIO program

```
.program cpu32_bdm
.side_set 1 opt              ; side-set pin = BKPT/DSCLK, idle high
.wrap_target
    pull block               ; word << 15 (left-justified)
    set x, 16
bit:
    out pins, 1     side 0 [6]   ; DSCLK low: drive DSI
    in  pins, 1                  ; sample DSO at the end of the low phase
    jmp !x last     side 1       ; DSCLK rises: target takes DSI, then shifts DSO
    jmp x-- bit            [6]   ; rest of the high phase, if a bit follows
last:
    out y, 1                 ; release flag: the OSR bit after the word
    jmp !y keep
    set pins, 0              ; DSI_EN low: DSI off
keep:
    push block               ; 17 bits, right-justified
.wrap
```

Config: out and in shift left, no autopull/autopush. `out` base = DSI,
`in` base = DSO, side-set base = DSCLK, `set` base = DSI_EN (GP11,
count 1).

- The firmware pushes `word << 15`, with bit 14 set for the last word
  before the CPU leaves background mode (`Link::transfer_exit`): DSI
  goes off 3 PIO cycles after that word's last rising edge, before the
  `push`.  The firmware turns DSI on again when FREEZE next asserts.

- Each phase is 8 PIO cycles, so DSCLK = PIO clock / 16; set it with
  clkdiv.  `.side_set 1 opt` leaves 3 delay bits, `[7]` at most.
- The high phase's delay is on the loop's back edge, so after the 17th
  rise the release check runs at once: `set pins` is the 3rd cycle
  after the rise.
- DSO is sampled one PIO cycle before DSCLK rises, 15 after the
  previous rise: the target's synchronizer (about 1.5 CLKOUT), its DSO
  delay and both translators' delays must fit in those 15.  At 1 MHz
  DSCLK that is 937 ns.
- After a halt DSCLK is already low, so the first `out ... side 0`
  keeps it low and the first rise is the release (Protocol summary).
- Between words DSCLK stays high (last side-set) for the release
  check, `push`, `pull` and `set` cycles plus any FIFO wait; the high
  phase only grows, which CPU32 allows.
- Reset entry and halt drive DSCLK low, and the firmware drives DSI's
  enable, with embassy-rp's `StateMachine::set_pins`: it pauses the SM
  (stalled on `pull`), points the `set` base at the pin, runs a `set
  pins` and restores the config, so PIO keeps both pins.

---

## Repo layout

One Rust workspace in `firmware/`, as in oncore-emulator, under one
pinned toolchain (`firmware/rust-toolchain.toml`).  `pod-fw` builds
only for the Pico targets, so it has its own `Cargo.lock`.

```
m683xx-pod/
├── hardware/
│   ├── m683xx-pod/      # KiCad schematic + board
│   └── release.py       # JLCPCB release package (uv)
├── production/          # each release as ordered
├── firmware/
│   ├── cpu32-target/    # Cpu32Target trait, no_std; shared with the emulator
│   ├── pod-proto/       # BDM words and command sequences; USB commands; no_std
│   ├── pod-fw/          # Pico / Pico 2 firmware, embassy-rp, PIO
│   └── pod-host/        # Rust library + `bdm` CLI (nusb); Cpu32Target impl
├── third_party/         # vendor manuals + NOTICE
├── README.md
├── PLAN.md
└── TODO.md
```

---

## Firmware (Pico / Pico 2)

Embassy (`embassy-rp`, `embassy-usb`, PIO through `embassy_rp::pio`).
A Cargo feature picks the chip: `rp2040` (thumbv6m-none-eabi, the
default) or `rp235xa` (thumbv8m.main-none-eabihf).
Reasons: async USB and PIO FIFO waits without a hand-written state
machine; `embassy-usb` vendor class; matches the async rules in
`AGENTS.md`.

USB: vendor class, one bulk OUT and one bulk IN endpoint, 64-byte
packets, one request per packet and one reply per request
(`pod-proto`'s `usb`).  VID:PID 1209:0001, pid.codes' test pair, until
the pod has its own.  Requests:

- Transfer: up to 15 raw words, each answered with the 17 bits shifted
  in, for bring-up and commands the pod does not sequence.  Not
  guarded: with the target running its clocks are BKPT pulses.
- Pins: FREEZE, RESET, DS levels and VDD in millivolts.  DSO is a PIO
  pin, which embassy-rp gives no way to read, so it is not reported.
  BERR is driven but not sensed: the 74LVC125's four channels are used.
- ResetHalt, Halt: the sequences in Protocol summary; refused without
  VDD, and NoFreeze if FREEZE does not assert in 100 ms.  Each then
  reads ATEMP, the entry source, as the first command, and returns it.
- SetClock: DSCLK's rate; the reply gives the rate set, never faster
  than asked and at most 4 MHz until the throughput test measures the
  margin (`pod-proto`'s `dsclk::MAX_HZ` has the arithmetic).
- Every BDM command, refused with NoFreeze unless FREEZE is asserted,
  since its clocks would otherwise be BKPT pulses:
  - ReadRegister, WriteRegister: D0-D7, A0-A7.
  - ReadSystemRegister, WriteSystemRegister: by select code.
  - ReadMemory, WriteMemory: up to 56 bytes in byte, word or long
    accesses, READ then DUMP or WRITE then FILL, on-chip.  Through SFC
    and DFC, which the host sets.
  - Go, Call: the last word through `Link::transfer_exit`.

The firmware's own sizes, release builds: about 24 KB of flash and
2.6 KB of statics on either chip.

Handle not-ready retries in firmware; report BERR and illegal-command
errors to the host.  `pod-proto`'s `session` does both, over a `Link`
trait that the PIO state machine implements; tests run it against
scripted CPU32RM command sequences.

Each result is collected with NOPs, so a DUMP or FILL block costs about
two transfers per access.  Sending the next DUMP in place of the NOP
would nearly halve that; done if the throughput test asks for it.

Flash and debug: `probe-rs` over SWD (`cargo run` in `firmware/pod-fw`),
which supports RP2040 and RP235x; without a probe, `picotool load -x`
over BOOTSEL.

Stack peak: paint the stack with a pattern at boot; a `stack`
USB command reports the high-water mark.  Not written yet.

## Host (Rust)

- `nusb` transport, `pod-proto` types.
- `Cpu32Target` trait: `halt`, `go`, `reset`, `read_reg`/`write_reg`,
  `read_mem`/`write_mem`. Defined in `cpu32-target`; implemented here
  and by the emulator.
- CLI: `bdm reset`, `bdm regs`, `bdm dump <addr> <len>`, `bdm load
  <srec/elf>`, `bdm go [addr]`.
- Target init script (chip selects, SIM setup) run after each reset.

---

## Bring-up checklist

- [ ] Schematic, breadboard first (Pico + 74AHCT125 + 74LVC125 +
      2N7002s), then the carrier
- [ ] PIO loopback: scope DSCLK/DSI, verify 17-bit framing and
      idle-high
- [ ] Reset into BDM; confirm FREEZE asserts
- [ ] RSREG ATEMP first (`0x2588`): confirms RSREG's `0x2580`
- [ ] NOP / not-ready handling
- [ ] RDREG D0; RSREG RPC
- [ ] Read SIMCR (0xFFFA00) and other SIM regs; compare to reset
      values
- [ ] Write chip-select init; read/write external RAM
- [ ] Block DUMP/FILL throughput test; raise DSCLK until it fails,
      back off
- [ ] Load and run a test program from RAM
- [ ] Flash programming (target-resident algorithm)
- [ ] `Cpu32Target` shared with emulator; run smartclock harness on
      silicon

## Open questions

- RSREG `0x2580` on silicon (Protocol summary).
- A VID:PID of the pod's own (pid.codes, or a vendor's).

## References

- `third_party/motorola/CPU32RM.pdf`: BDM (7.2), serial interface
  (7.2.7), command set (7.2.8)
- `third_party/motorola/MC68331UM.pdf`: BDM (5.10.2), timing (Table
  A-7), connector (Figure 5-11), SIM, reset configuration
- `third_party/motorola/SIMRM.pdf`: SIM reference, BKPT timing at reset
- `third_party/ti/`, `third_party/diodes/`: translator and MOSFET data
  sheets
- `third_party/raspberrypi/`: RP2040, RP2350, Pico and Pico 2 data
  sheets (PIO, GPIO ratings)
- Pavel Pisa, "BDM Interface for Motorola 683xx MCU":
  https://cmp.felk.cvut.cz/~pisa/m683xx/bdm_driver.html
- m68k-bdm (Linux driver, bdm-load):
  https://git.firebee.org/Firebee/m68k-bdm
