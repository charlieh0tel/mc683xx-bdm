# Hardware

`mc683xx-pod/` is the carrier board's KiCad project.  `production/`
holds each release as fabricated; it is not changed afterwards.

## Revisions

| Rev | Release | PCB | Assembly |
|---|---|---|---|
| 0.1 | `production/20261102-0.1/` | as 0.2 | A1 (Pico) not in the BOM, hand-soldered; J2 (SWD) fitted |
| 0.2 | `production/20261102-0.2/` | as 0.1 | A1 placed by JLCPCB (LCSC C9900019762); J2 not fitted |

0.1 and 0.2 are one PCB: their Gerbers and drill files match, apart
from creation dates.  The revision differs only in the BOM, the
placement file and the title blocks.  The silkscreen of both reads
"mc683xx-pod rev 0.1", since the silkscreen text was not bumped for
0.2.  A board with a Pico but no J2 is a 0.2.

## Next revision

Changes for the carrier after 0.2.  None is needed for 0.2 to work;
each fixes something seen on the bench.

1. BKPT released whatever the Pico does.  A Pico reset or power loss
   while U1 is enabled can glitch BKPT (`docs/bring-up.md`, step 5):
   GP9, U1's input, falls through R15 and the RP2040's pull-down in
   about 0.3 us, before Q3's gate, through its 100k, turns U1 off in
   about 1-2 us; U1 drives BKPT low, then R3 pulls it high, one DSCLK
   clock to a halted CPU.  The firmware keeps U1 off while idle, which
   leaves only a reset mid-request or during `hold`.  Fix: drive U1's
   input through an inverting stage like Q3's, a BSS138BK with its
   gate on GP9 (100k gate pull-down) and its drain on U1's input with
   a 2.2k pull-up to target VDD; R15 goes.  Every state that leaves
   GP9 undriven or low (reset, boot, power loss, unpowered) then holds
   U1's input high, BKPT released, whatever U1's enable does.  The
   firmware inverts GP9 (its pad's output override).  Check: the
   stage's edges at 4 MHz DSCLK (about 20 ns into U1's input and the
   FET's output capacitance) and 2.3 mA from VDD while DSCLK is low.
2. Silkscreen revision text bumped with the title blocks
   (`AGENTS.md`, Hardware).
3. Status LEDs on the carrier, visible with the Pico mounted (a pod
   in a case may hide the Pico's GP25 LED, lit while U1 is on).
   Required: target VDD, lit while the target is powered.  Either
   wired straight from target VDD (works with the pod unpowered; loads
   target VDD a few mA) or driven by the firmware from the VDD sense
   reading (no load on the target; needs a spare GPIO).  Optional, on
   one or two more LEDs (mono, bicolor or tricolor, not decided),
   driven by the firmware from spare GPIOs: target halted (FREEZE
   high) or running (FREEZE low, also with no target); safe or not
   safe to remove (U1 off or on).  Pins: one GPIO per LED or color,
   from the spare GPIOs (GP0, GP1, GP13-GP22, GP27, GP28; PLAN.md, pin
   table).  With item 1's inverting stage a replug is always safe, so
   that indication could show activity or errors instead.

`fastcheck.py` reports on the BDM link's fast nets
(`uv run python fastcheck.py mc683xx-pod/mc683xx-pod.kicad_pcb`).
`make render` (repository root) rebuilds `docs/images/mc683xx-pod-top.png`
from the board file.
