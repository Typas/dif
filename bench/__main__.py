"""CLI for the benchmark harness.

uv run python -m bench setup            # build optional native codecs (lzav)
uv run python -m bench codecs [imgs..]  # rank codecs over .difr by M
uv run python -m bench formats [imgs..] # compare DIF vs png/jxl/webp/avif/gif
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from . import native
from .compare import compare_image
from .compare import format_table as compare_table
from .runner import format_table, run

_IMAGE_EXTS = {".png", ".gif", ".webp", ".bmp", ".jpg", ".jpeg"}
_DEFAULT_DIR = Path(__file__).parent.parent / "testdata" / "images"


def _images(passed: list[str]) -> list[str]:
    if passed:
        return passed
    return sorted(
        str(p) for p in _DEFAULT_DIR.glob("*") if p.suffix.lower() in _IMAGE_EXTS
    )


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="bench")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("setup", help="build optional native codecs (lzav)")
    c = sub.add_parser("codecs", help="rank codecs over .difr by M")
    c.add_argument("images", nargs="*")
    c.add_argument("--strategy", default="arithmetic")
    c.add_argument("--repeats", type=int, default=5)
    f = sub.add_parser("formats", help="compare DIF vs other image formats")
    f.add_argument("images", nargs="*")
    f.add_argument("--repeats", type=int, default=3)
    args = ap.parse_args(argv)

    if args.cmd == "setup":
        lz = native.build_lzav()
        print("lzav shim:", "built" if lz else "FAILED (needs cc + network)")
        kz = native.build_kanzi()
        print("kanzi shim:", "built" if kz else "FAILED (needs cargo + git + network)")
        return 0

    imgs = _images(args.images)
    if not imgs:
        print("no images; pass paths or populate testdata/images/")
        return 1

    if args.cmd == "codecs":
        results, payloads = run(imgs, args.strategy, args.repeats)
        total = sum(len(b) for _, b in payloads)
        print(
            f"# {len(payloads)} images, {total} .difr bytes, strategy={args.strategy}"
        )
        print(format_table(results))
    elif args.cmd == "formats":
        for p in imgs:
            print(compare_table(p, compare_image(p, args.repeats)))
            print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
