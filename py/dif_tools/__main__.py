"""CLI: ``uv run python -m dif_tools convert IN OUT [options]``."""

from __future__ import annotations

import argparse
import sys

import dif

from .convert import convert_file
from .themes import STRATEGIES


def _codec(name: str) -> str:
    """An ``argparse`` type that accepts any codec variant the core
    ``Codec::parse`` accepts (the single source of truth), not a hand-maintained
    subset. ``dif.validate_codec`` raises ``ValueError`` on an unknown family or
    level, which argparse turns into a usage error."""
    dif.validate_codec(name)
    return name


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="dif_tools", description="Convert images to DIF."
    )
    sub = parser.add_subparsers(dest="command", required=True)

    conv = sub.add_parser("convert", help="Convert an image (or .drawio) to .dif/.difr")
    conv.add_argument("input", help="input image or .drawio file")
    conv.add_argument("output", help="output .dif (or .difr with --raw)")
    conv.add_argument(
        "--theme-strategy",
        choices=STRATEGIES,
        default="arithmetic",
        help="how to synthesize the dark theme (default: arithmetic)",
    )
    # Codec args accept any variant string `Codec::parse` accepts (validated via
    # `dif.validate_codec`), e.g. `store`, `zstd-16`, `brotli-11`, `lz4-fast1`.
    conv.add_argument(
        "--codec",
        type=_codec,
        metavar="CODEC",
        default="store",
        help=(
            "outer whole-body codec for .dif (default: store, which keeps frames "
            "seekable for low-memory parallel decode)"
        ),
    )
    conv.add_argument(
        "--palette-codec",
        type=_codec,
        metavar="CODEC",
        default="zstd-16",
        help="per-palette section codec (default: zstd-16)",
    )
    conv.add_argument(
        "--frame-codec",
        type=_codec,
        metavar="CODEC",
        default="zstd-10",
        help="per-frame section codec (default: zstd-10)",
    )
    conv.add_argument(
        "--raw", action="store_true", help="write uncompressed .difr instead"
    )
    conv.add_argument(
        "--index-width",
        choices=("auto", "8", "16"),
        default="auto",
        help="palette index width: auto-fit, or force 8/16-bit (quantizes to fit)",
    )
    conv.add_argument(
        "--threads",
        type=int,
        default=1,
        help="encode worker threads (default: 1 serial; >1 = inter-frame parallel)",
    )

    args = parser.parse_args(argv)
    if args.command == "convert":
        data = convert_file(
            args.input,
            args.output,
            strategy=args.theme_strategy,
            codec=args.codec,
            palette_codec=args.palette_codec,
            frame_codec=args.frame_codec,
            raw=args.raw,
            index_width=args.index_width,
            workers=args.threads,
        )
        print(f"wrote {args.output} ({len(data)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
