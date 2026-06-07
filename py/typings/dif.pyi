"""Type stub for the `dif` Rust extension module (see crates/dif-py), DIF v3."""

from __future__ import annotations

from typing import Literal

__version__: str

# A theme is its capability bitmask (bit0=light, bit1=dark, bit2=high-contrast)
# plus its base color (RGB8).
Abilities = int
Rgb = tuple[int, int, int]
Theme = tuple[Abilities, Rgb]
Rgba = tuple[int, int, int, int]
Strategy = Literal["keep", "invert", "arithmetic"]

def derive_dark_palette(
    colors: list[Rgba], strategy: Strategy, max_value: int
) -> list[Rgba]:
    """Derive a dark-theme palette from a light one (native OKLab). `max_value`
    is 255 (8-bit) or 65535 (16-bit)."""
    ...

def derive_dark_base_color(base: Rgb, strategy: Strategy) -> Rgb:
    """Derive the dark theme's base color (RGB8) from a light base color."""
    ...

# Any codec variant string the core `Codec::parse` accepts: a bare family aliasing
# its study-chosen default level (`zstd`->3, `brotli`->5, `deflate`->6, `lz4`->fast1,
# `lzav`->1, `bsc`->2) or `family-level` for any in-table level (`zstd-3`, `zstd--7`,
# `lz4-hc10`, `lzav-2`, `store`, ...). The set is open, so this is just `str`; see
# crates/dif-core Codec::parse for the truth, and `dif.validate_codec` to check one.
CodecName = str

def validate_codec(name: str) -> None:
    """Validate a codec variant string against the core `Codec::parse` (the single
    source of truth) without encoding; raises `ValueError` on an unknown variant or
    level. Lets the benchmark reject a bad `--outer-codecs` spec before the run."""
    ...

class Image:
    @staticmethod
    def indexed(
        width: int,
        height: int,
        color_bits: int,
        themes: list[Theme],
        palettes: list[list[Rgba]],
        frames: list[list[int]],
        delays: list[int] | None = ...,
        replay_count: int = ...,
    ) -> Image:
        """Build an indexed image. `color_bits` is 8 (RGBA8) or 16 (RGBA16); the
        index width is derived from the palette length. `delays` are per-frame
        microseconds; `replay_count` is 0=infinite, 1=static."""
        ...

    @staticmethod
    def indexed_from_rgba8(
        width: int, height: int, rgba: bytes, index_width: int | None = ...
    ) -> Image:
        """Build a single-theme (light) indexed image from a packed RGBA8 buffer
        (`4 * width * height` bytes). Palette dedup + index build run natively.
        `index_width` is `None` (auto-fit, quantizing only above 16-bit), `8`, or
        `16` (force that width, quantizing down when the source has more colors)."""
        ...

    def add_dark_theme(self, strategy: Strategy) -> None:
        """Derive a dark theme natively (OKLab palette + base color) and append it
        with abilities=dark. No palette crosses the FFI boundary."""
        ...

    def to_dif(
        self,
        codec: CodecName = ...,
        palette_codec: CodecName = ...,
        frame_codec: CodecName = ...,
        workers: int = ...,
    ) -> bytes:
        """Encode to a `.dif` container. `codec` is the outer whole-body codec;
        `palette_codec`/`frame_codec` compress the palette and frame sections
        (default `"store"` for the random-access layout). `workers` > 0 runs the
        multithreaded zstd/brotli encoder; the output is a standard container."""
        ...

    def to_difr(self) -> bytes: ...
    def render(
        self,
        mode: str = ...,
        base_color: Rgb = ...,
        frame: int = ...,
    ) -> tuple[int, int, bytes]:
        """Render `frame` under the theme matching `mode` and host `base_color`."""
        ...

    @staticmethod
    def from_dif(data: bytes, workers: int = ...) -> Image: ...
    @staticmethod
    def from_difr(data: bytes) -> Image: ...
    @property
    def width(self) -> int: ...
    @property
    def height(self) -> int: ...
    @property
    def color_bits(self) -> int: ...
    @property
    def index_bits(self) -> int: ...
    def quantized(self) -> bool:
        """`True` when the palette was OKLab-quantized to fit the index width."""
        ...

    @property
    def source_colors(self) -> int | None:
        """Unique-color count before quantization, or `None` if not quantized."""
        ...

    @property
    def frame_count(self) -> int: ...
    @property
    def replay_count(self) -> int: ...
    @property
    def themes(self) -> list[Theme]: ...
