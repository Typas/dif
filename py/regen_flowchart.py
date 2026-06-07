"""Regenerate web/demo/flowchart.dif for the current `.dif` container format.

The wasm viewer loads `flowchart.dif`. This script decodes the demo down to
pixels --- from either a legacy v2 body (brotli + the v2 layout) or the current v3
container (via the decoder's `render`) --- recolors the largest blue block red, and
re-emits a v3 `.dif` with the shipped triplet (outer `store`, palette `zstd-16`,
frame `zstd-10`) and a derived dark theme.

Idempotent: once the red block is present the recolor is skipped, so re-running
only re-encodes (e.g. after a container-format bump) without painting a second
block. Run via `just regen-demo`.
"""

from __future__ import annotations

import struct
from collections import deque
from pathlib import Path

import dif

_DIF = Path(__file__).resolve().parent.parent / "web" / "demo" / "flowchart.dif"

# The red one blue block is repainted to (RGBA8). Doubles as the idempotency
# marker: if a pixel already holds it, the recolor has run before.
_RED = (210, 38, 30, 255)


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


def _decode_v2_light(blob: bytes) -> tuple[int, int, bytearray]:
    """Decode a legacy v2 indexed `.dif` to the light theme's RGBA8 pixels."""
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
    light: list[tuple[int, int, int, int]] = []
    for _ in range(color_count):  # theme 0 (light) palette; the rest are skipped
        light.append((body[pos], body[pos + 1], body[pos + 2], body[pos + 3]))
        pos += 4
    pos += 4 * color_count * (theme_count - 1)

    px = width * height
    rgba = bytearray(px * 4)
    for i in range(px):
        idx, pos = _read_varint(body, pos)
        rgba[4 * i : 4 * i + 4] = bytes(light[idx])
    return width, height, rgba


def _load_light_rgba(blob: bytes) -> tuple[int, int, bytearray]:
    """Decode the demo (v2 or v3) to its light theme's RGBA8 pixels."""
    if blob[:4] == b"DIF3":
        img = dif.Image.from_dif(blob)
        w, h, rgba = img.render("light", (255, 255, 255), 0)
        return w, h, bytearray(rgba)
    if blob[:4] == b"DIF1":
        return _decode_v2_light(blob)
    raise SystemExit(f"{_DIF} is not a known .dif container")


def _is_blue(r: int, g: int, b: int, a: int) -> bool:
    """A saturated, opaque blue fill pixel (block fill, not antialiased edges)."""
    return a > 0 and b > 110 and b > r + 35 and b > g + 35


def _recolor_largest_blue_block(w: int, h: int, rgba: bytearray) -> bool:
    """Repaint the largest 4-connected blue region to `_RED` in place. Returns
    False (no change) when the red is already present --- keeps the regen idempotent."""
    px = w * h
    if any(tuple(rgba[4 * i : 4 * i + 4]) == _RED for i in range(px)):
        return False
    blue = [_is_blue(*rgba[4 * i : 4 * i + 4]) for i in range(px)]

    seen = bytearray(px)
    best: list[int] = []
    for s in range(px):
        if not blue[s] or seen[s]:
            continue
        comp: list[int] = []
        q = deque([s])
        seen[s] = 1
        while q:
            p = q.popleft()
            comp.append(p)
            r, c = divmod(p, w)
            for nr, nc in ((r - 1, c), (r + 1, c), (r, c - 1), (r, c + 1)):
                if 0 <= nr < h and 0 <= nc < w:
                    n = nr * w + nc
                    if blue[n] and not seen[n]:
                        seen[n] = 1
                        q.append(n)
        if len(comp) > len(best):
            best = comp
    if not best:
        raise SystemExit("no blue block found to recolor")
    for p in best:
        rgba[4 * p : 4 * p + 4] = bytes(_RED)
    return True


def regen() -> None:
    blob = _DIF.read_bytes()
    w, h, rgba = _load_light_rgba(blob)
    changed = _recolor_largest_blue_block(w, h, rgba)

    img = dif.Image.indexed_from_rgba8(w, h, bytes(rgba), None)
    img.add_dark_theme("arithmetic")
    new = img.to_dif("store", "zstd-16", "zstd-10")
    dif.Image.from_dif(new)  # verify the new decoder reads it before writing
    _DIF.write_bytes(new)

    note = (
        "recolored largest blue block red" if changed else "red block already present"
    )
    print(f"{_DIF.name}: rebuilt -> v3 ({len(blob)} -> {len(new)} bytes), {note}")


if __name__ == "__main__":
    regen()
