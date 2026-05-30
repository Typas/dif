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
    colors = np.array([[0, 0, 0, 255], [255, 255, 255, 255]], np.int64)
    out = derive_palette(colors, "arithmetic", 255)
    assert out[0, :3].mean() > 200  # black -> light
    assert out[1, :3].mean() < 55  # white -> dark


def test_invert_lut():
    lut = derive_lut("invert", 255)
    assert lut[0] == 255 and lut[255] == 0 and len(lut) == 256
