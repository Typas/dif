"""Regenerate web/demo/flowchart.dif for the current `.dif` container format.

The wasm viewer loads `flowchart.dif`. The v3 format is a clean break from v2
(new 64-byte container, constant-width indices, no grayscale/varint), so the
committed demo asset can't be header-rewritten — it must be rebuilt from its
pixels. This script decodes the legacy v2 body inline (brotli + the v2 layout),
reconstructs both themes' palettes, and re-emits a v3 `.dif` with the same image.

Idempotent: a file already in v3 (`DIF3`) is verified and left as-is.
Run via `just regen-demo`.
"""

from __future__ import annotations

import struct
from pathlib import Path

import dif

_DIF = Path(__file__).resolve().parent.parent / "web" / "demo" / "flowchart.dif"

# v3 theme capability bits (mirror dif_core::abilities).
_ABILITY_LIGHT = 1
_ABILITY_DARK = 2


def _read_varint(buf: bytes, pos: int) -> tuple[int, int]:
    """Read one UTF-8-style varint (the v2 index encoding); return (value, pos)."""
    b0 = buf[pos]
    pos += 1
    if b0 < 0x80:
        return b0, pos
    if b0 < 0xE0:
        value, extra = b0 & 0x1F, 1
    elif b0 < 0xF0:
        value, extra = b0 & 0x0F, 2
    else:
        value, extra = b0 & 0x07, 3
    for _ in range(extra):
        value = (value << 6) | (buf[pos] & 0x3F)
        pos += 1
    return value, pos


def _decode_v2(
    blob: bytes,
) -> tuple[int, int, list[list[tuple[int, int, int, int]]], list[int]]:
    """Decode a legacy v2 indexed `.dif`; return (w, h, palettes, frame0_indices)."""
    import brotli

    if blob[5] != 2:
        raise SystemExit(f"expected brotli (codec 2) in legacy asset, got {blob[5]}")
    raw_len = struct.unpack("<Q", blob[7:15])[0]
    body = brotli.decompress(blob[15:])
    if len(body) != raw_len:
        raise SystemExit("v2 body length mismatch")

    pos = 0
    flags = body[pos]
    pos += 1
    if flags & 0b1:
        raise SystemExit("legacy flowchart is grayscale; v3 is indexed-only")
    if flags & 0b10:
        raise SystemExit("legacy flowchart is 16-bit; expected 8-bit")
    width, height, frame_count = struct.unpack_from("<III", body, pos)
    pos += 12
    theme_count = body[pos]
    pos += 1
    for _ in range(theme_count):  # skip themes (tag u8, name_len u8, name)
        pos += 1
        name_len = body[pos]
        pos += 1 + name_len
    pos += 2 * frame_count  # frame_delays u16

    color_count, pos = _read_varint(body, pos)
    palettes: list[list[tuple[int, int, int, int]]] = []
    for _ in range(theme_count):
        pal: list[tuple[int, int, int, int]] = []
        for _ in range(color_count):
            pal.append((body[pos], body[pos + 1], body[pos + 2], body[pos + 3]))
            pos += 4
        palettes.append(pal)

    px = width * height
    indices: list[int] = []
    for _ in range(px):
        idx, pos = _read_varint(body, pos)
        indices.append(idx)
    return width, height, palettes, indices


def regen() -> None:
    blob = _DIF.read_bytes()
    if blob[:4] == b"DIF3":
        dif.Image.from_dif(blob)  # verify it still decodes
        print(f"{_DIF.name}: already v3 ({len(blob)} bytes) — no change")
        return
    if blob[:4] != b"DIF1":
        raise SystemExit(f"{_DIF} is not a known .dif container")

    width, height, palettes, indices = _decode_v2(blob)
    base = [(_ABILITY_LIGHT, (255, 255, 255)), (_ABILITY_DARK, (0, 0, 0))]
    themes = base[: len(palettes)]
    img = dif.Image.indexed(width, height, 8, themes, palettes, [indices])

    new = img.to_dif("brotli-11")
    dif.Image.from_dif(new)  # verify the new decoder reads it before writing
    _DIF.write_bytes(new)
    print(f"{_DIF.name}: rebuilt v2 -> v3 ({len(blob)} -> {len(new)} bytes)")


if __name__ == "__main__":
    regen()
