"""CLI: ``uv run python -m dif_tools convert IN OUT [options]``."""

from __future__ import annotations

import argparse
import sys

from .convert import convert_file
from .themes import STRATEGIES


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
    conv.add_argument(
        "--codec",
        choices=("store", "deflate", "brotli"),
        default="brotli",
        help="compression codec for .dif (default: brotli)",
    )
    conv.add_argument(
        "--raw", action="store_true", help="write uncompressed .difr instead"
    )

    args = parser.parse_args(argv)
    if args.command == "convert":
        data = convert_file(
            args.input,
            args.output,
            strategy=args.theme_strategy,
            codec=args.codec,
            raw=args.raw,
        )
        print(f"wrote {args.output} ({len(data)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
