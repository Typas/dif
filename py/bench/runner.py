"""Run every codec over `.difr` payloads and rank them by the ``M`` metric.

Each image is benchmarked on its own (one table per image); the harness then
rolls those per-image results up into one aggregate per directory, recursively,
so a whole tree like ``data/testdata/`` yields a stat block for every subdirectory.
"""

from __future__ import annotations

import os
import statistics
from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from dif_tools import image_to_dif_image

from .codecs import Codec, all_codecs, select_codecs
from .metric import compute_m, memcpy_speed, peak_rss, speed


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
    peak_mb: float = 0.0
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
    peak_mb: float = 0.0
    note: str = ""


def bench_image(
    path: str,
    raw: bytes,
    repeats: int = 5,
    num_threads: int = 1,
    codecs: list[Codec] | None = None,
) -> ImageReport:
    codec_w = 12
    ratio_w = 7
    speed_w = 14
    score_w = 7
    peak_w = 9
    MB: int = 1048576
    print(
        f"| {'codec':^{codec_w}} | {'S ratio':^{ratio_w}} | {'C speed (Mbps)':^{speed_w}} | {'D speed (Mbps)':^{speed_w}} | {'M score':^{score_w}} | {'peak MB':^{peak_w}} |"
    )
    print(
        f"|{'-' * (codec_w + 2)}|{'-' * (ratio_w + 2)}|{'-' * (speed_w + 2)}|{'-' * (speed_w + 2)}|{'-' * (score_w + 2)}|{'-' * (peak_w + 2)}|"
    )
    mem = memcpy_speed(raw, repeats)
    print(
        f"| {'memcpy':^{codec_w}} | {1.0:>{ratio_w}.1f} | {mem / MB:>{speed_w}.1f} | {mem / MB:>{speed_w}.1f} | {compute_m(1, 1, 1):>{score_w}.1f} | {'':>{peak_w}} |"
    )
    results: list[CodecResult] = []
    # Standalone algorithms only (no DIF container). `num_threads > 1` selects each
    # codec's multithreaded encoder where it has one (else single-thread).
    if codecs is None:
        codecs = all_codecs(num_threads)
    for codec in codecs:
        if not codec.available:
            print(f"-- WARN: {codec.name} is not available, skipped")
            results.append(
                CodecResult(
                    codec.name, 0, 0, 0, 0, 0, float("-inf"), False, 0.0, codec.note
                )
            )
            continue
        try:
            # The codec normally won't fail, don't try every time. Peak RSS is
            # sampled over a single compress+decompress so it captures the codec's
            # working set (incl. native allocations) as a delta over baseline.
            with peak_rss() as pk:
                comp = codec.compress(raw)
                codec.decompress(comp, len(raw))
            peak_mb = pk.delta / MB
            csp, comp = speed(lambda: codec.compress(raw), len(raw), repeats)
            dsp, decomp = speed(
                lambda: codec.decompress(comp, len(raw)), len(raw), repeats
            )
            if decomp != raw:
                raise ValueError("roundtrip mismatch")
            s = len(raw) / len(comp)
            c, d = mem / csp, mem / dsp
            m = compute_m(s, c, d)
            print(
                f"| {codec.name:^{codec_w}} | {s:>{ratio_w}.1f} | {csp / MB:>{speed_w}.1f} | {dsp / MB:>{speed_w}.1f} | {m:>{score_w}.1f} | {peak_mb:>{peak_w}.1f} |"
            )
            results.append(
                CodecResult(
                    codec.name,
                    s,
                    csp / MB,
                    dsp / MB,
                    c,
                    d,
                    m,
                    True,
                    peak_mb,
                    codec.note,
                )
            )
        except Exception:  # noqa: BLE001
            print(f"-- ERROR: {codec.name} has roundtrip failed, skipped")
            results.append(
                CodecResult(
                    codec.name,
                    0,
                    0,
                    0,
                    0,
                    0,
                    float("-inf"),
                    False,
                    0.0,
                    "roundtrip failed",
                )
            )
    results.sort(key=lambda r: r.m, reverse=True)
    return ImageReport(path, len(raw), mem / 1e6, results)


def run(
    paths: Sequence[str | Path],
    strategy: str = "arithmetic",
    repeats: int = 5,
    num_threads: int = 1,
    codec_specs: list[str] | None = None,
) -> list[ImageReport]:
    # Resolve the codec selection once (same lzbench `-e` syntax as `--outer-codecs`);
    # None = the whole registry. Raises ValueError on an unknown codec token.
    codecs = select_codecs(codec_specs, num_threads)
    reports: list[ImageReport] = []
    count = len(paths)
    for i, p in enumerate(paths):
        print(f"Benchmarking {p} ({i + 1}/{count}):")
        raw = image_to_dif_image(p, strategy=strategy).to_difr()
        reports.append(bench_image(str(p), raw, repeats, num_threads, codecs))
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
                statistics.mean(r.peak_mb for r in rs),
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
        f"{'C':>12}{'D':>12}{'M':>9}{'peak MB':>10}  note"
    )
    lines = [head, "-" * len(head)]
    for r in results:
        if r.available:
            lines.append(
                f"{r.name:<14}{r.ratio_s:>9.3f}{r.comp_mbps:>11.1f}{r.decomp_mbps:>13.1f}"
                f"{r.c:>12.1f}{r.d:>12.1f}{r.m:>9.3f}{r.peak_mb:>10.1f}  {r.note}"
            )
        else:
            lines.append(
                f"{r.name:<14}{'unavailable':>9}{'':>11}{'':>13}{'':>12}{'':>12}{'':>9}{'':>10}  {r.note}"
            )
    return "\n".join(lines)


TSV_HEADER = (
    "image",
    "codec",
    "bytes",
    "ratio_s",
    "comp_mbps",
    "decomp_mbps",
    "C",
    "D",
    "M",
    "peak_mb",
    "ok",
    "note",
)


def iter_rows(reports: Sequence[ImageReport]):
    """Yield flat rows for CSV/TSV export.

    Per image: a ``memcpy`` baseline row (``bytes`` = the raw ``.difr`` size,
    speeds = memcpy), then one row per codec whose ``bytes`` is the *compressed*
    size --- so the single ``bytes`` column shows the size difference directly.
    The old per-row ``difr_bytes``/``memcpy_mbps`` columns are gone (they were
    constant per image and folded into the memcpy row).
    """
    base = f"{compute_m(1, 1, 1):.4f}"
    for rep in reports:
        yield (
            rep.path,
            "memcpy",
            rep.difr_bytes,
            "1.0000",
            f"{rep.memcpy_mbps:.2f}",
            f"{rep.memcpy_mbps:.2f}",
            "1.00",
            "1.00",
            base,
            "",
            1,
            "",
        )
        for r in rep.results:
            comp_bytes = round(rep.difr_bytes / r.ratio_s) if r.ratio_s > 0 else ""
            yield (
                rep.path,
                r.name,
                comp_bytes,
                f"{r.ratio_s:.4f}",
                f"{r.comp_mbps:.2f}",
                f"{r.decomp_mbps:.2f}",
                f"{r.c:.2f}",
                f"{r.d:.2f}",
                f"{r.m:.4f}",
                f"{r.peak_mb:.1f}",
                int(r.available),
                r.note,
            )


def format_stats_table(stats: list[DirStat]) -> str:
    """Aggregate block as a GitHub-flavored markdown table."""
    rows = [
        "| codec | n | ratio S | C | D | M mean | M std | peak MB | note |",
        "|---|--:|--:|--:|--:|--:|--:|--:|---|",
    ]
    for s in stats:
        rows.append(
            f"| {s.name} | {s.n} | {s.ratio_s:.3f} | {s.c:.1f} | {s.d:.1f} "
            f"| {s.m_mean:.3f} | {s.m_std:.3f} | {s.peak_mb:.1f} | {s.note} |"
        )
    return "\n".join(rows)
