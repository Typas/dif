"""Binding-level tests: encode/decode roundtrips against the Rust codec."""

from __future__ import annotations

import dif
import numpy as np
import pytest

CODECS = ("store", "deflate", "brotli")


def _indexed():
    light = [(255, 255, 255, 255), (0, 0, 0, 255)]
    dark = [(0, 0, 0, 255), (255, 255, 255, 255)]
    return dif.Image.indexed(
        2,
        2,
        8,
        [(0, "light"), (1, "dark")],
        [light, dark],
        [[0, 1, 1, 0]],
    )


@pytest.mark.parametrize("codec", CODECS)
def test_indexed_roundtrip_all_codecs(codec):
    img = _indexed()
    blob = img.to_dif(codec)
    assert blob[:4] == b"DIF1"
    back = dif.Image.from_dif(blob)
    assert back.width == 2 and back.height == 2
    assert back.themes == [(0, "light"), (1, "dark")]
    # source (light) theme reproduces original pixels
    _, _, light_rgba = back.render("light", 0)
    assert list(light_rgba[0:4]) == [255, 255, 255, 255]
    # dark theme swaps
    _, _, dark_rgba = back.render("dark", 0)
    assert list(dark_rgba[0:4]) == [0, 0, 0, 255]


def test_difr_roundtrip():
    img = _indexed()
    raw = img.to_difr()
    assert raw[:4] == b"DIFR"
    back = dif.Image.from_difr(raw)
    assert back.themes == [(0, "light"), (1, "dark")]


def test_grayscale_lut_roundtrip():
    levels = 256
    identity = list(range(levels))
    inverted = [255 - i for i in range(levels)]
    img = dif.Image.grayscale(
        2,
        2,
        8,
        [(0, "light"), (1, "dark")],
        [identity, inverted],
        [[10, 200, 0, 255]],
    )
    back = dif.Image.from_dif(img.to_dif("brotli"))
    assert back.is_grayscale
    _, _, light = back.render("light", 0)
    _, _, dark = back.render("dark", 0)
    assert light[0] == 10  # identity LUT
    assert dark[0] == 245  # 255 - 10


def test_theme_fallback_to_first():
    # Only a light theme present; asking for dark falls back to theme 0.
    img = dif.Image.indexed(1, 1, 8, [(0, "light")], [[(1, 2, 3, 255)]], [[0]])
    _, _, rgba = dif.Image.from_dif(img.to_dif("store")).render("dark", 0)
    assert list(rgba) == [1, 2, 3, 255]


def test_invalid_inputs_raise():
    with pytest.raises(ValueError):
        dif.Image.indexed(1, 1, 7, [(0, "x")], [[(0, 0, 0, 0)]], [[0]])  # bad depth
    with pytest.raises(ValueError):
        # palette index out of range
        dif.Image.indexed(1, 1, 8, [(0, "x")], [[(0, 0, 0, 0)]], [[5]])
    img = _indexed()
    with pytest.raises(ValueError):
        img.render("teal", 0)  # bad mode


def test_corrupt_magic_rejected():
    blob = bytearray(_indexed().to_dif("store"))
    blob[0] = ord("X")
    with pytest.raises(ValueError):
        dif.Image.from_dif(bytes(blob))


def test_16bit_grayscale_roundtrip():
    rng = np.random.default_rng(0)
    samples = rng.integers(0, 65536, size=16, dtype=np.uint16).tolist()
    identity = list(range(65536))
    img = dif.Image.grayscale(4, 4, 16, [(0, "light")], [identity], [samples])
    back = dif.Image.from_dif(img.to_dif("deflate"))
    assert back.depth_bits == 16
