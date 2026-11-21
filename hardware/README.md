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

`fastcheck.py` reports on the BDM link's fast nets
(`uv run python fastcheck.py mc683xx-pod/mc683xx-pod.kicad_pcb`).
`make render` (repository root) rebuilds `docs/images/mc683xx-pod-top.png`
from the board file.
