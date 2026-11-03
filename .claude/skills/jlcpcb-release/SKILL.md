---
name: jlcpcb-release
description: Build the JLCPCB release package for the mc683xx-pod carrier board (Gerber/drill zip, BOM, placement, source tgz) under production/YYYYMMDD-REV/, and walk the order checklist. Use when the user asks to release, package, or order the board from JLCPCB.
---

# JLCPCB release

Read `hardware/ee_tooling/skills/jlcpcb-release/SKILL.md` first: it
says how `jlcpcb-release` works, what it writes and how to fix
rotations.  This file adds the board's specifics.  From `hardware/`:

```
uv run jlcpcb-release
```

`jlcpcb.toml` gives the board's directory, copper layers, production
directory (`production/`) and source files.

## Ordering checklist (PLAN.md, Hardware)

- 4 layers, 1.6 mm, JLC04161H-7628 stackup, ENIG.
- Assembly: top side SMT.  The Pico (A1) and the BDM header (J1) are
  fitted by hand: A1 is out of the BOM; J1 is through-hole, DNP unless
  the order says otherwise.
- In the BOM preview, compare each matched part's MPN and package with
  the BOM's.

## Rotations and offsets

The corrections are by LCSC part, in
`hardware/ee_tooling/src/ee_tooling/parts.toml`; a new part's goes there
(in ee_tooling, then the submodule updated here).
