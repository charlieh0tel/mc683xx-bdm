"""Build a JLCPCB release package for a board.

Writes the project's production directory (PROJECTS), YYYYMMDD-REV/, with:
  NAME-REV-jlcpcb.zip   Gerbers and drill files, for the PCB order
  NAME-REV-bom.csv      BOM in JLCPCB's columns, for assembly
  NAME-REV-cpl.csv      placement in JLCPCB's columns, for assembly
  NAME-REV-source.tgz   the design files, PDFs and docs it was made from

NAME is the KiCad project, REV its board's title-block revision.  ERC and
DRC must be clean and hardware/ committed, unless --allow-dirty.
"""

import argparse
import csv
import datetime
import io
import math
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent

# Every board's non-copper Gerber layers.
OTHER_LAYERS = [
    "F.Mask",
    "B.Mask",
    "F.Paste",
    "B.Paste",
    "F.Silkscreen",
    "B.Silkscreen",
    "Edge.Cuts",
]

# JLCPCB draws each LCSC part from its own model, whose zero can differ from
# KiCad's footprint.  Degrees added to KiCad's rotation, by LCSC part, from
# JLCPCB's placement preview (pin-1 dot against our pin 1).  Found on
# oncore-emulator's first order, 2026-10-26; they hold for any board.
ROTATION_FIX: dict[str, float] = {
    "C51118": -90,  # AP2112K-3.3
    "C163712": 180,  # SN74AHCT1G126
    "C840096": 180,  # SN74LVC1G34
    "C7519": -90,  # USBLC6-2SC6
    "C82045": 180,  # BSS138
}

# Where JLCPCB's model sits off our footprint: mm added to the position, in
# the part's own frame (KiCad rotation 0, y up), by LCSC part.
# Parts whose correction JLCPCB's preview has not yet given: checked and
# moved to ROTATION_FIX at their first order.
UNCHECKED: dict[str, str] = {}

OFFSET_FIX: dict[str, tuple[float, float]] = {
    "C165948": (0.0, 1.5),  # HRO TYPE-C-31-M-12: back 1.5 mm from its face
}


@dataclass(frozen=True)
class Project:
    """A board that can be released."""

    name: str  # the KiCad project, and its directory under hardware/
    copper: tuple[str, ...]  # copper layers, top to bottom
    production: Path  # where its YYYYMMDD-REV/ releases go
    sources: tuple[str, ...]  # files and directories, from the repo root, for the source tgz

    @property
    def directory(self) -> Path:
        return HERE / self.name

    @property
    def pcb(self) -> Path:
        return self.directory / f"{self.name}.kicad_pcb"

    @property
    def sch(self) -> Path:
        return self.directory / f"{self.name}.kicad_sch"

    @property
    def layers(self) -> list[str]:
        return [*self.copper, *OTHER_LAYERS]


def project_sources(name: str, *extra: str) -> tuple[str, ...]:
    """A KiCad project's design files and libraries, then extra."""
    d = f"hardware/{name}"
    files = (".kicad_pcb", ".kicad_sch", ".kicad_pro", ".kicad_dru", ".pretty", ".3dshapes")
    return (
        *(f"{d}/{name}{f}" for f in files),
        f"{d}/fp-lib-table",
        f"{d}/sym-lib-table",
        *extra,
        "PLAN.md",
        "docs",
    )


PROJECTS = {
    p.name: p
    for p in (
        Project(
            "m683xx-pod",
            ("F.Cu", "B.Cu"),
            ROOT / "production",
            project_sources("m683xx-pod", "hardware/m683xx-pod/m683xx-pod.kicad_sym"),
        ),
    )
}


def run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, cwd=HERE, capture_output=True, text=True, check=False)


def kicad(*args: str) -> None:
    r = run("kicad-cli", *args)
    if r.returncode != 0:
        sys.exit(f"kicad-cli {' '.join(args)} failed:\n{r.stdout}{r.stderr}")


def revision(p: Project) -> str:
    m = re.search(r'\(title_block\s.*?\(rev "([^"]+)"\)', p.pcb.read_text(), re.S)
    if not m:
        sys.exit(f"no revision in {p.pcb.name}'s title block")
    return m.group(1)


def check(p: Project, work: Path, allow_dirty: bool) -> str:
    """ERC, DRC with schematic parity, and a clean hardware/; return the commit."""
    r = run(
        "kicad-cli", "sch", "erc", "--exit-code-violations", "-o", str(work / "erc.rpt"), str(p.sch)
    )
    if r.returncode != 0:
        sys.exit(f"ERC is not clean: see {work / 'erc.rpt'}")
    r = run(
        "kicad-cli",
        "pcb",
        "drc",
        "--schematic-parity",
        "--exit-code-violations",
        "-o",
        str(work / "drc.rpt"),
        str(p.pcb),
    )
    if r.returncode != 0:
        sys.exit(f"DRC is not clean: see {work / 'drc.rpt'}")
    status = run("git", "status", "--porcelain", "--", ".").stdout.strip()
    if status and not allow_dirty:
        sys.exit(f"hardware/ has uncommitted changes:\n{status}")
    commit = run("git", "rev-parse", "--short", "HEAD").stdout.strip()
    return commit + ("-dirty" if status else "")


def gerbers(p: Project, work: Path, out: Path) -> None:
    g = work / "gerbers"
    g.mkdir()
    kicad(
        "pcb",
        "export",
        "gerbers",
        "--layers",
        ",".join(p.layers),
        "--no-x2",
        "--subtract-soldermask",
        "--check-zones",
        "-o",
        str(g),
        str(p.pcb),
    )
    kicad(
        "pcb",
        "export",
        "drill",
        "--format",
        "excellon",
        "--excellon-units",
        "mm",
        "--excellon-zeros-format",
        "decimal",
        "--excellon-oval-format",
        "alternate",
        "--excellon-separate-th",
        "--drill-origin",
        "absolute",
        "-o",
        f"{g}/",
        str(p.pcb),
    )
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for f in sorted(g.iterdir()):
            z.write(f, f.name)


def bom(p: Project, work: Path, out: Path) -> dict[str, str]:
    """JLCPCB BOM, without not-fitted parts; return its designators' LCSC parts."""
    raw = work / "bom.csv"
    kicad(
        "sch",
        "export",
        "bom",
        "--exclude-dnp",
        "--fields",
        "Value,Reference,Footprint,LCSC,${QUANTITY}",
        "--labels",
        "Comment,Designator,Footprint,LCSC Part #,Qty",
        "--group-by",
        "Value,Footprint,LCSC",
        "--ref-range-delimiter",
        "",
        "-o",
        str(raw),
        str(p.sch),
    )
    rows = list(csv.DictReader(io.StringIO(raw.read_text())))
    refs: dict[str, str] = {}
    missing = []
    with out.open("w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["Comment", "Designator", "Footprint", "LCSC Part #"])
        for r in rows:
            fp = r["Footprint"].split(":", 1)[-1]
            w.writerow([r["Comment"], r["Designator"], fp, r["LCSC Part #"]])
            for x in r["Designator"].split(","):
                refs[x.strip()] = r["LCSC Part #"]
            if not r["LCSC Part #"]:
                missing.append(r["Designator"])
    if missing:
        print("warning: no LCSC part number for", ", ".join(missing))
    return refs


def cpl(p: Project, work: Path, out: Path, refs: dict[str, str]) -> None:
    """JLCPCB placement for the parts in the BOM."""
    raw = work / "pos.csv"
    kicad(
        "pcb",
        "export",
        "pos",
        "--format",
        "csv",
        "--units",
        "mm",
        "--side",
        "both",
        "--exclude-dnp",
        "-o",
        str(raw),
        str(p.pcb),
    )
    rows = list(csv.DictReader(io.StringIO(raw.read_text())))
    placed = set()
    with out.open("w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["Designator", "Val", "Package", "Mid X", "Mid Y", "Rotation", "Layer"])
        for r in rows:
            if r["Ref"] not in refs:
                continue
            lcsc = refs[r["Ref"]]
            kicad_rot = float(r["Rot"])
            ox, oy = OFFSET_FIX.get(lcsc, (0.0, 0.0))
            t = math.radians(kicad_rot)
            x = float(r["PosX"]) + ox * math.cos(t) - oy * math.sin(t)
            y = float(r["PosY"]) + ox * math.sin(t) + oy * math.cos(t)
            rot = (kicad_rot + ROTATION_FIX.get(lcsc, 0.0)) % 360
            layer = "Top" if r["Side"].lower() == "top" else "Bottom"
            w.writerow(
                [
                    r["Ref"],
                    r["Val"],
                    r["Package"],
                    f"{x:.4f}mm",
                    f"{y:.4f}mm",
                    f"{rot:g}",
                    layer,
                ]
            )
            placed.add(r["Ref"])
    for lcsc, part in UNCHECKED.items():
        if lcsc in refs.values():
            print(f"check {part} ({lcsc}) in JLCPCB's placement preview: no rotation known yet")
    if set(refs) - placed:
        sys.exit(f"in the BOM but not placed: {', '.join(sorted(set(refs) - placed))}")


def source(p: Project, work: Path, out: Path, commit: str) -> None:
    pdf = work / "pdf"
    pdf.mkdir()
    kicad("sch", "export", "pdf", "-o", str(pdf / f"{p.name}-schematic.pdf"), str(p.sch))
    kicad(
        "pcb",
        "export",
        "pdf",
        "--mode-multipage",
        "--layers",
        ",".join([*p.copper, "F.Silkscreen", "B.Silkscreen", "F.Fab", "B.Fab"]),
        "--common-layers",
        "Edge.Cuts",
        "-o",
        str(pdf / f"{p.name}-board.pdf"),
        str(p.pcb),
    )
    with tarfile.open(out, "w:gz") as t:
        for s in p.sources:
            t.add(ROOT / s, arcname=f"{out.stem.removesuffix('.tar')}/{s}")
        for f in sorted(pdf.iterdir()):
            t.add(f, arcname=f"{out.stem.removesuffix('.tar')}/pdf/{f.name}")
        info = f"commit {commit}\nbuilt {datetime.datetime.now().isoformat(timespec='seconds')}\n"
        ti = tarfile.TarInfo(f"{out.stem.removesuffix('.tar')}/COMMIT")
        ti.size = len(info)
        t.addfile(ti, io.BytesIO(info.encode()))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("project", nargs="?", choices=PROJECTS, default="m683xx-pod")
    ap.add_argument("--date", default=datetime.date.today().strftime("%Y%m%d"))
    ap.add_argument("--force", action="store_true", help="replace an existing release")
    ap.add_argument("--allow-dirty", action="store_true", help="allow uncommitted hardware/")
    a = ap.parse_args()

    p = PROJECTS[a.project]
    rev = revision(p)
    dest = p.production / f"{a.date}-{rev}"
    if dest.exists():
        if not a.force:
            sys.exit(f"{dest} exists; use --force to replace it")
        shutil.rmtree(dest)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        commit = check(p, work, a.allow_dirty)
        dest.mkdir(parents=True)
        base = f"{p.name}-{rev}"
        gerbers(p, work, dest / f"{base}-jlcpcb.zip")
        refs = bom(p, work, dest / f"{base}-bom.csv")
        cpl(p, work, dest / f"{base}-cpl.csv", refs)
        source(p, work, dest / f"{base}-source.tgz", commit)
    print(f"{dest.relative_to(ROOT)} from {commit}:")
    for f in sorted(dest.iterdir()):
        print(f"  {f.name}")


if __name__ == "__main__":
    main()
