"""Run every codec over `.difr` payloads and rank them by the ``M`` metric.

Each image is benchmarked on its own (one table per image); the harness then
rolls those per-image results up into one aggregate per directory, recursively,
so a whole tree like ``testdata/`` yields a stat block for every subdirectory.
"""

from __future__ import annotations

import os
import statistics
from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from dif_tools import image_to_dif_image

from .codecs import all_codecs
from .metric import compute_m, memcpy_speed, speed


@dataclass
class CodecResult:
    """One codec on one image."""

    name: str
    ratio_s: float
    comp_mbps: float
    decomp_mbps: float
    c: float
    d: float
    m: float
    available: bool
    note: str = ""


@dataclass
class ImageReport:
    """Every codec's result on a single image."""

    path: str
    difr_bytes: int
    memcpy_mbps: float
    results: list[CodecResult]


@dataclass
class DirStat:
    """One codec aggregated over every image under a directory."""

    name: str
    n: int
    ratio_s: float
    c: float
    d: float
    m_mean: float
    m_std: float
    note: str = ""


def bench_image(path: str, raw: bytes, repeats: int = 5) -> ImageReport:
    mem = memcpy_speed(raw, repeats)
    results: list[CodecResult] = []
    for codec in all_codecs():
        if not codec.available:
            results.append(
                CodecResult(codec.name, 0, 0, 0, 0, 0, float("-inf"), False, codec.note)
            )
            continue
        try:
            comp = codec.compress(raw)
            if codec.decompress(comp, len(raw)) != raw:
                raise ValueError("roundtrip mismatch")
            csp = speed(lambda: codec.compress(raw), len(raw), repeats)
            dsp = speed(lambda: codec.decompress(comp, len(raw)), len(raw), repeats)
            s = len(raw) / len(comp)
            c, d = mem / csp, mem / dsp
            results.append(
                CodecResult(
                    codec.name,
                    s,
                    csp / 1e6,
                    dsp / 1e6,
                    c,
                    d,
                    compute_m(s, c, d),
                    True,
                    codec.note,
                )
            )
        except Exception:  # noqa: BLE001
            results.append(
                CodecResult(
                    codec.name, 0, 0, 0, 0, 0, float("-inf"), False, "roundtrip failed"
                )
            )
    results.sort(key=lambda r: r.m, reverse=True)
    return ImageReport(path, len(raw), mem / 1e6, results)


def run(
    paths: Sequence[str | Path], strategy: str = "arithmetic", repeats: int = 5
) -> list[ImageReport]:
    reports: list[ImageReport] = []
    for p in paths:
        raw = image_to_dif_image(p, strategy=strategy).to_difr()
        reports.append(bench_image(str(p), raw, repeats))
    return reports


def _aggregate(reps: Sequence[ImageReport]) -> list[DirStat]:
    by_codec: dict[str, list[CodecResult]] = defaultdict(list)
    notes: dict[str, str] = {}
    for rep in reps:
        for r in rep.results:
            if r.available:
                by_codec[r.name].append(r)
                notes[r.name] = r.note
    stats: list[DirStat] = []
    for name, rs in by_codec.items():
        ms = [r.m for r in rs]
        stats.append(
            DirStat(
                name,
                len(rs),
                statistics.mean(r.ratio_s for r in rs),
                statistics.mean(r.c for r in rs),
                statistics.mean(r.d for r in rs),
                statistics.mean(ms),
                statistics.pstdev(ms) if len(ms) > 1 else 0.0,
                notes[name],
            )
        )
    stats.sort(key=lambda s: s.m_mean, reverse=True)
    return stats


def subdir_stats(
    reports: Sequence[ImageReport],
) -> list[tuple[str, list[DirStat]]]:
    """Aggregate per directory, recursively: every ancestor dir (down to the
    common root of the inputs) gets a stat block covering all images beneath it.
    Returned outermost-first.
    """
    if not reports:
        return []
    paths = [Path(r.path).resolve() for r in reports]
    root = Path(os.path.commonpath([str(p.parent) for p in paths]))
    buckets: dict[Path, list[ImageReport]] = defaultdict(list)
    for rep, p in zip(reports, paths):
        d = p.parent
        while True:
            buckets[d].append(rep)
            if d == root:
                break
            d = d.parent
    base = root.parent  # so the root itself shows by name, children as root/sub
    out: list[tuple[str, list[DirStat]]] = []
    for d in sorted(buckets, key=lambda p: (len(p.parts), str(p))):
        out.append((os.path.relpath(d, base), _aggregate(buckets[d])))
    return out


def format_table(results: list[CodecResult]) -> str:
    head = (
        f"{'codec':<14}{'ratio S':>9}{'comp MB/s':>11}{'decomp MB/s':>13}"
        f"{'C':>12}{'D':>12}{'M':>9}  note"
    )
    lines = [head, "-" * len(head)]
    for r in results:
        if r.available:
            lines.append(
                f"{r.name:<14}{r.ratio_s:>9.3f}{r.comp_mbps:>11.1f}{r.decomp_mbps:>13.1f}"
                f"{r.c:>12.1f}{r.d:>12.1f}{r.m:>9.3f}  {r.note}"
            )
        else:
            lines.append(
                f"{r.name:<14}{'unavailable':>9}{'':>11}{'':>13}{'':>12}{'':>12}{'':>9}  {r.note}"
            )
    return "\n".join(lines)


TSV_HEADER = (
    "image",
    "difr_bytes",
    "memcpy_mbps",
    "codec",
    "ratio_s",
    "comp_mbps",
    "decomp_mbps",
    "C",
    "D",
    "M",
    "ok",
    "note",
)


def iter_rows(reports: Sequence[ImageReport]):
    """Yield one flat row per (image, codec) for CSV/TSV export."""
    for rep in reports:
        for r in rep.results:
            yield (
                rep.path,
                rep.difr_bytes,
                f"{rep.memcpy_mbps:.1f}",
                r.name,
                f"{r.ratio_s:.4f}",
                f"{r.comp_mbps:.2f}",
                f"{r.decomp_mbps:.2f}",
                f"{r.c:.2f}",
                f"{r.d:.2f}",
                f"{r.m:.4f}",
                int(r.available),
                r.note,
            )


def format_stats_table(stats: list[DirStat]) -> str:
    """Aggregate block as a GitHub-flavored markdown table."""
    rows = [
        "| codec | n | ratio S | C | D | M mean | M std | note |",
        "|---|--:|--:|--:|--:|--:|--:|---|",
    ]
    for s in stats:
        rows.append(
            f"| {s.name} | {s.n} | {s.ratio_s:.3f} | {s.c:.1f} | {s.d:.1f} "
            f"| {s.m_mean:.3f} | {s.m_std:.3f} | {s.note} |"
        )
    return "\n".join(rows)
