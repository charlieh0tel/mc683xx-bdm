---
name: jlcpcb-release
description: Build the JLCPCB release package for the mc683xx-pod carrier board (Gerber/drill zip, BOM, placement, source tgz) under hardware/production/YYYYMMDD-REV/, and walk the order checklist. Use when the user asks to release, package, or order the board from JLCPCB.
---

# JLCPCB release

Read `hardware/ee_tooling/skills/jlcpcb-release/SKILL.md` first: it
says how `jlcpcb-release` works, what it writes and how to fix
rotations.  This file adds the board's specifics.  From `hardware/`:

```
uv run jlcpcb-release
```

`jlcpcb.toml` gives the board's directory, copper layers, production
directory (`hardware/production/`) and source files.

## Ordering checklist (PLAN.md, Hardware)

- 4 layers, 1.6 mm, JLC04161H-7628 stackup, ENIG.
- Assembly: top side SMT.  JLCPCB places the Pico (A1, SC0915,
  C9900019762).  J1 (C5665) is through-hole, DNP unless the order says
  otherwise; in the placement preview it needs 90 deg clockwise and
  5.08 mm south, 1.27 mm east (its zero is its body's center, ours is
  pin 1).  J2 is DNP in the design (C160403 has been short): fitted by
  hand.
- In the BOM preview, compare each matched part's MPN and package with
  the BOM's.

## Rotations and offsets

The corrections are by LCSC part, in
`hardware/ee_tooling/src/ee_tooling/parts.toml`.  The submodule follows
ee_tooling's `release/0.2` (AGENTS.md); moving it to `main` is the
owner's decision.
