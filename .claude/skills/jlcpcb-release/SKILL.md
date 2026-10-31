---
name: jlcpcb-release
description: Build the JLCPCB release package for the m683xx-pod carrier board (Gerber/drill zip, BOM, placement, source tgz) under production/YYYYMMDD-REV/, and walk the order checklist. Use when the user asks to release, package, or order the board from JLCPCB.
---

# JLCPCB release

`hardware/release.py` builds the package.  Run it from `hardware/`:

```
uv run python release.py            # today's date, REV from the board title block
uv run python release.py --date 20261027 --force   # rebuild a given day's release
```

`PROJECTS` in `release.py` gives the board's copper layers, production
directory and source files.  `ROTATION_FIX` and `OFFSET_FIX` are by LCSC
part, so they hold for every board.

It stops unless ERC is clean, DRC (with schematic parity) is clean, and
`hardware/` has no uncommitted changes (`--allow-dirty` only for a trial;
the tgz's COMMIT file then says `-dirty`).

## Before running

1. The board's title-block revision is the release's REV: bump it in the
   board and schematic first if this is a new revision.
2. Commit everything in `hardware/`.

## What it writes

`production/YYYYMMDD-REV/`, NAME being the project:

| File | Use |
|---|---|
| `NAME-REV-jlcpcb.zip` | JLCPCB PCB order: Gerbers (each copper layer, mask, paste, silk, Edge.Cuts; Protel extensions, no X2, soldermask subtracted from silk) and Excellon drill in mm, PTH and NPTH separate |
| `NAME-REV-bom.csv` | assembly BOM: Comment, Designator, Footprint, LCSC Part #; not-fitted parts left out |
| `NAME-REV-cpl.csv` | assembly placement: Designator, Val, Package, Mid X, Mid Y, Rotation, Layer; only parts in the BOM |
| `NAME-REV-source.tgz` | the design files, footprint and 3D libraries, PLAN.md, docs/, schematic and board PDFs, and COMMIT |

Commit `production/` after the order is placed, with any rotation fixes
found on the way (below).

## Ordering checklist

- 2 layers, 1.6 mm (PLAN.md, Hardware).
- Assembly: top side SMT.  The Pico's sockets and the BDM header are
  through-hole: DNP, fitted by hand, unless the order says otherwise.
- In the BOM preview, compare each matched part's MPN and package with
  the BOM's.

## Rotations and offsets

JLCPCB draws each LCSC part from its own model, whose zero can differ from
KiCad's footprint, so corrections go by LCSC part (every placement of a
part shares one).  In JLCPCB's placement preview, compare each part's
magenta pin-1 dot with our pin 1 (silk mark or pad 1):

- turned: add the LCSC part and degrees to `ROTATION_FIX` in
  `release.py` (JLCPCB's dot 90 deg counter-clockwise from ours: -90);
- shifted: add the LCSC part and the shift in mm to `OFFSET_FIX`, in
  the part's own frame (KiCad rotation 0, y up);

then commit and rebuild with `--force`.  A part not yet checked goes in
`UNCHECKED`; `release.py` names it at each build until it is.
