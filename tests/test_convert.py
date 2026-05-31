"""Converter tests: lossless source theme, theme synthesis, strategies."""

from __future__ import annotations

import dif
import numpy as np
import pytest
from PIL import Image as PILImage

from dif_tools import image_to_dif_image
from dif_tools.themes import derive_lut, derive_palette


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
    back = dif.Image.from_dif(img.to_dif("brotli"))
    _, _, rgba = back.render("light", 0)
    got = np.frombuffer(rgba, np.uint8).reshape(8, 8, 4)
    assert np.array_equal(got, arr)


@pytest.mark.parametrize("strategy", ["keep", "invert", "arithmetic"])
def test_gray_source_theme_lossless(tmp_path, strategy):
    path, arr = _save_gray(tmp_path)
    img = image_to_dif_image(path, strategy=strategy)
    assert img.is_grayscale
    back = dif.Image.from_dif(img.to_dif("brotli"))
    _, _, rgba = back.render("light", 0)
    got = np.frombuffer(rgba, np.uint8).reshape(8, 8, 4)[..., 0]
    assert np.array_equal(got, arr)


@pytest.mark.parametrize(
    "codec,want_id,want_level",
    [
        ("zstd-3", 4, 3),
        ("zstd-10", 4, 10),
        ("brotli-5", 2, 5),
        ("brotli-11", 2, 11),
        ("libdeflate-6", 1, 6),
        ("lz4-fast1", 5, 1),
        ("lzav-1", 6, 1),
    ],
)
def test_dif_codec_variants_roundtrip(tmp_path, codec, want_id, want_level):
    path, arr = _save_color(tmp_path)
    img = image_to_dif_image(path, strategy="keep")
    blob = img.to_dif(codec)
    # Header records (codec byte, level byte) at offsets 5 and 6.
    assert blob[:4] == b"DIF1"
    assert blob[5] == want_id
    assert blob[6] == want_level
    back = dif.Image.from_dif(blob)
    _, _, rgba = back.render("light", 0)
    got = np.frombuffer(rgba, np.uint8).reshape(8, 8, 4)
    assert np.array_equal(got, arr)


def test_dif_default_codec_is_zstd3(tmp_path):
    path, _ = _save_color(tmp_path)
    img = image_to_dif_image(path, strategy="keep")
    blob = img.to_dif()  # no-arg default
    assert blob[5] == 4 and blob[6] == 3  # zstd, level 3


def test_keep_strategy_single_theme(tmp_path):
    path, _ = _save_color(tmp_path)
    img = image_to_dif_image(path, strategy="keep")
    assert img.themes == [(0, "light")]


def test_derived_dark_differs(tmp_path):
    path, arr = _save_color(tmp_path)
    img = image_to_dif_image(path, strategy="arithmetic")
    back = dif.Image.from_dif(img.to_dif("brotli"))
    _, _, light = back.render("light", 0)
    _, _, dark = back.render("dark", 0)
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


def test_invert_lut():
    lut = derive_lut("invert", 255)
    assert lut[0] == 255 and lut[255] == 0 and len(lut) == 256
