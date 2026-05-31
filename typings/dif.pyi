"""Type stub for the `dif` Rust extension module (see crates/dif-py)."""

from __future__ import annotations

from typing import Literal

__version__: str

Theme = tuple[int, str]
Rgba = tuple[int, int, int, int]
Strategy = Literal["keep", "invert", "arithmetic"]

def derive_dark_palette(
    colors: list[Rgba], strategy: Strategy, max_value: int
) -> list[Rgba]:
    """Derive a dark-theme palette from a light one (native OKLab). `max_value`
    is 255 (8-bit) or 65535 (16-bit)."""
    ...

def derive_dark_lut(strategy: Strategy, max_value: int) -> list[int]:
    """Build the dark-theme grayscale LUT (native OKLab); length `max_value + 1`."""
    ...

# The study's 7 codec variants plus bare-family aliases (each alias resolves to
# its study-chosen default level: zstd->3, brotli->5, deflate->6, lz4->fast1,
# lzav->1). See crates/dif-py codec_id for the single source of truth.
CodecName = Literal[
    "store",
    "deflate",
    "libdeflate",
    "deflate-6",
    "libdeflate-6",
    "brotli",
    "brotli-5",
    "brotli-11",
    "zstd",
    "zstd-3",
    "zstd-10",
    "lz4",
    "lz4-fast1",
    "lzav",
    "lzav-1",
]

class Image:
    @staticmethod
    def indexed(
        width: int,
        height: int,
        depth_bits: int,
        themes: list[Theme],
        palettes: list[list[Rgba]],
        frames: list[list[int]],
        delays: list[int] | None = ...,
    ) -> Image: ...
    @staticmethod
    def indexed_from_rgba8(
        width: int, height: int, depth_bits: int, rgba: bytes
    ) -> Image:
        """Build a single-theme (light) indexed image from a packed RGBA8 buffer
        (`4 * width * height` bytes). Palette dedup + index build run natively."""
        ...

    @staticmethod
    def grayscale_from_samples(
        width: int, height: int, depth_bits: int, samples: bytes
    ) -> Image:
        """Build a single-theme (light) grayscale image from a packed sample
        buffer: `width*height` bytes for 8-bit, `2*width*height` little-endian
        bytes for 16-bit. Add a dark theme with `add_dark_theme`."""
        ...

    def add_dark_theme(self, strategy: Strategy) -> None:
        """Derive a dark theme natively (OKLab palette / tone LUT) and append it.
        No palette/LUT crosses the FFI boundary."""
        ...

    @staticmethod
    def grayscale(
        width: int,
        height: int,
        depth_bits: int,
        themes: list[Theme],
        luts: list[list[int]],
        frames: list[list[int]],
        delays: list[int] | None = ...,
    ) -> Image: ...
    def to_dif(self, codec: CodecName = ...) -> bytes:
        """Encode to a `.dif` container. `codec` is one variant string carrying
        both family and level (default `"zstd-3"`, the study's best pick)."""
        ...

    def to_difr(self) -> bytes: ...
    def render(self, mode: str = ..., frame: int = ...) -> tuple[int, int, bytes]: ...
    @staticmethod
    def from_dif(data: bytes) -> Image: ...
    @staticmethod
    def from_difr(data: bytes) -> Image: ...
    @property
    def width(self) -> int: ...
    @property
    def height(self) -> int: ...
    @property
    def depth_bits(self) -> int: ...
    @property
    def frame_count(self) -> int: ...
    @property
    def is_grayscale(self) -> bool: ...
    @property
    def themes(self) -> list[Theme]: ...
