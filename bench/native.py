"""Optional native codecs that have no PyPI wheel: lzav and kanzi.

Both are exposed to the benchmark as ``ctypes``-loaded C-ABI shared libraries.

- **lzav**: a single public-domain header. :func:`build_lzav` fetches it and
  compiles a tiny shim into ``bench/_native``.
- **kanzi**: adapted from kanzi-cpp via the Rust crate ``crates/kanzi-shim``
  (a cdylib wrapping kanzi's C API). :func:`build_kanzi` vendors kanzi-cpp and
  ``cargo build``s the shim; levels 1 and 2 are exposed.

Run ``python -m bench setup`` to build both.
"""

from __future__ import annotations

import ctypes
import shutil
import subprocess
import tempfile
import urllib.request
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .codecs import Codec

_ROOT = Path(__file__).parent.parent
_NATIVE_DIR = Path(__file__).parent / "_native"
_LZAV_HEADER_URL = "https://raw.githubusercontent.com/avaneev/lzav/master/lzav.h"
_LZAV_SO = _NATIVE_DIR / "liblzavshim.so"

_KANZI_DIR = _ROOT / "crates/kanzi-shim"
_KANZI_SO = _KANZI_DIR / "target/release/libkanzi_shim.so"
_KANZI_REPO = "https://github.com/flanglet/kanzi-cpp"
_KANZI_VENDOR = _KANZI_DIR / "vendor/kanzi-cpp"

_SHIM_C = """
#include "lzav.h"
int shim_bound(int n){ return lzav_compress_bound(n); }
int shim_compress(const void* s, void* d, int sl, int dl){ return lzav_compress_default(s,d,sl,dl); }
int shim_decompress(const void* s, void* d, int sl, int dl){ return lzav_decompress(s,d,sl,dl); }
"""


def build_lzav() -> bool:
    """Fetch lzav.h and compile the shim. Returns True on success."""
    cc = shutil.which("cc") or shutil.which("gcc") or shutil.which("clang")
    if cc is None:
        return False
    _NATIVE_DIR.mkdir(exist_ok=True)
    header = _NATIVE_DIR / "lzav.h"
    try:
        if not header.exists():
            with urllib.request.urlopen(_LZAV_HEADER_URL, timeout=30) as resp:
                header.write_bytes(resp.read())
        with tempfile.TemporaryDirectory() as td:
            cfile = Path(td) / "shim.c"
            cfile.write_text(_SHIM_C)
            subprocess.run(
                [
                    cc,
                    "-O3",
                    "-shared",
                    "-fPIC",
                    "-o",
                    str(_LZAV_SO),
                    str(cfile),
                    f"-I{_NATIVE_DIR}",
                ],
                check=True,
            )
        return _LZAV_SO.exists()
    except Exception:
        return False


def _lzav_codec() -> "Codec | None":
    if not _LZAV_SO.exists():
        return None
    lib = ctypes.CDLL(str(_LZAV_SO))
    lib.shim_bound.restype = ctypes.c_int
    lib.shim_bound.argtypes = [ctypes.c_int]
    for fn in (lib.shim_compress, lib.shim_decompress):
        fn.restype = ctypes.c_int
        fn.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]

    def compress(data: bytes) -> bytes:
        bound = lib.shim_bound(len(data))
        dst = ctypes.create_string_buffer(bound)
        n = lib.shim_compress(data, dst, len(data), bound)
        if n <= 0:
            raise RuntimeError("lzav compress failed")
        return dst.raw[:n]

    def decompress(comp: bytes, orig_len: int) -> bytes:
        dst = ctypes.create_string_buffer(orig_len)
        n = lib.shim_decompress(comp, dst, len(comp), orig_len)
        if n < 0:
            raise RuntimeError("lzav decompress failed")
        return dst.raw[:n]

    from .codecs import Codec as _C

    return _C("lzav-1", compress, decompress)


def build_kanzi() -> bool:
    """Vendor kanzi-cpp and cargo-build the shim cdylib. Returns True on success."""
    cargo = shutil.which("cargo")
    if cargo is None:
        return False
    try:
        if not _KANZI_VENDOR.exists():
            git = shutil.which("git")
            if git is None:
                return False
            subprocess.run(
                [git, "clone", "--depth", "1", _KANZI_REPO, str(_KANZI_VENDOR)],
                check=True,
            )
        subprocess.run(
            [
                cargo,
                "build",
                "--release",
                "--manifest-path",
                str(_KANZI_DIR / "Cargo.toml"),
            ],
            check=True,
        )
        return _KANZI_SO.exists()
    except Exception:
        return False


def _kanzi_codecs() -> list["Codec"]:
    if not _KANZI_SO.exists():
        return []
    lib = ctypes.CDLL(str(_KANZI_SO))
    lib.kanzi_bound.restype = ctypes.c_size_t
    lib.kanzi_bound.argtypes = [ctypes.c_size_t]
    lib.kanzi_compress.restype = ctypes.c_long
    lib.kanzi_compress.argtypes = [
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_int,
    ]
    lib.kanzi_decompress.restype = ctypes.c_long
    lib.kanzi_decompress.argtypes = [
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_void_p,
        ctypes.c_size_t,
    ]

    from .codecs import Codec as _C

    def make(level: int) -> "Codec":
        def compress(data: bytes) -> bytes:
            bound = lib.kanzi_bound(len(data))
            dst = ctypes.create_string_buffer(bound)
            n = lib.kanzi_compress(data, len(data), dst, bound, level)
            if n < 0:
                raise RuntimeError(f"kanzi compress failed ({n})")
            return dst.raw[:n]

        def decompress(comp: bytes, orig_len: int) -> bytes:
            dst = ctypes.create_string_buffer(orig_len)
            n = lib.kanzi_decompress(comp, len(comp), dst, orig_len)
            if n < 0:
                raise RuntimeError(f"kanzi decompress failed ({n})")
            return dst.raw[:n]

        return _C(f"kanzi-{level}", compress, decompress)

    return [make(1), make(2)]


def codecs() -> list["Codec"]:
    out: list[Codec] = []
    c = _lzav_codec()
    if c is not None:
        out.append(c)
    out.extend(_kanzi_codecs())
    return out
