"""Regenerate web/flowchart.dif for the current `.dif` container format.

The wasm viewer loads `flowchart.dif`. When the `.dif` header format changes
(see `crates/dif-core`), the committed demo asset must be re-emitted or the
decoder rejects it. The *image body* (`write_body`) is stable across the v1->v2
bump — only the container header changed (a `level:u8` byte was inserted after
`codec:u8`, and `version` went 1->2). So this transcode is a pure header rewrite
that keeps the existing compressed stream byte-for-byte; the rendered demo is
unchanged.

Idempotent: a file already in the current format is verified and left as-is.
Run via `just regen-demo`.
"""

from __future__ import annotations

from pathlib import Path

import dif

_DIF = Path(__file__).resolve().parent.parent / "web" / "demo" / "flowchart.dif"
# The v1 brotli stream was produced at quality 9; record it as the level byte
# (informational only — decode is level-agnostic).
_BROTLI_LEVEL = 9


def regen() -> None:
    blob = _DIF.read_bytes()
    if blob[:4] != b"DIF1":
        raise SystemExit(f"{_DIF} is not a .dif container")

    version = blob[4]
    if version == 2:
        dif.Image.from_dif(blob)  # verify it still decodes
        print(f"{_DIF.name}: already v2 ({len(blob)} bytes) — no change")
        return
    if version != 1:
        raise SystemExit(f"unexpected .dif version {version}; cannot transcode")

    # v1: magic | ver | codec | raw_len[8] | body
    # v2: magic | ver | codec | level | raw_len[8] | body  (body unchanged)
    codec = blob[5]
    raw_len = blob[6:14]
    body = blob[14:]
    new = b"DIF1" + bytes([2, codec, _BROTLI_LEVEL]) + raw_len + body

    dif.Image.from_dif(new)  # verify the new decoder reads it before writing
    _DIF.write_bytes(new)
    print(f"{_DIF.name}: transcoded v1 -> v2 ({len(blob)} -> {len(new)} bytes)")


if __name__ == "__main__":
    regen()
