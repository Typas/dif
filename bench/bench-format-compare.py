"""Boxplot diff of two bench-format TSVs.

Compares a baseline run (A) against a new run (B) emitted by `just bench-formats`
(see bench/compare.py:iter_rows) and plots the *relative* ratio B/A per format as
horizontal boxplots, one figure per (source-extension, metric).

Usage:
    uv run bench/bench-format-compare.py <A.tsv> <B.tsv>

For every (image, format) pair present in both TSVs we compute B/A for each of
size / enc_mbps / dec_mbps. Pairs are then bucketed by the *original image file
extension* (the basename's suffix in the `image` column, e.g. `foo.drawio` ->
`drawio`) -- not the converted `format`. Within each extension every format
becomes one horizontal box whose spread is the ratio over all that extension's
images.

Output PNGs (transparent, written to CWD):
    bench-format-diff-<n>-<ext>-<size|enc|dec>.png
`n` is a per-run serial that avoids clobbering an older comparison: all PNGs from
one run share the lowest n >= 1 with no existing bench-format-diff-<n>-*.png.
"""

from __future__ import annotations

import csv
import sys
from collections import defaultdict
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt

# filename suffix -> (TSV column, title fragment)
METRICS = (
    ("size", "size", "Size (Relative)"),
    ("enc", "enc_mbps", "Encoding Speed (Relative)"),
    ("dec", "dec_mbps", "Decoding Speed (Relative)"),
)


def _read(path: Path) -> dict[tuple[str, str], dict[str, float]]:
    """Map (image, format) -> {size, enc, dec}, skipping unavailable rows.

    Unavailable formats write "" for size/enc/dec (bench/compare.py), so a field
    is only recorded when it parses as a float.
    """
    out: dict[tuple[str, str], dict[str, float]] = {}
    with path.open(newline="") as f:
        for row in csv.DictReader(f, delimiter="\t"):
            vals: dict[str, float] = {}
            for suffix, col, _ in METRICS:
                raw = (row.get(col) or "").strip()
                if raw:
                    try:
                        vals[suffix] = float(raw)
                    except ValueError:
                        pass
            if vals:
                out[(row["image"], row["format"])] = vals
    return out


def _format_order(path: Path) -> dict[str, list[str]]:
    """First-appearance order of `format` per extension, taken from A."""
    order: dict[str, list[str]] = defaultdict(list)
    seen: dict[str, set[str]] = defaultdict(set)
    with path.open(newline="") as f:
        for row in csv.DictReader(f, delimiter="\t"):
            ext = Path(row["image"]).suffix[1:]
            fmt = row["format"]
            if fmt not in seen[ext]:
                seen[ext].add(fmt)
                order[ext].append(fmt)
    return order


def _next_serial(outdir: Path) -> int:
    """Lowest n >= 1 with no existing bench-format-diff-<n>-*.png in outdir."""
    n = 1
    while any(outdir.glob(f"bench-format-diff-{n}-*.png")):
        n += 1
    return n


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__)
        print("error: expected exactly two TSV paths (A then B)", file=sys.stderr)
        return 2

    a_path, b_path = Path(argv[0]), Path(argv[1])
    a_map, b_map = _read(a_path), _read(b_path)
    ext_order = _format_order(a_path)

    # results[ext][fmt][metric_suffix] -> list of B/A ratios over images
    results: dict[str, dict[str, dict[str, list[float]]]] = defaultdict(
        lambda: defaultdict(lambda: defaultdict(list))
    )
    for key, a_vals in a_map.items():
        b_vals = b_map.get(key)
        if b_vals is None:
            print(f"WARN: missing {key} in {b_path.name}")
            continue
        image, fmt = key
        ext = Path(image).suffix[1:]
        for suffix, _, _ in METRICS:
            a = a_vals.get(suffix)
            b = b_vals.get(suffix)
            if a and b is not None:  # guard A != 0 and both present
                results[ext][fmt][suffix].append(b / a)

    if not results:
        print(
            "error: no (image, format) pairs shared between the two TSVs",
            file=sys.stderr,
        )
        return 1

    outdir = Path.cwd()
    n = _next_serial(outdir)
    written: list[str] = []

    for ext in (e for e in ext_order if e in results):  # A's extension order
        fmts = [f for f in ext_order[ext] if f in results[ext]]
        for suffix, _, label in METRICS:
            # Keep only formats with data for this metric, in first-seen order.
            boxed = [
                (f, results[ext][f][suffix]) for f in fmts if results[ext][f][suffix]
            ]
            if not boxed:
                continue
            labels = [f for f, _ in boxed]
            data = [d for _, d in boxed]

            fig, ax = plt.subplots(figsize=(24, 0.9 * len(boxed) + 4.5))
            ax.boxplot(data, vert=False, tick_labels=labels)
            ax.invert_yaxis()  # first format on top
            ax.axvline(1.0, linestyle="--", linewidth=0.8, color="0.4")
            ax.set_title(f"{ext.capitalize()} {label}", fontsize=30)
            ax.set_xlabel("B / A", fontsize=24)
            ax.tick_params(labelsize=21)

            name = f"bench-format-diff-{n}-{ext}-{suffix}.png"
            fig.savefig(outdir / name, transparent=True, bbox_inches="tight")
            plt.close(fig)
            written.append(name)

    for name in written:
        print(name)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
