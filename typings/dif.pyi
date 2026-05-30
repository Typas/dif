"""Type stub for the `dif` Rust extension module (see crates/dif-py)."""

from __future__ import annotations

__version__: str

Theme = tuple[int, str]
Rgba = tuple[int, int, int, int]

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
    def grayscale(
        width: int,
        height: int,
        depth_bits: int,
        themes: list[Theme],
        luts: list[list[int]],
        frames: list[list[int]],
        delays: list[int] | None = ...,
    ) -> Image: ...
    def to_dif(self, codec: str = ...) -> bytes: ...
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
