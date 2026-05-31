"""CLI for the benchmark harness.

uv run python -m bench setup            # build optional native codecs (lzav)
uv run python -m bench codecs [imgs..]  # rank codecs over .difr by M
uv run python -m bench formats [imgs..] # compare DIF vs png/jxl/webp/avif/gif
"""

from __future__ import annotations

import argparse
import csv
import sys
from pathlib import Path

from . import native
from . import compare as cmp
from .compare import DIF_CODECS, compare_image
from .runner import (
    TSV_HEADER,
    format_stats_table,
    iter_rows,
    run,
    subdir_stats,
)

_IMAGE_EXTS = {
    ".png",
    ".gif",
    ".webp",
    ".bmp",
    ".jpg",
    ".jpeg",
    ".tif",
    ".tiff",
    ".drawio",
}


def _images(passed: list[str]) -> list[str]:
    """Expand each path: a directory yields its images, a file passes through.

    No default — the caller must name the images (or a dir of them).
    """
    out: list[str] = []
    for raw in passed:
        p = Path(raw)
        if p.is_dir():
            out.extend(str(q) for q in p.rglob("*") if q.suffix.lower() in _IMAGE_EXTS)
        else:
            out.append(str(p))
    return sorted(out)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="bench")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("setup", help="build optional native codecs (lzav)")
    c = sub.add_parser("codecs", help="rank codecs over .difr by M")
    c.add_argument("images", nargs="*")
    c.add_argument("--strategy", default="arithmetic")
    c.add_argument("--repeats", type=int, default=5)
    c.add_argument(
        "--out",
        default="bench-codecs.tsv",
        help="per-image results written here as TSV (default: bench-codecs.tsv)",
    )
    c.add_argument(
        "--report",
        default="bench-report.md",
        help="benchmark report (default: bench-report.md)",
    )
    f = sub.add_parser("formats", help="compare DIF vs other image formats")
    f.add_argument("images", nargs="*")
    f.add_argument("--repeats", type=int, default=3)
    f.add_argument(
        "--dif-codecs",
        nargs="+",
        choices=DIF_CODECS,
        default=list(DIF_CODECS),
        metavar="VARIANT",
        help="DIF codec variants to compare (default: all 7)",
    )
    f.add_argument(
        "--out",
        default="bench-formats.tsv",
        help="per-(image,format) results as TSV (default: bench-formats.tsv)",
    )
    f.add_argument(
        "--report",
        default="bench-formats.md",
        help="comparison report (default: bench-formats.md)",
    )
    args = ap.parse_args(argv)

    if args.cmd == "setup":
        lz = native.build_lzav()
        print("lzav shim:", "built" if lz else "FAILED (needs cc + network)")
        kz = native.build_kanzi()
        print("kanzi shim:", "built" if kz else "FAILED (needs cargo + git + network)")
        return 0

    imgs = _images(args.images)
    if not imgs:
        print("no images; pass image files or a directory, e.g. testdata/")
        return 1

    if args.cmd == "codecs":
        reports = run(imgs, args.strategy, args.repeats)

        # Per-image rows -> TSV (machine-parseable detail).
        with open(args.out, "w", newline="") as fh:
            w = csv.writer(fh, delimiter="\t")
            w.writerow(TSV_HEADER)
            w.writerows(iter_rows(reports))

        total = sum(r.difr_bytes for r in reports)
        print(f"# {len(reports)} images, {total} .difr bytes, strategy={args.strategy}")
        print(f"# per-image detail -> {args.out}")
        print("# C,D measured against each image's memcpy baseline\n")

        # Aggregate per directory, recursively (markdown tables).
        with open(args.report, "w", newline="") as rp:
            for label, stats in subdir_stats(reports):
                title = f"### {label}/  (M aggregated over images beneath)"
                table = format_stats_table(stats)
                print(title)
                print(table)
                print()
                rp.write(f"{title}\n\n{table}\n\n")
    elif args.cmd == "formats":
        print(f"# {len(imgs)} images; per-(image,format) detail -> {args.out}\n")
        with (
            open(args.report, "w", newline="") as rp,
            open(args.out, "w", newline="") as fh,
        ):
            w = csv.writer(fh, delimiter="\t")
            w.writerow(cmp.TSV_HEADER)
            count = len(imgs)
            for i, p in enumerate(imgs):
                print(f"Benchmarking {p} ({i + 1}/{count}):")
                # stream=True prints each format's row live (like bench codecs);
                # markdown + TSV go to the report files.
                rows = compare_image(p, args.repeats, args.dif_codecs, stream=True)
                print()
                rp.write(f"{cmp.markdown_table(p, rows)}\n\n")
                w.writerows(cmp.iter_rows(p, rows))
    return 0


if __name__ == "__main__":
    sys.exit(main())
