"""Report on the BDM link's fast nets: for each, its pins, routed length
and layers, each via's distance to the nearest GND via, and each F.Cu
segment's crossings of B.Cu tracks with the two nearest GND vias.

Usage: uv run python fastcheck.py BOARD (KiCad closed or saved).
Coordinates are KiCad's absolute, in mm.
"""

import math
import sys
from collections.abc import Iterable, Sequence

# An (x, y) position in mm, KiCad absolute.
type Point = tuple[float, float]

# The DSCLK, DSI and DSO nets, Pico pin to J1, through U1, R1/R2 and U2.
FAST_NETS = (
    "Net-(A1-GPIO2)",
    "Net-(R1-Pad1)",
    "Net-(J1-BKPT{slash}DSCLK)",
    "Net-(A1-GPIO3)",
    "Net-(R2-Pad1)",
    "Net-(J1-IFETCH{slash}DSI)",
    "Net-(J1-IPIPE{slash}DSO)",
    "Net-(A1-GPIO6)",
)
GROUND_NET = "GND"
# GND vias reported nearest each crossing.
CROSSING_NEIGHBORS = 2


def _orientation(p: Point, q: Point, r: Point) -> float:
    return (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])


def crossing(a: Point, b: Point, c: Point, d: Point) -> Point | None:
    """Where segment a-b properly crosses segment c-d, or None (touching or
    collinear segments do not cross)."""
    if not (
        _orientation(a, b, c) * _orientation(a, b, d) < 0
        and _orientation(c, d, a) * _orientation(c, d, b) < 0
    ):
        return None
    denominator = (b[0] - a[0]) * (d[1] - c[1]) - (b[1] - a[1]) * (d[0] - c[0])
    u = ((c[0] - a[0]) * (d[1] - c[1]) - (c[1] - a[1]) * (d[0] - c[0])) / denominator
    return (a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1]))


def nearest(p: Point, points: Iterable[Point], count: int) -> list[float]:
    """The distances from p to its count nearest points, ascending."""
    return sorted(math.dist(p, q) for q in points)[:count]


def _distances(values: Sequence[float]) -> str:
    return ", ".join(f"{v:.1f}" for v in values) or "none"


def main(argv: Sequence[str]) -> int:
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    import pcbnew  # KiCad's, in the system Python only

    board = pcbnew.LoadBoard(argv[1])

    def point(v: object) -> Point:
        return (pcbnew.ToMM(v.x), pcbnew.ToMM(v.y))  # type: ignore[attr-defined]

    def pins(net: str) -> str:
        return ", ".join(
            f"{f.GetReference()}.{p.GetNumber()}"
            for f in board.GetFootprints()
            for p in f.Pads()
            if p.GetNetname() == net
        )

    tracks = list(board.GetTracks())
    vias = [t for t in tracks if isinstance(t, pcbnew.PCB_VIA)]
    segments = [t for t in tracks if not isinstance(t, pcbnew.PCB_VIA)]
    ground = [point(v.GetPosition()) for v in vias if v.GetNetname() == GROUND_NET]
    back = [s for s in segments if s.GetLayer() == pcbnew.B_Cu]
    names = {str(n) for n in board.GetNetsByName().keys()}

    for net in FAST_NETS:
        if net not in names:
            print(f"{net}: not on the board", file=sys.stderr)
            continue
        own = [s for s in segments if s.GetNetname() == net]
        length = sum(pcbnew.ToMM(s.GetLength()) for s in own)
        layers = ", ".join(sorted({s.GetLayerName() for s in own}))
        print(f"{net} [{pins(net)}]: {length:.1f} mm on {layers or 'no layer'}")
        for via in (v for v in vias if v.GetNetname() == net):
            p = point(via.GetPosition())
            print(f"  via ({p[0]:.2f}, {p[1]:.2f}): GND via {_distances(nearest(p, ground, 1))} mm")
        for front in (s for s in own if s.GetLayer() == pcbnew.F_Cu):
            for other in back:
                x = crossing(
                    point(front.GetStart()),
                    point(front.GetEnd()),
                    point(other.GetStart()),
                    point(other.GetEnd()),
                )
                if x is None:
                    continue
                print(
                    f"  crosses B.Cu {other.GetNetname()} [{pins(other.GetNetname())}]"
                    f" at ({x[0]:.1f}, {x[1]:.1f}):"
                    f" GND vias {_distances(nearest(x, ground, CROSSING_NEIGHBORS))} mm"
                )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
