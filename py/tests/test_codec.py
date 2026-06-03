"""Binding-level tests: encode/decode roundtrips against the Rust codec (v3)."""

from __future__ import annotations

import dif
import pytest

CODECS = ("store", "deflate", "brotli-5", "zstd-3", "lz4-fast1", "lzav-1")

_LIGHT_THEME = (1, (255, 255, 255))  # abilities=light, white base
_DARK_THEME = (2, (0, 0, 0))  # abilities=dark, black base


def _indexed():
    light = [(255, 255, 255, 255), (0, 0, 0, 255)]
    dark = [(0, 0, 0, 255), (255, 255, 255, 255)]
    return dif.Image.indexed(
        2,
        2,
        8,
        [_LIGHT_THEME, _DARK_THEME],
        [light, dark],
        [[0, 1, 1, 0]],
    )


@pytest.mark.parametrize("codec", CODECS)
def test_indexed_roundtrip_all_codecs(codec):
    img = _indexed()
    blob = img.to_dif(codec)
    assert blob[:4] == b"DIF3"
    back = dif.Image.from_dif(blob)
    assert back.width == 2 and back.height == 2
    assert back.themes == [_LIGHT_THEME, _DARK_THEME]
    # source (light) theme reproduces original pixels
    _, _, light_rgba = back.render("light", (255, 255, 255), 0)
    assert list(light_rgba[0:4]) == [255, 255, 255, 255]
    # dark theme swaps
    _, _, dark_rgba = back.render("dark", (0, 0, 0), 0)
    assert list(dark_rgba[0:4]) == [0, 0, 0, 255]


def test_per_section_codecs_roundtrip():
    # Outer Store + per-section palette/frame codecs (the random-access layout).
    img = _indexed()
    blob = img.to_dif("store", "zstd-3", "lz4-fast1")
    assert blob[:4] == b"DIF3"
    back = dif.Image.from_dif(blob)
    assert back.themes == [_LIGHT_THEME, _DARK_THEME]


def test_difr_roundtrip():
    img = _indexed()
    raw = img.to_difr()
    assert raw[:5] == b"DIFR3"
    back = dif.Image.from_difr(raw)
    assert back.themes == [_LIGHT_THEME, _DARK_THEME]


def test_rgba16_roundtrip():
    img = dif.Image.indexed(
        1, 1, 16, [_LIGHT_THEME], [[(65535, 1000, 0, 65535)]], [[0]]
    )
    back = dif.Image.from_dif(img.to_dif("deflate"))
    assert back.color_bits == 16
    # 16-bit color is scaled to 8-bit on render (>> 8).
    _, _, rgba = back.render("light", (255, 255, 255), 0)
    assert list(rgba) == [255, 3, 0, 255]


def test_16bit_index_width():
    # 300 distinct colors force a 16-bit index plane.
    palette = [(i % 256, (i // 256) % 256, 0, 255) for i in range(300)]
    frames = [[i for i in range(300)]]
    img = dif.Image.indexed(300, 1, 8, [_LIGHT_THEME], [palette], frames)
    assert img.index_bits == 16
    back = dif.Image.from_dif(img.to_dif("zstd-3"))
    assert back.index_bits == 16
    _, _, rgba = back.render("light", (255, 255, 255), 0)
    assert list(rgba[0:4]) == [0, 0, 0, 255]
    assert list(rgba[4:8]) == [1, 0, 0, 255]


def test_multiframe_delays_and_replay():
    img = dif.Image.indexed(
        2,
        1,
        8,
        [_LIGHT_THEME],
        [[(10, 20, 30, 255), (40, 50, 60, 255)]],
        [[0, 1], [1, 0]],
        delays=[100, 200],
        replay_count=0,
    )
    back = dif.Image.from_dif(img.to_dif("store", "store", "zstd-3"))
    assert back.frame_count == 2
    assert back.replay_count == 0
    _, _, f1 = back.render("light", (255, 255, 255), 1)
    assert list(f1[0:4]) == [40, 50, 60, 255]


def test_theme_fallback_to_first():
    # Only a light theme present; asking for dark falls back to theme 0.
    img = dif.Image.indexed(1, 1, 8, [_LIGHT_THEME], [[(1, 2, 3, 255)]], [[0]])
    _, _, rgba = dif.Image.from_dif(img.to_dif("store")).render("dark", (0, 0, 0), 0)
    assert list(rgba) == [1, 2, 3, 255]


def test_pick_nearest_base_color():
    pal = [(0, 0, 0, 255)]
    img = dif.Image.indexed(
        1,
        1,
        8,
        [(2, (0, 0, 0)), (2, (40, 40, 40))],  # two dark-capable themes
        [pal, [(255, 255, 255, 255)]],
        [[0]],
    )
    back = dif.Image.from_dif(img.to_dif("store"))
    # A charcoal host background is nearer theme 1 (base 40,40,40) -> white.
    _, _, rgba = back.render("dark", (45, 45, 45), 0)
    assert list(rgba) == [255, 255, 255, 255]


def test_invalid_inputs_raise():
    with pytest.raises(ValueError):
        dif.Image.indexed(
            1, 1, 7, [(1, (0, 0, 0))], [[(0, 0, 0, 0)]], [[0]]
        )  # bad bits
    with pytest.raises(ValueError):
        # palette index out of range
        dif.Image.indexed(1, 1, 8, [(1, (0, 0, 0))], [[(0, 0, 0, 0)]], [[5]])
    img = _indexed()
    with pytest.raises(ValueError):
        img.render("teal", (0, 0, 0), 0)  # bad mode


def test_corrupt_magic_rejected():
    blob = bytearray(_indexed().to_dif("store"))
    blob[0] = ord("X")
    with pytest.raises(ValueError):
        dif.Image.from_dif(bytes(blob))
