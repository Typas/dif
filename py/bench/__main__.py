"""CLI for the benchmark harness.

uv run python -m bench setup            # build optional native codecs (lzav, kanzi, libbsc)
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
from .codecs import select_codecs
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


def _codecs(s: str) -> list[str]:
    """Parse an lzbench `-e` codec spec into a flat list of DIF variant strings.

    `/` separates codecs, `,` enumerates levels of the preceding family:
    `family,L1,L2…` -> `family-L1`, `family-L2`, … (`brotli,5,11` -> `brotli-5`,
    `brotli-11`; `zstd,3,22` -> `zstd-3`, `zstd-22`; `lz4,fast1,hc10` -> `lz4-fast1`,
    `lz4-hc10`). A bare family with no comma is its default level (`zstd`, `store`).
    `/` is never part of a codec name, so `--flag=a/b,1` parses and stays clear of
    the `images` positional."""
    out: list[str] = []
    for seg in s.split("/"):
        if not seg:
            continue
        family, *levels = seg.split(",")
        if levels:
            out.extend(f"{family}-{lvl}" for lvl in levels)
        else:
            out.append(family)
    return out


def _index_widths(s: str) -> list[str]:
    """`/`-split index-width list (same list axis as the codec flags), validating
    each against auto/8/16 (argparse `choices` can't pair with a list `type`)."""
    vals = [x for x in s.split("/") if x]
    bad = [v for v in vals if v not in ("auto", "8", "16")]
    if bad:
        raise argparse.ArgumentTypeError(
            f"invalid index width(s) {bad}; choose from auto, 8, 16"
        )
    return vals


def _check_codecs(parser: argparse.ArgumentParser, *specs: list[str] | None) -> None:
    """Reject unknown codec variants up front via `dif.validate_codec` (the core
    `Codec::parse`), so a typo errors once here instead of as a ValueError row per
    image in the table."""
    import dif

    bad: list[str] = []
    for spec in specs:
        for v in spec or []:
            try:
                dif.validate_codec(v)
            except ValueError:
                if v not in bad:
                    bad.append(v)
    if bad:
        parser.error(
            f"unknown DIF codec variant(s): {', '.join(bad)} "
            "(see --help for the family/level syntax)"
        )


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="bench")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("setup", help="build optional native codecs (lzav, kanzi, libbsc)")
    c = sub.add_parser("codecs", help="rank codecs over .difr by M")
    c.add_argument("images", nargs="*")
    c.add_argument("--strategy", default="arithmetic")
    c.add_argument("--repeats", type=int, default=5)
    c.add_argument(
        "--codecs",
        type=_codecs,
        default=None,
        metavar="lzbench-spec",
        help="select standalone codecs to bench, same lzbench `-e` syntax as "
        "--dif-codecs: `/` separates families, `,` enumerates levels (default: the "
        "whole registry), e.g. --codecs=zstd,3,10/bsc,1,2,3/lzav. Matches the names "
        "shown in the table (with bsc->libbsc, deflate->libdeflate aliases)",
    )
    c.add_argument(
        "--numthreads",
        type=int,
        default=1,
        help="codec threads (default 1 = single-thread). >1 uses each codec's "
        "multithreaded encoder where it has one (zstd), else single-thread",
    )
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
        "--numthreads",
        type=int,
        default=1,
        help="codec threads (default 1 = 1-core comparison); >1 adds dif -mt rows "
        "and scales jxl/avif and brotli (zstd barely splits)",
    )
    f.add_argument(
        "--dif-codecs",
        type=_codecs,
        default=list(DIF_CODECS),
        metavar="lzbench-spec",
        help="outer DIF codec variants, lzbench `-e` syntax: `/` separates codecs, "
        "`,` enumerates levels of the preceding family (default: the study set), "
        "e.g. --dif-codecs=zstd,3/brotli,5,11/store",
    )
    f.add_argument(
        "--dif-palette-codecs",
        type=_codecs,
        default=None,
        metavar="lzbench-spec",
        help="palette-section codecs, same `/`,`,` syntax as --dif-codecs (default: "
        "inherit the outer codec); a list runs the cartesian product with --dif-codecs",
    )
    f.add_argument(
        "--dif-frame-codecs",
        type=_codecs,
        default=None,
        metavar="lzbench-spec",
        help="frame-section codecs, same `/`,`,` syntax as --dif-codecs (default: "
        "inherit the outer codec); a list runs the cartesian product with --dif-codecs",
    )
    f.add_argument(
        "--index-width",
        type=_index_widths,
        default=["auto"],
        metavar="auto|8|16[/…]",
        help="`/`-separated DIF index width(s): auto-fit (default), or force 8/16-bit "
        "(quantizes to fit). Multiple values enumerate a dif row set per width. The "
        "resolved width shows in each row label as -8b/-16b",
    )
    f.add_argument(
        "--dif-only",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="only measure DIF codecs and report M relative to the store baseline",
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
        # Sources are git submodules; if a build fails, `git submodule update --init`.
        lz = native.build_lzav()
        print("lzav shim:", "built" if lz else "FAILED (needs cc + submodule)")
        kz = native.build_kanzi()
        print("kanzi shim:", "built" if kz else "FAILED (needs cargo + submodule)")
        bs = native.build_libbsc()
        print("libbsc shim:", "built" if bs else "FAILED (needs cc + c++ + submodule)")
        return 0

    imgs = _images(args.images)
    if not imgs:
        print("no images; pass image files or a directory, e.g. data/testdata/")
        return 1

    lfs_pointers = [
        p
        for p in imgs
        if open(p, "rb").read(200).startswith(b"version https://git-lfs.github.com")
    ]
    if lfs_pointers:
        print("error: the following files are Git LFS pointers (run `git lfs pull`):")
        for p in lfs_pointers:
            print(f"  {p}")
        return 1

    if args.cmd == "codecs":
        try:
            select_codecs(args.codecs, args.numthreads)  # validate early
        except ValueError as e:
            ap.error(str(e))
        reports = run(imgs, args.strategy, args.repeats, args.numthreads, args.codecs)

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
        _check_codecs(
            ap, args.dif_codecs, args.dif_palette_codecs, args.dif_frame_codecs
        )
        print(f"# {len(imgs)} images; per-(image,format) detail -> {args.out}\n")
        reports: list[cmp.ImageRows] = []
        # Per-image detail streams to the console and the TSV; the report gets
        # the aggregate (mirrors `bench codecs`).
        with open(args.out, "w", newline="") as fh:
            w = csv.writer(fh, delimiter="\t")
            w.writerow(cmp.TSV_HEADER)
            count = len(imgs)
            for i, p in enumerate(imgs):
                print(f"Benchmarking {p} ({i + 1}/{count}):")
                rows = compare_image(
                    p,
                    args.repeats,
                    args.dif_codecs,
                    args.dif_palette_codecs,
                    args.dif_frame_codecs,
                    stream=True,
                    numthreads=args.numthreads,
                    index_widths=args.index_width,
                    dif_only=args.dif_only,
                )
                print()
                w.writerows(cmp.iter_rows(p, rows))
                reports.append((p, rows))

        # Aggregate per directory, recursively (markdown tables) -> report.
        with open(args.report, "w", newline="") as rp:
            for label, stats in cmp.subdir_stats(reports):
                title = f"### {label}/  (formats aggregated over images beneath)"
                table = cmp.format_stats_table(stats)
                print(title)
                print(table)
                print()
                rp.write(f"{title}\n\n{table}\n\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
