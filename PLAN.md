# mc683xx-pod

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
  (github.com/charlieh0tel/mc683xx, crates/target); the pod depends on
  it by git, pinned to a rev.

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

The Pico's DSI_EN is a PIO pin as well: the program drops it 3 PIO
cycles after the last rising edge of GO's opcode or CALL's last word,
about 190 ns at 1 MHz DSCLK (PIO program), which may beat FREEZE
negating.  The firmware raises it once FREEZE asserts.

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
`in` base = DSO, side-set base = DSCLK, `set` base = DSI_EN (GP4,
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
mc683xx-pod/
├── hardware/
│   ├── mc683xx-pod/      # KiCad schematic + board
│   ├── production/       # each release as ordered
│   └── ee_tooling/      # shared KiCad tooling and house style (submodule, follows main)
├── firmware/
│   ├── bdm-model/       # a CPU32 in background mode at the serial word, and a board around it; no_std; test target
│   ├── pod-proto/       # BDM words and command sequences; USB commands; no_std
│   ├── pod-fw/          # Pico / Pico 2 firmware, embassy-rp, PIO
│   └── pod-host/        # Rust library + `bdm` CLI (nusb)
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
(`pod-proto`'s `usb`).  The answer to each request is `pod-proto`'s
`pod`, over a `Board` trait: the firmware's board (`board.rs`) is the
Pico's pins, ADC, PIO link and timeouts, so the request rules run in
host tests too.  VID:PID 1209:0001, pid.codes' test pair, until
the pod has its own.  Requests:

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
- ATEMP holds the entry source only until the first command after
  entry, and the CPU can enter on its own (BGND, a breakpoint, a double
  bus fault).  A task watches FREEZE and counts its edges (`freeze.rs`);
  before each request, if FREEZE is high and the count moved since the
  last read, the pod drives DSCLK and DSI and reads ATEMP first
  (`pod-proto`'s `entry`).  It reports it as fresh, or as stale when
  the CPU was halted when the pod started and so may have been
  commanded before.  This also turns DSI on after a self-entry: GO
  turned it off.
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

The firmware's own sizes, release builds: 26144 bytes of text on the
RP2040 and 25052 on the RP2350, 2740 bytes of statics on either.

Handle not-ready retries in firmware; report BERR and illegal-command
errors to the host.  `pod-proto`'s `session` does both, over a `Link`
trait that the PIO state machine implements.  Two sets of host tests
run it: scripted word sequences from CPU32RM's figures (`pod-proto`),
and `bdm-model`, a CPU32 in background mode that answers each word as
CPU32RM 7.2.8 says, decodes opcodes independently of `pod-proto`, and
records what a real CPU would not take: clocks while running, words
lost while busy, DUMP or FILL without READ or WRITE, GO or CALL
without the DSI release.  It is `no_std` so a second Pico can run it
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

- `pod-host`'s `transport`: a `Transport` trait, one request packet
  out and one reply back; `UsbTransport` over `nusb`, blocking, 1 s
  timeout, the first device with the pod's VID:PID; `StreamTransport`
  on a Unix or TCP socket, each message a length byte and the packet,
  for m683xx-emu's virtual BDM cable (`bdm --pod unix:PATH`).  The
  protocol is `docs/usb-protocol.md`; this repo owns it.  `bdm` sends
  Hello first and stops on another protocol version.
- `pod-host`'s `pod`: each USB request as a call.  Memory goes as
  56-byte requests and raw transfers as 15-word ones; a chunk never
  splits an access.  Tested against scripted replies.
- `pod-host`'s `target`: `PodTarget`, `Cpu32Target` over a `Pod`.  The
  entry source is the pod's ATEMP (Pins, Halt, ResetHalt): fresh
  decodes per Table 7-2, stale is `Entry::Unknown`.  It is kept until
  the target resumes.  A memory call sets SFC or DFC to its function
  code, reading the program's value first; SFC/DFC reads and writes
  reach the program's value; go, call and `restore_function_codes` put
  it back.
- `pod-host`'s `stand_in` and `bdm-stand-in`: `pod-proto`'s `pod` over
  `bdm-model`'s `ModelBoard` on a stream socket, one client at a time.
  `bdm` and `PodTarget` run end to end against it with no hardware
  (`pod-host/tests/stand_in.rs`).  The model's FREEZE is its state, VDD
  5 V; a halt or reset enters with a breakpoint, and a reset keeps
  registers and memory.  Not the emulator: no instructions run, so a
  test enters background mode itself.
- `bdm`: pins, halt, reset, go, call, clock, regs, get, set, dump,
  write, xfer, loopback, through `PodTarget`.  Halt and reset print the entry
  source.  Dump and write take `--fc`, default 5 (supervisor data).
  Dump with `--output FILE` writes the raw bytes to FILE instead of a
  hex listing.
  Each run restores SFC and DFC before it exits, since the next run
  knows nothing of it.
- Linux needs a udev rule for access without root (README.md).
- `mc683xx-target` (m683xx-emu, no_std, MIT OR Apache-2.0) holds
  `Cpu32Target` and the types every crate here uses: Address, Size,
  Register, SystemRegister, FunctionCode, Entry.  Memory calls name
  their function code; halting returns the entry source decoded from
  ATEMP.  Taken by git at a pinned rev until its crates.io release
  (0.3.0).
- Loading and init (agreed with m683xx-emu, 2026-11-01):
  `mc683xx-debug`, in m683xx-emu (MIT OR Apache-2.0), over
  `Cpu32Target`.
  - Loading: S-records (checksums, S5/S6 counts), ELF32 big-endian
    m68k PT_LOAD segments, and raw binary at an address.  Writes are
    fc 5 by default, word writes with odd-edge bytes, an optional
    read-back verify, RPC from the entry, and a report of ranges and
    entry.
  - Init script lines: `write_memory.b|w|l`, `write_register`,
    `write_system_register`, `read_memory.w ADDRESS expect VALUE [mask
    MASK] [fc]` and `poll_memory.w ADDRESS mask MASK expect VALUE
    timeout_ms N`, in host time.
  - The pod adds `bdm load` and `bdm init`, tested end to end against
    `bdm-stand-in`.
- gdb (agreed with m683xx-emu, 2026-11-01): one gdb remote adapter
  over `Cpu32Target`, in m683xx-emu, with `m68k:cpu32` and no FPU.  An
  optional capability trait gives the emulator ROM breakpoints,
  watchpoints and a native step; the pod has `Cpu32Target` alone.
  - Breakpoints: BGND ($4AFA) planted through memory writes, so RAM
    only on the pod.
  - Continue: go, then poll `halted()`; Ctrl-C is `halt()`.
  - Memory: fc 5, with `monitor` fc, reset, init and load.
  - `step()` joins `Cpu32Target`.  `PodTarget::step` sets SR's T1 and
    GOs, with the trace vector on a BGND stub in scratch RAM.  It then
    unwinds the format $2 frame and restores the vector, or VBR when
    the table had to be copied to RAM because it is in ROM (CPU32RM
    6.3.8).  So step works on code in ROM.  An illegal or privileged
    instruction is not traced: the step times out and halts, and
    reports where it stopped.
  - Its test needs a CPU that runs instructions: the integration test
    against `mc683xx-run --bdm`, not `bdm-stand-in`.

---

## Bring-up checklist

How to run each item and what to expect: `docs/bring-up.md`.

- [ ] Schematic, breadboard first (Pico + 74AHCT1G125s + 74LVC1G17s +
      BSS138BKs), then the carrier
- [ ] PIO loopback (`bdm loopback`, GP3 to GP2): scope DSCLK/DSI,
      verify 17-bit framing and idle-high.  Data checked on an
      RP2040-Zero, unscoped: 36000 words ok at 1 MHz (1.745 s) and
      4 MHz (1.297 s), 36 words a USB round trip: 1.3 ms a round trip
      at 4 MHz against 0.15 ms on the wire, so USB-limited, about
      28k words/s
- [ ] Carrier, no target (rev 0.2): USB-flashed, enumerates, inputs
      low.  DS, BERR, FREEZE and RESET each read high with 5 V on
      their J1 pin, low without.  Loopback at J1 (8 to
      10, FREEZE high), unscoped: 3600 words ok at 1 MHz (0.180 s) and
      4 MHz (0.132 s).  VDD sense reads 5.344 V at 5.287 V measured
      (USB 5 V from an FTDI cable), 1.1% high.  BKPT idle at R3's
      pull-up (5.2 V); U1 driving it needs a load (1 kOhm to GND) to show
- [ ] Reset into BDM; confirm FREEZE asserts
- [ ] RSREG ATEMP first (`0x2588`): confirms RSREG's `0x2580`
- [ ] BGND and a breakpoint after GO: `bdm pins` reports a fresh ATEMP
      of 1 and 0; DSI driven again
- [ ] Silicon questions the manuals leave open (from m683xx-emu's
      model, which takes one answer for each):
  - PCC after a double bus fault out of reset: $00000001 (CPU32RM
    7.2.5.3) or $FFFFFFFF (7.2.3)
  - BKPT on a CPU in STOP or LPSTOP: enters background mode or not
  - A double bus fault with BDM enabled: whether the SIM halt monitor
    still resets
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
- A VID:PID of the pod's own: Raspberry Pi's PID program under VID
  0x2E8A, for RP2040 and RP2350 products (github.com/raspberrypi/usb-pid,
  request form there); the owner applies.  pid.codes needs a public
  open-source project.

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
