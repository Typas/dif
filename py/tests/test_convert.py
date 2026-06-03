"""Converter tests: lossless source theme, theme synthesis, strategies (v3)."""

from __future__ import annotations

import dif
import numpy as np
import pytest
from PIL import Image as PILImage

from dif_tools import image_to_dif_image
from dif_tools.themes import derive_base_color, derive_palette

_LIGHT = (255, 255, 255)

# Outer codec family nibble (high 4 bits of the codec byte at offset 8).
_FAMILY = {"zstd": 4, "brotli": 2, "libdeflate": 1, "lz4": 5, "lzav": 6}


def _save_color(tmp_path, name="diag.png"):
    arr = np.zeros((8, 8, 4), np.uint8)
    arr[..., 3] = 255
    arr[:4, :4, :3] = (200, 30, 40)
    arr[4:, 4:, :3] = (30, 90, 200)
    arr[:4, 4:, :3] = (245, 245, 245)
    p = tmp_path / name
    PILImage.fromarray(arr, "RGBA").save(p)
    return p, arr


def _save_gray(tmp_path, name="g.png"):
    arr = np.arange(64, dtype=np.uint8).reshape(8, 8) * 4
    p = tmp_path / name
    PILImage.fromarray(arr, "L").save(p)
    return p, arr


@pytest.mark.parametrize("strategy", ["keep", "invert", "arithmetic"])
def test_color_source_theme_lossless(tmp_path, strategy):
    path, arr = _save_color(tmp_path)
    img = image_to_dif_image(path, strategy=strategy)
    back = dif.Image.from_dif(img.to_dif("brotli-5"))
    _, _, rgba = back.render("light", _LIGHT, 0)
    got = np.frombuffer(rgba, np.uint8).reshape(8, 8, 4)
    assert np.array_equal(got, arr)


@pytest.mark.parametrize("strategy", ["keep", "invert", "arithmetic"])
def test_gray_source_theme_lossless(tmp_path, strategy):
    # A grayscale source loads as RGBA8 (v3 is indexed-only) and round-trips.
    path, arr = _save_gray(tmp_path)
    img = image_to_dif_image(path, strategy=strategy)
    back = dif.Image.from_dif(img.to_dif("brotli-5"))
    _, _, rgba = back.render("light", _LIGHT, 0)
    got = np.frombuffer(rgba, np.uint8).reshape(8, 8, 4)
    expect = np.dstack([arr, arr, arr, np.full_like(arr, 255)])
    assert np.array_equal(got, expect)


@pytest.mark.parametrize(
    "codec,family",
    [
        ("zstd-3", 4),
        ("zstd-10", 4),
        ("brotli-5", 2),
        ("brotli-11", 2),
        ("libdeflate-6", 1),
        ("lz4-fast1", 5),
        ("lzav-1", 6),
    ],
)
def test_dif_codec_variants_roundtrip(tmp_path, codec, family):
    path, arr = _save_color(tmp_path)
    img = image_to_dif_image(path, strategy="keep")
    blob = img.to_dif(codec)
    # The outer codec byte (offset 8) packs family<<4 | level index.
    assert blob[:4] == b"DIF3"
    assert blob[8] >> 4 == family
    back = dif.Image.from_dif(blob)
    _, _, rgba = back.render("light", _LIGHT, 0)
    got = np.frombuffer(rgba, np.uint8).reshape(8, 8, 4)
    assert np.array_equal(got, arr)


def test_dif_default_codec_is_zstd(tmp_path):
    path, _ = _save_color(tmp_path)
    img = image_to_dif_image(path, strategy="keep")
    blob = img.to_dif()  # no-arg default
    assert blob[8] >> 4 == _FAMILY["zstd"]


def test_keep_strategy_single_theme(tmp_path):
    path, _ = _save_color(tmp_path)
    img = image_to_dif_image(path, strategy="keep")
    assert img.themes == [(1, _LIGHT)]  # abilities=light, white base


def test_derived_dark_differs(tmp_path):
    path, _ = _save_color(tmp_path)
    img = image_to_dif_image(path, strategy="arithmetic")
    back = dif.Image.from_dif(img.to_dif("brotli-5"))
    _, _, light = back.render("light", _LIGHT, 0)
    _, _, dark = back.render("dark", (0, 0, 0), 0)
    assert bytes(light) != bytes(dark)


def test_invert_palette_is_negative():
    colors = np.array(
        [[0, 0, 0, 255], [255, 255, 255, 255], [200, 30, 40, 128]], np.int64
    )
    out = derive_palette(colors, "invert", 255)
    assert list(out[0]) == [255, 255, 255, 255]  # black -> white
    assert list(out[1]) == [0, 0, 0, 255]  # white -> black
    assert list(out[2]) == [55, 225, 215, 128]  # alpha preserved


def test_arithmetic_lightness_inverts_extremes():
    # Achromatic colors flip fully: black -> light, white -> dark.
    colors = np.array([[0, 0, 0, 255], [255, 255, 255, 255]], np.int64)
    out = derive_palette(colors, "arithmetic", 255)
    assert out[0, :3].mean() > 200  # black -> light
    assert out[1, :3].mean() < 55  # white -> dark


def test_arithmetic_chromatic_stays_visible():
    # A light high-chroma color (yellow) must NOT crush to near-black; it keeps
    # its hue and lands at a visible mid lightness. Alpha is untouched.
    out = derive_palette(np.array([[253, 216, 53, 200]], np.int64), "arithmetic", 255)[
        0
    ]
    assert out[:3].max() > 120  # visible, not near-black
    assert out[0] > out[2] and out[1] > out[2]  # still warm: R,G > B
    assert out[3] == 200  # alpha preserved


def test_derive_base_color():
    assert derive_base_color((255, 255, 255), "invert") == (0, 0, 0)
    assert derive_base_color((0, 0, 0), "invert") == (255, 255, 255)
