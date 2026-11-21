# mc683xx-bdm

USB background debug mode (BDM) pod for Motorola/Freescale CPU32
(683xx) parts, primarily the MC68331. Raspberry Pi Pico (RP2040) or
Pico 2 (RP2350) + PIO for the serial link, 5 V level shifting, Rust firmware (Embassy) and host.

Goals:
- Halt, inspect, and modify a running 68331 (registers, memory,
  SIM/QSM).
- Load and run code in RAM; program external flash.
- Share a `Cpu32Target` trait with the `cpu32`/`mc68331` emulator
  crates so the smartclock harness can run against silicon or the
  emulator.  The trait is m683xx-emu's `mc683xx-target` crate
  (github.com/charlieh0tel/mc683xx, crates/target), from crates.io.

- gdb on silicon through the pod, with the same adapter as the
  emulator (Host, gdb).

Non-goals (for now): other BDM families (ColdFire, HCS08), real-time
trace.

License: the hardware design is CERN-OHL-S-2.0 (`hardware/LICENSE`);
firmware and documentation are GPL-3.0-only (`LICENSE`).

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
| MCU | Pico (RP2040) or Pico 2 (RP2350), soldered flat by its castellated edge | USB to host; same footprint and pinout; JLCPCB places a Pico (SC0915, C9900019762); a Pico 2 is fitted by hand |
| 3.3 V → 5 V outputs | 74AHCT1G125 ×2 (U1 BKPT/DSCLK, U2 DSI), VCC = target VDD | 220 Ω series (R1, R2): about 20 mA if DSI and IFETCH overlap (MC68331UM Table A-1: 25 mA per pin), or into an unpowered 74AHCT1G125's output clamp if the cable's signals meet before its VDD (SCLS378P 5.1: ±20 mA); each its own OE |
| 74AHCT1G125 input pull-downs | 100 kΩ to GND on GP9 and GP3 (R15, R16) | The inputs must not float (SCLS378P 5.3, note 1); the Pico's own pull-downs need it powered |
| U1 OE pull-up, BKPT/DSCLK | 1 kΩ to target VDD (R4), pulled low by Q3 (BSS138BK, gate on GP10, 100 kΩ gate pull-down) | The OE edge within the 74AHCT1G125's 20 ns/V (SCLS378P 5.3): about 8 ns/V into its 10 pF (5.5) and Q3's 7 pF; 5 mA from VDD while Q3 is on.  Output tristated while the pod is unpowered or booting.  Q3 keeps the target's 5 V off every Pico pin: a GPIO on this pull-up would see 5 V, over the RP2040's and RP2350's 3.63 V limit unpowered (RP2040 datasheet 5.5.2.1, RP2350 14.9.1) |
| DSI enable gate | 74AHCT1G00 (U3), VCC = target VDD | Drives U2's OE: NAND(FREEZE, DSI_EN), so DSI is driven only while FREEZE is high and the Pico enables it |
| DSI_EN pull-down | 100 kΩ to GND | DSI off while the pod is unpowered or booting |
| 5 V → 3.3 V inputs | 74LVC1G17 ×5, Schmitt trigger, VCC = 3.3 V: U4 RESET, U5 IPIPE/DSO, U6 FREEZE, U7 DS, U8 BERR | One SOT-23-5 per input, each placed at its J1 pin.  RESET and BERR are open drain on the target and rise as its pull-ups let them; a Schmitt input sets no transition-rate limit.  5.5 V inputs at any VCC and Ioff (SCES351Y 5.3, 5.5).  IPIPE is pin 10 outside BDM, so U5 reads both.  4.6 ns max A to Y at 3.3 V (SCES351Y 5.6) |
| 74LVC1G17 input pull-downs | 100 kΩ to GND, header side (R10-R13, R17) | Inputs low with the cable unplugged even at the 74LVC1G17's worst-case 5 µA input leakage (SCES351Y 5.5); 50 µA load at 5 V |
| RESET drive | BSS138BK (Nexperia, LCSC C282529), open-drain | Target also drives RESET; sense separately.  RDS(on) at most 6.5 Ω at VGS = 2.5 V, VGSth at most 1.6 V (BSS138BK datasheet, Table 7): specified below the Pico's 3.3 V gate drive |
| BERR drive | BSS138BK, open-drain | Abort hung bus cycles |
| FET gate pull-downs | 100 kΩ to GND | RESET and BERR not asserted while the pod is unpowered or booting |
| VDD sense | 100 kΩ / 100 kΩ divider → ADC0, 100 nF to GND | 2.75 V at 5.5 V; high value to limit current into an unpowered RP2040; the capacitor holds GP26 while the ADC samples from the divider's 50 kΩ.  Halt and reset refuse below 4.5 V |
| BDM header | 2x5 0.1" shrouded, keyed | A reversed cable cannot put VDD on BERR |
| SWD connector, J2 | JST SH 3-pin horizontal, SM03B-SRSS-TB (LCSC C160403) | From the Pico's debug pads: 1 SWCLK, 2 GND, 3 SWDIO, Raspberry Pi's debug connector order (RP-003139-SP); the Debug Probe's JST-SH cable plugs in.  The RP2040's internal pull-ups, no external ones |
| Decoupling | 100 nF X7R at each IC | No bulk capacitor on target VDD, so plugging in does not load the target's supply |
| BKPT pull-up | 10 kΩ to target VDD | Target runs normally with pod unpowered |

Two output enables: BKPT must be driven to request a halt while DSI
stays tristated, because pin 8 is IFETCH, a CPU output, whenever
FREEZE is negated (gotcha below).  DSI's is gated in hardware by
FREEZE itself, so DSI goes off whatever ends background mode: GO,
CALL, a reset from the target's side, a watchdog, a raw Transfer.
From FREEZE falling to DSI off is at most 20 ns: 10 ns through the
74AHCT1G00 and 10 ns for the 74AHCT1G125 to disable, both at 50 pF over
temperature (SCLS316P 5.6, SCLS378P 5.6).  The manuals give no bound
on how soon IFETCH is driven after FREEZE negates (MC68331UM Table A-7,
Figure A-14); the series resistor limits any overlap's current.

The Pico's DSI_EN is a PIO pin as well: the program drops it 8 PIO
cycles, the whole high phase, after the last rising edge of GO's
opcode or CALL's last word, 500 ns at 1 MHz DSCLK and 125 ns at 4 MHz
(PIO program), which may beat FREEZE negating.  The firmware raises it once FREEZE asserts, or once a CALL
has left FREEZE high (Firmware).

Pico pins (GPn):

| GP | Signal | Direction |
|---|---|---|
| 2 | IPIPE/DSO | PIO in |
| 3 | DSI | PIO out |
| 4 | DSI_EN, to the 74AHCT1G00 | PIO set, high enables |
| 5 | FREEZE | in, pull-down |
| 6 | DS | in |
| 7 | BERR drive (Q2 gate) | out, high asserts BERR |
| 8 | BERR sense | in |
| 9 | BKPT/DSCLK | PIO side-set and set |
| 10 | BKPT/DSCLK driver enable (Q3 gate) | out, high enables |
| 11 | RESET drive (Q1 gate) | out, high asserts RESET |
| 12 | RESET sense | in |
| 25 | LED (on the Pico) | out |
| 26 | VDD sense (ADC0) | analog |

Board: a 4-layer carrier (JLCPCB JLC04161H-7628 stackup, ENIG) with the
translators and the BDM header; the Pico is soldered on flat, placed
by JLCPCB.  A custom
RP2040 PCB only if the carrier proves too bulky.  68 x 59 mm.  In1 is
solid GND; In2 is VDD left of x = 74 mm and +3V3 right of it; GND is
poured on F.Cu and B.Cu too, stitched along the edge, on a 5 mm grid
(`stitch --grid 5 --near 2.5`) and by hand.  Every signal runs
on F.Cu over In1; the Fast net class's nets have no vias
(`hardware/fastcheck.py` reports the plane under each segment).  VDD
reaches the sense divider (R8) on the +3V3 side in short runs on B.Cu.

Translators stay on either Pico: the RP2350's GPIOs are "5 V-tolerant
(powered) and 3.3 V-failsafe (unpowered)" (RP2350 datasheet, chapter 1
features), and the pod must sit harmlessly on a running target while
unpowered.

Unpowered sides (TI datasheets in `third_party/ti/`):
- Target off, pod on: 74AHCT1G125 inputs at 3.3 V with VCC = 0 are within
  ratings: VI to 7 V at any VCC, clamp only below 0 V (SCLS378P, 5.1).
- Target off, pod on: the 74AHCT1G00's DSI_EN input at 3.3 V with
  VCC = 0 is within ratings: VI to 7 V at any VCC (SCLS316P, 5.1).
- Pod off, target on: 74LVC1G17 inputs at 5 V with VCC = 0 are within
  ratings: VI to 6.5 V, 5.5 V recommended, at any VCC, and Ioff limits
  the current to 10 µA (SCES351Y, 5.1, 5.3, 5.5).

**Gotcha:** pin 8 is IFETCH (an output) when the CPU is not in BDM. Keep the DSI driver tristated until FREEZE is asserted.

---

## Protocol summary

- BKPT/DSCLK idles **high**. Low while running requests a halt.
- **Enable BDM:** BKPT low at the rising edge of RESET; BDM stays enabled until the next reset. BKPT low at least 2 clocks before RESET negates (MC68331UM 5.10.2.1).
- **Enter BDM from reset:** BKPT low → pulse RESET → release RESET → wait for FREEZE.
- **Halt:** BKPT low → wait for FREEZE.  Only with BDM enabled at the
  last reset; otherwise BKPT is a breakpoint exception, vector $0C, in
  the running program (MC68331UM 5.10.2.2.1, Table 5-3).
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
is from Pisa's driver documentation (References), confirmed on silicon
(Bring-up checklist: RSREG ATEMP after a reset entry).

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
    nop             [4]      ; DSI held to the high phase's end
    set pins, 0              ; DSI_EN low: DSI off
keep:
    push block               ; 17 bits, right-justified
.wrap
```

Config: out and in shift left, no autopull/autopush. `out` base = DSI,
`in` base = DSO, side-set base = DSCLK, `set` base = DSI_EN (GP4,
count 1).

- The firmware pushes `word << 15`, with bit 14 set for the last word
  before the CPU leaves background mode (`Link::transfer_exit`): DSI
  goes off 8 PIO cycles after that word's last rising edge, before the
  `push`.  The firmware turns DSI on again when FREEZE next asserts.
- DSI hold on that last bit: the CPU wants DSI held one CLKOUT after
  the rise (CPU32RM 7.2.7.1, Protocol summary).  8 cycles is the whole
  high phase, half a DSCLK period, and a DSCLK cycle is at least 2
  CLKOUT for DSCLK to be in spec (MC68331UM Table A-7), so the hold
  covers one CLKOUT at the slowest target clock the rate allows: twice
  DSCLK (8 MHz at 4 MHz, 2 MHz at the 1 MHz default, 20 kHz at the
  10 kHz floor).  At 4 MHz and the 68331's 16.78 MHz that is 125 ns
  against 59.6 ns, 65 ns to spare; U3's and U2's disable delays (up
  to 20 ns) only lengthen it.  FREEZE falling can cut DSI off sooner
  in hardware, but only once the CPU has taken the bit
  (`dsclk::DSI_EXIT_HOLD_CYCLES`, `dsclk::dsi_exit_hold_ns`).

- Each phase is 8 PIO cycles, so DSCLK = PIO clock / 16; set it with
  clkdiv.  `.side_set 1 opt` leaves 3 delay bits, `[7]` at most.
- The high phase's delay is on the loop's back edge, so after the 17th
  rise the release check runs at once, and only an exit word pays the
  `nop`: `set pins` is the 8th cycle after the rise.
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

The host Rust workspace is the repository's root (`Cargo.toml`,
`Cargo.lock`): the host crates and the shared `mc683xx-bdm-proto`, all
in `crates/`, so cargo, cargo-deb and maturin all run at the root.
Each crate's directory is its short name (`crates/proto` is
`mc683xx-bdm-proto`), as in mc683xx.
`firmware/pod-fw` builds only for the Pico targets, so it is its own
workspace with its own `Cargo.lock`, and takes `mc683xx-bdm-proto` by path.
One pinned toolchain (`rust-toolchain.toml`) and one `clippy.toml` and
`rustfmt.toml`, at the root, which cargo, clippy and rustfmt find from
either workspace by searching upward.

```
mc683xx-bdm/
├── hardware/
│   ├── mc683xx-pod/      # KiCad schematic + board
│   ├── production/       # each release as ordered
│   └── ee_tooling/      # shared KiCad tooling and house style (submodule, release/0.2)
├── crates/
│   ├── proto/           # mc683xx-bdm-proto: BDM words and command sequences; USB commands; no_std
│   ├── host/            # mc683xx-bdm: Rust library + `bdm` CLI (nusb); git-version.rs, the git commit in its and pod-fw's versions
│   ├── py/              # mc683xx-bdm-py: Python bindings, `mc683xx_bdm` (PyO3, maturin, uv)
│   ├── model/           # mc683xx-bdm-model: a CPU32 in background mode at the serial word, and a board around it; no_std; test target
│   └── stand-in/        # mc683xx-bdm-stand-in: `bdm-stand-in`, a pod on a socket over the model; test aid
├── firmware/
│   └── pod-fw/          # Pico / Pico 2 firmware, embassy-rp, PIO; its own workspace
├── packaging/           # Debian changelog, udev rule
├── scripts/             # published-crates.sh, for the release workflow
├── third_party/         # vendor manuals + NOTICE
├── Cargo.toml           # the host workspace
├── rust-toolchain.toml  # the toolchain, for both workspaces
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
(`mc683xx-bdm-proto`'s `usb`).  Each request but Hello carries a tag
byte the reply echoes, so a host that gave up on a reply (a timeout,
Ctrl-C) discards it when it comes later instead of taking it for the
next request's; Hello is untagged and fixed across versions, so a
version mismatch is still reported as one.  Each reply also carries
FREEZE's edge count (low 8 bits), so a host sees a leave and re-entry
that FREEZE's level hides.  The answer to each request is
`mc683xx-bdm-proto`'s `pod`, over a `Board` trait: the firmware's board (`board.rs`) is the
Pico's pins, ADC, PIO link and timeouts, so the request rules run in
host tests too.  VID:PID 1209:0001, pid.codes' shared test pair: the
pod has no PID of its own, and none is applied for.  Serial number string: the chip's unique ID in
uppercase hex, 16 characters, read once at boot (`serial.rs`): the
RP2040's flash unique ID (embassy-rp's `blocking_unique_id`), the
RP2350's OTP chip ID; pico-sdk's `pico_unique_board_id` convention, so
picotool and `lsusb -v` show the same string.  It is what tells two
pods apart (`bdm --pod usb:SERIAL`).  Requests:

- Transfer: up to 15 raw words, each answered with the 17 bits shifted
  in, for bring-up and commands the pod does not sequence.  Not
  guarded: with the target running its clocks are BKPT pulses.
- Pins: FREEZE, RESET, DS, BERR levels, VDD in millivolts, and ATEMP for
  the current stay in background mode (below).  DSO is a PIO
  pin, which embassy-rp gives no way to read, so it is not reported.
  BERR is sensed on GP8 (U8).
- ResetHalt, Halt: the sequences in Protocol summary; refused without
  VDD, and NoFreeze if FREEZE does not assert in 100 ms.  Each returns
  ATEMP.  Halt on a CPU already in background mode only returns it.
- ResetRun: RESET pulsed with BKPT high, so the CPU runs with
  background mode off; refused without VDD, and ResetHeld if RESET is
  still low 100 ms later (the target holds it).  It and ResetHalt skip
  the ATEMP read, so a link out of frame cannot stop them; so do Hello,
  Version, SetClock and GetClock, which touch no target, so the Hello
  and GetClock a host sends first cannot stop them either.
- Halt guard: after its own ResetRun the pod knows background mode is
  off, so until a ResetHalt or a FREEZE edge (an entry shows it on) it
  refuses Halt and Hold with BdmDisabled instead of pulling BKPT low,
  which the running program would take as a breakpoint exception.
  After the pod's start or a reset it did not make it cannot know, and
  goes ahead.
- Hold: BKPT low with no ATEMP read, so nothing releases it until the
  next request: the CPU halts and stays in, or comes back to,
  background mode through a reset the pod did not make (a watchdog's).
- BERR abort: a halted CPU not ready after the retries gets a 10 us
  BERR pulse (Q2) and its bus error is read, so a BDM access nothing
  answers (FRZBM set, an unmapped address) cannot leave the pod and
  the CPU stuck until a reset.  Never while FREEZE is low: read from
  the pin right before the pulse.
- Resync: NOP pairs, and a lone DSCLK edge after each pair whose second
  NOP does not read "command complete", until one does (at most 17).
  On request only: on a target a pod restart left the link in frame
  but the CPU refusing commands, which only a reset cleared, so an
  automatic resync bought nothing (docs/bring-up.md, step 5).
- Bootsel turns U1 off (GP10 low, then 10 us) before the boot ROM's
  reboot, so a resetting Pico cannot glitch BKPT low; this needs
  embassy-rp's `unstable-pac` for the one register write.  The
  production build's panic handler does the same before it waits for
  the watchdog (Watchdog, below).
- ATEMP holds the entry source only until the first command after
  entry, and the CPU can enter on its own (BGND, a breakpoint, a double
  bus fault).  A task watches FREEZE and counts its edges (`freeze.rs`):
  after each edge interrupt, and every 10 ms without one, it reads the
  pin's edge latches (one per direction) before the next wait's arming
  clears them, and the level; an edge the latches miss (while arming,
  or a second one the same way) changes the parity the level shows, so
  one more is counted (`entry::freeze_edges`).  A quick exit and
  re-entry (GO onto a BGND) is two latched edges, not lost.  Two missed
  edges together still are.  Before each request, if FREEZE is high and the count moved since the
  last read, the pod drives DSCLK and DSI and reads ATEMP first
  (`mc683xx-bdm-proto`'s `entry`).  It reports it as fresh, or as stale when
  the CPU was halted when the pod started and so may have been
  commanded before.  This also turns DSI on after a self-entry: GO
  turned it off.  Every sequenced command, memory block, BerrPulse and
  Resync drives DSCLK and DSI as well, once FREEZE is checked (no DSCLK
  edge), so none clocks with the link undriven: after the pod's boot,
  or a GO the CPU did not take, with no entry read since.
- SetClock: DSCLK's rate; the reply gives the rate set, never faster
  than asked and at most 4 MHz until the throughput test measures the
  margin (`mc683xx-bdm-proto`'s `dsclk::MAX_HZ` has the arithmetic),
  and at least 10 kHz (`dsclk::MIN_HZ`): with not-ready waits bounded
  by time as well as NOPs (1000 NOPs or 20 ms, `pod::NOT_READY_*`),
  that bounds every request's time, which the host's timeout covers
  at the rate in use (`pod::Workload`; docs/usb-protocol.md).
- GetClock: the rate in effect, as SetClock's reply gave it.  The pod
  keeps its rate from one host to the next, so a host asks it at the
  handshake and times its requests by it, not by the floor: a dead
  pod or pulled cable is then reported in about a second at any rate.
  Only GetClock's own exchange is timed at the floor.
- Every BDM command, refused with NoFreeze unless FREEZE is asserted,
  since its clocks would otherwise be BKPT pulses:
  - ReadRegister, WriteRegister: D0-D7, A0-A7.
  - ReadSystemRegister, WriteSystemRegister: by select code.
  - ReadMemory, WriteMemory: up to 56 bytes in byte, word or long
    accesses, READ then DUMP or WRITE then FILL, on-chip.  Through SFC
    and DFC, which the host sets.
  - Go, Call: the last word through `Link::transfer_exit`.  Then
    Call waits for FREEZE to fall or its edge count to move, within
    100 ms (`Board::left_background`): a CALL whose stacking takes a
    bus or address error leaves the CPU in background mode, and its
    next word answers the error (CPU32RM 7.2.8.13).  Clocking that
    word blind is a BKPT pulse to a CPU that did leave, so FREEZE
    decides first; if it stays high the pod drives DSI again, reads
    the error with a NOP and answers BusError.  The timeout is long
    against stacking's two bus cycles, a bus monitor's timeout and
    FREEZE's 10 ms refresh (`freeze.rs`); a stacking cycle that
    outlasts it with no bus monitor reads as not ready.  Stacking that
    ends after the wait is not a failure: FREEZE, read from the pin
    before DSI is driven and before each NOP, shows the CPU gone, and
    the CALL answers Ok with nothing more clocked.
- FREEZE before every word: each sequenced command reads FREEZE from
  the pin (SIO's `GPIO_IN`, `freeze::level`, not the watcher's last
  look) before each word through `Session` (`Link::freeze_high`), and
  stops with NoFreeze once it is low, so a target reset in the middle
  of a command (a block DUMP or FILL, a not-ready wait, the BERR
  abort's read, Resync) gets no BKPT pulses.  A word already going out
  when FREEZE falls still goes: the check narrows the window to one
  word.  The cost is one register read a word; on the model,
  host-side, DUMP throughput did not change measurably (about
  70 MB/s either way).

Watchdog (`watchdog.rs`, `mc683xx-bdm-proto`'s `watchdog`): the chip's
hardware watchdog, 8 s on either chip, the RP2040's longest (2^24
ticks of 1 us, counted twice: RP2040-E1).  A task checks once a second
and feeds it while the serve loop waits for a request, however long
the host is idle, and while the request it is carrying out makes
progress: a word over the link (each PIO word counts) or the request's
end.  So the chip resets when the executor stops (a loop without an
await, the panic handler's loop, a hard fault) or a request goes 8 s without
a link word.
- Not a free-running ticker: that feeds through a request stuck on an
  await that never ends.  Not fed by the serve loop alone: it sleeps
  in its read for as long as the host is idle.
- The longest legitimate gaps between words: ResetHalt's 10 ms RESET
  pulse and up to 100 ms for FREEZE, and one word at the slowest DSCLK
  (10 kHz: 1.7 ms).  Not-ready retries (up to 1000 NOPs or 20 ms), a
  long DUMP or FILL and Resync are word after word.
- A reply the host does not collect waits in the USB write, which
  counts as idle.  Not caught: a USB stack that stops answering while
  the serve loop waits for a request.
- A watchdog reset is a pod restart: under a halted CPU it takes the
  serial link out of frame and loses the kept ATEMP (`docs/bring-up.md`,
  step 5).  So the timeout is generous and the reset rare: recovery from
  a firmware fault in place of a replug, not part of normal running.
- Panics: the production build's handler (`panic.rs`) disables
  interrupts, turns U1 off as bootsel does (GP10 low, then 10 us), and
  spins, starving it: a reset 8 s later with BKPT held high by R3.  With
  `rtt`, panic-probe faults the core, U1 left as it was; the watchdog
  pauses while a debugger halts the core (CTRL's PAUSE_DBG0/1 and
  PAUSE_JTAG), so under probe-rs the panic stays for inspection; with
  no probe attached it resets as the production build does, but can
  glitch BKPT.
- A watchdog reset with no panic (a loop without an await, a hard
  fault, a request stuck without a link word) is not covered: no
  software runs before it, so it can still glitch BKPT
  (`docs/bring-up.md`, step 5).
- `bdm bootsel`: the boot ROM's `reset_to_usb_boot` reprograms the
  watchdog for its own reboot into BOOTSEL (RP2040 datasheet 2.8.3.1.5,
  Table 168).  The boot ROM's device staying up past the timeout is a
  bring-up check (`docs/bring-up.md`, step 1).
- With `rtt`, a reset by the watchdog's timeout is logged at start
  (WATCHDOG's REASON).  Not reported over USB: that is a protocol
  change (TODO.md).

The firmware's own sizes, release builds (`make size`, also in CI's
summary): 32248 bytes of text on the RP2040 and 30520 on the RP2350,
2896 bytes of statics on either.

Handle not-ready retries in firmware; report BERR and illegal-command
errors to the host.  `mc683xx-bdm-proto`'s `session` does both, over a `Link`
trait that the PIO state machine implements.  Two sets of host tests
run it: scripted word sequences from CPU32RM's figures (`mc683xx-bdm-proto`),
and `mc683xx-bdm-model`, a CPU32 in background mode that answers each word as
CPU32RM 7.2.8 says, decodes opcodes independently of `mc683xx-bdm-proto`, and
records what a real CPU would not take: clocks while running, words
lost while busy, DUMP or FILL without READ or WRITE, GO or CALL
without the DSI release, BERR while running.  Its board can reset the
target in the middle of a command and make CALL's FREEZE fall late.
It is `no_std` so a second Pico can run it
as a stand-in target later.

Each result is collected with NOPs, so a DUMP or FILL block costs about
two transfers per access.  Sending the next DUMP in place of the NOP
would nearly halve that; done if the throughput test asks for it.

Flash and debug: a Raspberry Pi Debug Probe on J2, the carrier's 3-pin
JST-SH SWD connector (SWCLK, GND, SWDIO, from the Pico's debug pads),
with `probe-rs` (`cargo run` in `firmware/pod-fw`), which supports
RP2040 and RP235x; without a probe, `picotool load -x` over BOOTSEL.

Stack peak: cortex-m-rt's `paint-stack` fills the stack with
0xCCCCCCCC at reset, and `firmware/pod-fw/stack-peak` reads it back
over SWD (probe-rs) and reports the deepest word changed.  No firmware
code and no `unsafe`; no USB request, so the protocol is unchanged.

## Host (Rust)

- `mc683xx-bdm`'s `transport`: a `Transport` trait, request packets
  sent and reply packets received, each in order, a receive with its
  own timeout (`Pod` works it out for each request from DSCLK's rate,
  docs/usb-protocol.md); `UsbTransport` over `nusb`, blocking, the device with the pod's VID:PID and the serial number
  asked for (case ignored), or with none asked for the only one: more
  than one is an error listing their serial numbers, never a guess.
  On USB a request goes out without waiting for the pod to take it
  and a read always waits for the next reply: a pod still writing a
  reply the host gave up on takes nothing until that reply is read.
  `Pod` tags each request (1-255, round again) and discards replies
  with another tag, Hello's too, which takes only Hello's own form.
  `PodAddress` is `--pod`'s text (`usb`, `usb:SERIAL`, `unix:PATH`,
  `tcp:HOST:PORT`) and connects, for `bdm` and other callers alike;
  `list_pods` reads each pod's serial number and bus-ports from the
  OS's descriptor cache without opening it.  `StreamTransport`
  on a Unix or TCP socket, each message a length byte and the packet,
  for `bdm-stand-in` (`bdm --pod unix:PATH`).  The
  protocol is `docs/usb-protocol.md`; this repo owns it.  `bdm`
  sends Hello first and stops on another protocol version, then asks
  DSCLK's rate (GetClock) for its timeouts (`Pod::handshake`).
- `mc683xx-bdm`'s `register`: registers by name, `d0`-`a7`, `sp`, `pc`
  (RPC, the emulator's name) and the system registers, read and
  written through any `Cpu32Target`, and the system registers in
  listing order.
- `mc683xx-bdm`'s `pod`: each USB request as a call.  Memory goes as
  56-byte requests and raw transfers as 15-word ones; a chunk never
  splits an access.  Tested against scripted replies.
- `mc683xx-bdm`'s `target`: `PodTarget`, `Cpu32Target` over a `Pod`.  The
  entry source is the pod's ATEMP (Pins, Halt, ResetHalt): fresh
  decodes per Table 7-2, stale is `Entry::Unknown`.  It is kept until
  the target resumes, a request is refused with NoFreeze (the target
  left background mode, a reset say), or a reply's FREEZE edge count
  differs from the one the halt was seen with: the target left, perhaps
  back into background mode (a reset under Hold), which FREEZE's level
  hides.  A call whose reply shows that fails with `Error::Reentered`,
  since it reached a stay this did not know.  A halt with the entry
  kept confirms it with Pins, and so does the first look after a Hold,
  which keeps the halted state.  Forgetting it drops the program's SFC
  and DFC kept for restoring: a halted CPU leaves only through a reset.
  A memory call sets SFC or DFC to its function
  code, reading the program's value first; SFC/DFC reads and writes
  reach the program's value; go, call and `restore_function_codes` put
  it back.  Resync keeps it, and takes the CPU's as unknown, as after
  a write whose reply did not come: the next restore writes it.
- `mc683xx-bdm`'s `stand_in` and `bdm-stand-in`: `mc683xx-bdm-proto`'s
  `pod` over `mc683xx-bdm-model`'s `ModelBoard` on a stream socket, one
  client at a time.  The binary is a crate of its own,
  `mc683xx-bdm-stand-in`, unpublished as the model is, so
  `mc683xx-bdm` publishes without the model, which it takes for its
  tests only (a path-only dev-dependency, which a package drops).
  `bdm --pod unix:PATH` is Unix's: elsewhere (Windows) it fails, and
  `tcp:HOST:PORT` reaches a stand-in.  CI builds `bdm` on macOS and
  Windows and runs its tests on macOS; the tests use Unix sockets, so
  not on Windows.
  `bdm` and `PodTarget` run end to end against it with no hardware
  (`crates/host/tests/stand_in.rs`).  The model's FREEZE is its state, VDD
  5 V; a halt or reset enters with a breakpoint, and a reset keeps
  registers and memory.  Not the emulator: once resumed the model runs
  only NOP, MOVEQ, BRA.S, TRAP #n, ILLEGAL, BGND, MOVE/ANDI/ORI/EORI
  #imm to SR and RTE from a $0 or $2 frame (the SR writers privileged)
  from its RAM, with
  T1/T0 trace, exception frames $0 and $2 through VBR, a requested
  interrupt, and A7 as SSP or USP by SR's S (`mc683xx-bdm-model`'s `program`);
  any other instruction, an access outside RAM or 1000 instructions
  and it runs on unwatched.  Enough for `PodTarget::step`; anything
  else enters background mode by the test's hand.
- `bdm`: pins, halt, hold, berr, reset (`--run`: ResetRun), resync, bootsel, list, version, go, call, clock, regs, get, set, dump,
  write, xfer, loopback, gdb (Host, gdb), through `PodTarget` (bootsel
  and list excepted).  Halt and reset print the entry
  source.  Clock with a rate sets it (SetClock), without one shows the
  rate in effect (GetClock).  Dump and write take `--fc`, default 5 (supervisor data).
  Dump with `--output FILE` writes the raw bytes to FILE instead of a
  hex listing.
  Each run restores SFC and DFC before it exits, since the next run
  knows nothing of it, whether or not its command worked; a command
  failure and a restoring one are reported together, as a gdb
  session's and its cleanup's.  So does a Python `Pod`, on close, leaving its
  `with` block or, best effort, when collected unclosed (with a
  `ResourceWarning`, as Python's files).
- Scripting: Python bindings, `mc683xx-bdm-py` (Python module `mc683xx_bdm`),
  PyO3 over `PodTarget`, so a script keeps one pod open across calls
  and uses Python's loops and polling.  No script mode in `bdm`, which
  would be a language of its own, and no sharing of a pod between
  processes: one process opens it, as USB allows.  The layer is thin:
  each method is one `PodTarget` or `Pod` call, with the GIL released
  while it waits on the pod; register names, the protocol check and
  the pod's address (`PodAddress`, `usb:SERIAL` included) are
  `mc683xx-bdm`'s, shared with `bdm`.  The API follows the emulator's
  bindings (m683xx-emu's `cpu32-py`, module `mc683xx`)
  where the concepts match, so a script can drive either: `halt`,
  `go`, `reset_with_breakpoint`, `background`, `read_register(name)`,
  `registers()`, `bus_read`/`bus_write(address, size, ...,
  function_code=5)`, `Entry`, `BusError`, `TargetError`.  Its build is
  cpu32-py's (maturin, a `python` feature so `cargo test` needs no
  Python, abi3 from 3.10, stubs in `python/`), but in a uv project
  (AGENTS.md, Python) rather than a Makefile's venv.  `make lint` runs
  clippy with `python`, ruff and pytest against `bdm-stand-in`, mypy's
  stubtest of the stubs against the module (allowlist:
  `stubtest-allowlist.txt`, the compiled submodule) and mypy on the
  tests; mypy is a dev dependency only, as in cpu32-py.
  `Pod.serve_gdb(address="127.0.0.1:1234", ready=None, *, scratch)`
  is `bdm gdb` from a script (Host, gdb): cpu32-py's `serve_gdb`
  signature, with `scratch` added and keyword-only, since the pod has
  no native step; it returns `Ending.DETACHED` or `Ending.KILLED`.  The
  session runs with the GIL released; Python's signal handlers run
  between waits (every 20 ms waiting on gdb, every poll of a running
  target), so Ctrl-C ends it, cleaned up, and is raised.  No board
  setup in it or in `bdm`: a script does that before serving.
- Linux needs a udev rule for access without root
  (`packaging/udev/70-mc683xx-pod.rules`, README.md): uaccess for the
  user at the seat, and group plugdev, mode 0660, for a login over
  ssh, which uaccess does not reach.
- Debian package `mc683xx-bdm` (`make deb`): `bdm`, the udev rule and
  the RP2040 UF2, built by cargo-deb as deb-workflows' `rust-build-deb` builds it.
  The release workflow builds the UF2 in a job of its own and hands it
  to `rust-build-deb` as its `prebuilt-artifact`; there it is made by
  elf2uf2-rs, byte for byte picotool's UF2 (RP2040 family ID), since
  no runner image carries picotool 2.  `rust-build-deb` takes no
  dependency of its own, so on a tag the job that calls it waits for
  `gate`: a tag that fails `gate` publishes nothing anywhere.
  `bdm-stand-in` is a test aid and does not ship.
- crates.io and PyPI, beside mc683xx's crates and Python package, and
  as mc683xx publishes them (RELEASING.md): `mc683xx-bdm-proto` and
  `mc683xx-bdm` (the library and `bdm`), and the Python package
  `mc683xx-bdm` (abi3 wheels for Linux, macOS and Windows, and an
  sdist), from a `v*` tag by trusted publishing (OIDC), no stored
  token.  One version for all of them, the workspace's.
- Bootloader knock: picotool's reset interface (interface 1), not a
  pod request, so `picotool -f` works too and a pod whose protocol
  differs from `bdm`'s can still be reflashed.  `bdm bootsel` sends it
  and waits for a boot ROM device (2E8A:0003 or 000F) at a bus address
  that had none before the request, so a board already in BOOTSEL is
  not taken for the pod.  Not by serial number: the RP2350 boot ROM's
  is its OTP chip ID in hex (RP2350 datasheet 5.7.2), as the pod's, but
  the RP2040 datasheet does not say what its boot ROM's is.  Copying
  the UF2 is left to the user (or picotool).
- `mc683xx-target` (m683xx-emu, no_std, MIT OR Apache-2.0) holds
  `Cpu32Target` and the types every crate here uses: Address, Size,
  Register, SystemRegister, FunctionCode, Entry.  Memory calls name
  their function code; halting returns the entry source decoded from
  ATEMP.  From crates.io (0.7).  Its error type converts from
  `mc683xx_target::Unsupported`, which a later trait method's default
  body returns; `pod::Error::Unsupported` carries it.
- Loading and init (agreed with m683xx-emu, 2026-11-01; pending:
  `mc683xx-debug` has not landed, and neither `bdm load` nor `bdm
  init` exists yet, TODO.md): `mc683xx-debug`, in m683xx-emu (MIT OR
  Apache-2.0), over `Cpu32Target`.
  - Loading: S-records (checksums, S5/S6 counts), ELF32 big-endian
    m68k PT_LOAD segments, and raw binary at an address.  Writes are
    fc 5 by default, word writes with odd-edge bytes, an optional
    read-back verify, RPC from the entry, and a report of ranges and
    entry.
  - Init script lines: `write_memory.b|w|l`, `write_register`,
    `write_system_register`, `read_memory.w ADDRESS expect VALUE [mask
    MASK] [fc]` and `poll_memory.w ADDRESS mask MASK expect VALUE
    timeout_ms N`, in host time.
  - The pod is to add `bdm load` and `bdm init`, tested end to end
    against `bdm-stand-in`.
- gdb (agreed with m683xx-emu, 2026-11-01): one gdb remote adapter
  over `Cpu32Target`, in m683xx-emu, with `m68k:cpu32` and no FPU.  An
  optional capability trait gives the emulator ROM breakpoints,
  watchpoints and a native step; the pod has `Cpu32Target` alone.
  - Breakpoints: BGND ($4AFA) planted through memory writes, so RAM
    only on the pod.
  - Continue: go, then poll `halted()`; Ctrl-C is `halt()`.
  - Memory: fc 5, `monitor fc N` for another; `monitor reset` is
    `reset_halted`.  Init and load are not monitor commands yet.
  - `bdm gdb [--listen HOST:PORT] --scratch ADDRESS`: mc683xx-gdb
    (crates.io, default features off, so no chip or board crates)
    serving its `bdm::Bdm` over `PodTarget`, one session (`mc683xx-bdm`'s
    `gdb`, the session for `bdm` and `Pod.serve_gdb` alike).  Listens on
    127.0.0.1:1234 by default (mc683xx's `serve_gdb` default; localhost,
    since the session controls the target; port 0 picks one, named on
    standard error).  `--scratch` is required: Bdm always offers gdb a step,
    and gdb steps over a breakpoint before every continue from it.
    Detach resumes the target, kill leaves it halted; either prints
    which.  SIGINT or SIGTERM to `bdm` (signal-hook flags, asked
    between waits as Python's handlers are) ends the session as an
    error does: mc683xx-gdb halts the target and puts back every word
    BGND replaced, `bdm` puts back SFC and DFC, and exits 128 plus the
    signal, so no BGND is left in the program.  A second signal
    meanwhile exits at once.  gdb's own Ctrl-C is a byte on its
    connection, a halt for gdb, not this.  Tested end to end
    (`crates/host/tests/gdb.rs`): the `bdm`
    binary on the model, raw packets on TCP: `?`, `m`, `g`, `s`,
    `Z0`/`z0` and `c` to the breakpoint, Ctrl-C, `D` and `k`, SIGINT
    while running (the breakpoint's word, SFC and DFC back) and SIGTERM
    waiting for gdb; the same
    from Python (`crates/py/tests/test_gdb.py`), with a signal handler's
    exception waiting for gdb and while running, and raised still when
    cleaning up after it fails (the failure its `__cause__`).
  - `step()` is in `Cpu32Target` (mc683xx-target 0.7): from RPC to
    where T1 would trap, before any handler's first instruction, the
    exceptions the instruction raised and an interrupt pending taken
    (its doc comment has the contract).  `PodTarget::step` (`step.rs`)
    meets it by pointing VBR, for the step, at a table in scratch RAM
    (`PodTarget::set_scratch`, 1026 bytes the program does not use) whose every
    vector is one BGND stub after it, setting T1 (T0 clear) and GOing;
    so an exception, the trace among them, lands at the stub, and the
    step works on code in ROM.  The table is written on the first step
    after a reset, GO or CALL.  At the stub (RPC past it) it reads the
    frames from A7 down to the first not returning to the stub, then
    unwinds: the step's trace frame removed, the frame above it (an
    interrupt's) given its SR and PC; the program's trace bits back in
    the frame stacked from the program, unless that frame is the trace's
    and the instruction writes SR: then its own (MOVE to SR, RTE, STOP,
    LPSTOP: what it left; ANDI, ORI, EORI to SR: the program's bits so
    combined), read from RPC before the step; every PC still the stub made
    the program's vector for the exception below it; RPC the program's
    handler for the last one, or the traced PC.  With the program's own
    T1 set the trace frame is its own and is kept.  Then VBR back, and
    SR last.  A step that fails once VBR is changed (a refused write,
    frames that do not parse) gets VBR and SR's trace bits back, best
    effort, and reports its own error.  Stopped elsewhere (a stepped BGND, a double bus fault): VBR and
    SR's trace bits back, and the entry reported.  Not stopped in
    100 ms: halted; then RPC past the stub (it got there as the wait
    ended) is unwound as above, anything else gets VBR and the trace
    bits back and an error with RPC.  The program's T0 is not
    honored (its change-of-flow trace is lost), and what the contract
    calls implementation-defined (a VBR read or MOVE from SR in the
    step sees the pod's) is the pod's.
  - Every vector the stub, not the trace vector alone in the program's
    table (copied to RAM when in ROM): the contract stops before every
    handler, which only a table of the pod's own sees (decided
    2026-11-20, from mc683xx PLAN.md's `Cpu32Target` row).
  - Tested against `bdm-stand-in`'s model (`crates/host/tests/stand_in.rs`):
    one instruction, user mode, TRAP, an interrupt pending, BGND, no
    stop, no scratch, MOVE to SR setting and clearing T1, ORI to SR,
    RTE popping T1, a privilege violation; the unwinding itself on frames
    (`step.rs`'s tests), an interrupt above the trace among them.
    On silicon not yet.

---

## Bring-up checklist

How to run each item and what to expect: `docs/bring-up.md`.

- [x] Schematic and the rev 0.2 carrier, without a breadboard
- [ ] PIO loopback (`bdm loopback`, GP3 to GP2): scope DSCLK/DSI,
      verify 17-bit framing and idle-high.  Data checked on an
      RP2040-Zero, unscoped: 36000 words ok at 1 MHz (1.745 s) and
      4 MHz (1.297 s), 36 words a USB round trip: 1.3 ms a round trip
      at 4 MHz against 0.15 ms on the wire, so USB-limited, about
      28k words/s
- [x] Carrier, no target (rev 0.2): USB-flashed, enumerates, inputs
      low.  DS, BERR, FREEZE and RESET each read high with 5 V on
      their J1 pin, low without.  Loopback at J1 (8 to
      10, FREEZE high), unscoped: 3600 words ok at 1 MHz (0.180 s) and
      4 MHz (0.132 s).  VDD sense reads 5.344 V at 5.287 V measured
      (USB 5 V from an FTDI cable), 1.1% high.  BKPT idle at R3's
      pull-up (5.2 V).  U1 driving it, into 1 kOhm to GND, on the
      scope (TODO.md)
- [x] Reset into BDM; confirm FREEZE asserts (1 MHz DSCLK; docs/58503a/)
- [x] RSREG ATEMP first (`0x2588`): confirms RSREG's `0x2580`.  ATEMP
      $00000000 after a reset entry
- [ ] BGND and a breakpoint after GO: `bdm pins` reports a fresh ATEMP
      of 1 and 0; DSI driven again
- [ ] Silicon questions the manuals leave open (from m683xx-emu's
      model, which takes one answer for each):
  - PCC after a double bus fault out of reset: $00000001 (CPU32RM
    7.2.5.3) or $FFFFFFFF (7.2.3)
  - BKPT on a CPU in STOP or LPSTOP: enters background mode or not
  - A double bus fault with BDM enabled: whether the SIM halt monitor
    still resets
  - PCC after a reset entry, no fault: $FFFFFFFF (docs/58503a/)
- [ ] NOP / not-ready handling
- [x] RDREG D0; RSREG RPC: `bdm regs`, RPC the reset vector's PC
- [x] Read SIMCR (0xFFFA00) and other SIM regs; compare to reset
      values: after a reset entry SIMCR $60CF (FRZSW and FRZBM set),
      CSBARBT $0007, CSORBT $7B70 (docs/58503a/)
- [ ] Write chip-select init; read/write external RAM.  Chip selects
      written and read through (CSBOOT and CS1: a 512 KB flash read
      back identical to a known image; docs/58503a/); RAM not yet
- [ ] Block DUMP/FILL throughput test; raise DSCLK until it fails,
      back off.  DUMP at 1 MHz: 512 KB of longs in 18.9 s, about
      28 KB/s
- [ ] Load and run a test program from RAM
- [ ] Flash programming (target-resident algorithm)
- [ ] `Cpu32Target` shared with emulator; run smartclock harness on
      silicon

## Open questions

None.

## References

- `third_party/motorola/CPU32RM.pdf`: BDM (7.2), serial interface
  (7.2.7), command set (7.2.8)
- `third_party/motorola/MC68331UM.pdf`: BDM (5.10.2), timing (Table
  A-7), connector (Figure 5-11), SIM, reset configuration
- `third_party/motorola/SIMRM.pdf`: SIM reference, BKPT timing at reset
- `third_party/ti/`, `third_party/nexperia/`: translator and MOSFET data
  sheets
- `third_party/raspberrypi/`: RP2040, RP2350, Pico and Pico 2 data
  sheets (PIO, GPIO ratings)
- Pavel Pisa, "BDM Interface for Motorola 683xx MCU":
  https://cmp.felk.cvut.cz/~pisa/m683xx/bdm_driver.html
- m68k-bdm (Linux driver, bdm-load):
  https://git.firebee.org/Firebee/m68k-bdm
