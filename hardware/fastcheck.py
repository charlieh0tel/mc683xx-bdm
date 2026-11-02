"""Report on the BDM link's fast nets, those in the Fast net class: for
each, its pins, routed length and layers, and each via's distance to the
nearest GND via.  On a board with inner layers, the plane under each
segment (the next copper layer in), as runs of the zone nets found there,
"none" where it has no fill; on a two-layer board, each F.Cu segment's
crossings of B.Cu tracks with the two nearest GND vias.

Usage: uv run python fastcheck.py BOARD (KiCad closed or saved).
Coordinates are KiCad's absolute, in mm.
"""

import math
import sys
from collections.abc import Iterable, Sequence

# An (x, y) position in mm, KiCad absolute.
type Point = tuple[float, float]

FAST_CLASS = "Fast"
GROUND_NET = "GND"
# GND vias reported nearest each crossing.
CROSSING_NEIGHBORS = 2
# Distance between the points sampled along a segment for its plane, mm.
SAMPLE_MM = 0.25


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


def runs(samples: Sequence[str | None]) -> list[tuple[str | None, int]]:
    """Consecutive equal samples, as (value, count) in order."""
    out: list[tuple[str | None, int]] = []
    for value in samples:
        if out and out[-1][0] == value:
            out[-1] = (value, out[-1][1] + 1)
        else:
            out.append((value, 1))
    return out


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

    copper = board.GetCopperLayerCount()
    # The plane under each outer layer: the next copper layer in.
    planes = (
        {pcbnew.F_Cu: pcbnew.In1_Cu, pcbnew.B_Cu: board.GetLayerID(f"In{copper - 2}.Cu")}
        if copper >= 4
        else {}
    )
    zones = list(board.Zones())

    def under(layer: int, at: Point) -> str | None:
        """The net of the zone fill on layer at a point, or None."""
        v = pcbnew.VECTOR2I_MM(*at)
        return next(
            (z.GetNetname() for z in zones if z.IsOnLayer(layer) and z.HitTestFilledArea(layer, v)),
            None,
        )

    tracks = list(board.GetTracks())
    vias = [t for t in tracks if isinstance(t, pcbnew.PCB_VIA)]
    segments = [t for t in tracks if not isinstance(t, pcbnew.PCB_VIA)]
    ground = [point(v.GetPosition()) for v in vias if v.GetNetname() == GROUND_NET]
    back = [s for s in segments if s.GetLayer() == pcbnew.B_Cu]
    fast = sorted(
        str(name)
        for name, net in board.GetNetsByName().items()
        if net.GetNetClassName() == FAST_CLASS
    )
    if not fast:
        print(f"no nets in the {FAST_CLASS} net class", file=sys.stderr)
        return 1

    for net in fast:
        own = [s for s in segments if s.GetNetname() == net]
        length = sum(pcbnew.ToMM(s.GetLength()) for s in own)
        layers = ", ".join(sorted({s.GetLayerName() for s in own}))
        print(f"{net} [{pins(net)}]: {length:.1f} mm on {layers or 'no layer'}")
        for via in (v for v in vias if v.GetNetname() == net):
            p = point(via.GetPosition())
            print(f"  via ({p[0]:.2f}, {p[1]:.2f}): GND via {_distances(nearest(p, ground, 1))} mm")
        for segment in own if planes else ():
            a, b = point(segment.GetStart()), point(segment.GetEnd())
            steps = max(1, round(math.dist(a, b) / SAMPLE_MM))
            layer = planes.get(segment.GetLayer())
            if layer is None:
                continue
            seen = [
                under(layer, (a[0] + (b[0] - a[0]) * k / steps, a[1] + (b[1] - a[1]) * k / steps))
                for k in range(steps + 1)
            ]
            print(
                f"  {segment.GetLayerName()} ({a[0]:.2f}, {a[1]:.2f})-({b[0]:.2f}, {b[1]:.2f})"
                f" over {board.GetLayerName(layer)}: "
                + ", then ".join(f"{name or 'none'}" for name, _ in runs(seen))
            )
        for front in (s for s in own if s.GetLayer() == pcbnew.F_Cu and not planes):
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
