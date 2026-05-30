"""Run every codec over `.difr` payloads and rank them by the ``M`` metric."""

from __future__ import annotations

import statistics
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from dif_tools import image_to_dif_image

from .codecs import all_codecs
from .metric import compute_m, memcpy_speed, speed


@dataclass
class CodecResult:
    name: str
    ratio_s: float
    comp_mbps: float
    decomp_mbps: float
    m: float
    available: bool
    note: str = ""


def difr_payloads(
    paths: Sequence[str | Path], strategy: str = "arithmetic"
) -> list[tuple[str, bytes]]:
    out = []
    for p in paths:
        raw = image_to_dif_image(p, strategy=strategy).to_difr()
        out.append((Path(p).name, raw))
    return out


def run(paths: Sequence[str | Path], strategy: str = "arithmetic", repeats: int = 5):
    payloads = difr_payloads(paths, strategy)
    results: list[CodecResult] = []
    for codec in all_codecs():
        if not codec.available:
            results.append(
                CodecResult(codec.name, 0, 0, 0, float("-inf"), False, codec.note)
            )
            continue
        ratios, comps, decomps, ms = [], [], [], []
        ok = True
        for _name, raw in payloads:
            try:
                comp = codec.compress(raw)
                if codec.decompress(comp, len(raw)) != raw:
                    ok = False
                    break
                mem = memcpy_speed(raw, repeats)
                csp = speed(lambda: codec.compress(raw), len(raw), repeats)
                dsp = speed(lambda: codec.decompress(comp, len(raw)), len(raw), repeats)
                s = len(raw) / len(comp)
                ratios.append(s)
                comps.append(csp)
                decomps.append(dsp)
                ms.append(compute_m(s, mem / csp, mem / dsp))
            except Exception:  # noqa: BLE001
                ok = False
                break
        if ok and ms:
            results.append(
                CodecResult(
                    codec.name,
                    statistics.mean(ratios),
                    statistics.mean(comps) / 1e6,
                    statistics.mean(decomps) / 1e6,
                    statistics.mean(ms),
                    True,
                    codec.note,
                )
            )
        else:
            results.append(
                CodecResult(
                    codec.name, 0, 0, 0, float("-inf"), False, "roundtrip failed"
                )
            )
    results.sort(key=lambda r: r.m, reverse=True)
    return results, payloads


def format_table(results: list[CodecResult]) -> str:
    head = (
        f"{'codec':<14}{'ratio S':>9}{'comp MB/s':>11}{'decomp MB/s':>13}{'M':>9}  note"
    )
    lines = [head, "-" * len(head)]
    for r in results:
        if r.available:
            lines.append(
                f"{r.name:<14}{r.ratio_s:>9.3f}{r.comp_mbps:>11.1f}{r.decomp_mbps:>13.1f}{r.m:>9.3f}  {r.note}"
            )
        else:
            lines.append(
                f"{r.name:<14}{'unavailable':>9}{'':>11}{'':>13}{'':>9}  {r.note}"
            )
    return "\n".join(lines)
