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
import platform
import shutil
import subprocess
import tempfile
import urllib.request
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .codecs import Codec

_ROOT = Path(__file__).parent.parent.parent
_NATIVE_DIR = Path(__file__).parent / "_native"
_LZAV_HEADER_URL = "https://raw.githubusercontent.com/avaneev/lzav/master/lzav.h"
_LZAV_SO = _NATIVE_DIR / "liblzavshim.so"

_KANZI_DIR = _ROOT / "crates/kanzi-shim"
_KANZI_SO = _KANZI_DIR / "target/release/libkanzi_shim.so"
_KANZI_REPO = "https://github.com/flanglet/kanzi-cpp"
_KANZI_VENDOR = _KANZI_DIR / "vendor/kanzi-cpp"

_ZXC_REPO = "https://github.com/hellobertrand/zxc"
_ZXC_SRC = _NATIVE_DIR / "zxc"
_ZXC_SO = _NATIVE_DIR / "libzxcshim.so"
_ZXC_LEVELS = (1, 3, 6)

_ZXC_SHIM_C = """
#include <stdint.h>
#include <string.h>
#include "zxc.h"
uint64_t shim_bound(uint64_t n){ return (uint64_t)zxc_compress_bound((size_t)n); }
long long shim_compress(const void* s, uint64_t sl, void* d, uint64_t dl,
                        int level, int n_threads){
    zxc_compress_opts_t o; memset(&o, 0, sizeof o);
    o.level = level; o.n_threads = n_threads;
    return (long long)zxc_compress(s, (size_t)sl, d, (size_t)dl, &o);
}
long long shim_decompress(const void* s, uint64_t sl, void* d, uint64_t dl){
    zxc_decompress_opts_t o; memset(&o, 0, sizeof o);
    return (long long)zxc_decompress(s, (size_t)sl, d, (size_t)dl, &o);
}
"""

_SHIM_C = """
#include "lzav.h"
int shim_bound(int n){ return lzav_compress_bound(n); }
int shim_compress(const void* s, void* d, int sl, int dl){ return lzav_compress_default(s,d,sl,dl); }
int shim_decompress(const void* s, void* d, int sl, int dl){ return lzav_decompress(s,d,sl,dl); }
"""


def build_lzav() -> bool:  # pragma: no cover
    """Fetch lzav.h and compile the shim. Returns True on success.

    No-cover: needs network + a C compiler; exercised by ``just bench-setup``.
    """
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


def build_kanzi() -> bool:  # pragma: no cover
    """Vendor kanzi-cpp and cargo-build the shim cdylib. Returns True on success.

    No-cover: needs git + cargo + network; exercised by ``just bench-setup``.
    """
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


# zxc compiles compress/decompress/huffman once per SIMD ISA (each tagged with a
# ZXC_FUNCTION_SUFFIX); the rest compile plain and the dispatcher selects at
# runtime. Mirror meson.build's `variant_sets`.
_ZXC_VARIANT_SRCS = ("zxc_compress.c", "zxc_decompress.c", "zxc_huffman.c")
_ZXC_PLAIN_SRCS = (
    "zxc_common.c",
    "zxc_driver.c",
    "zxc_dispatch.c",
    "zxc_pstream.c",
    "zxc_seekable.c",
)


def _zxc_variants() -> list[tuple[str, list[str]]]:
    m = platform.machine().lower()
    if m in ("x86_64", "amd64"):
        return [
            ("_default", []),
            ("_sse2", ["-msse2"]),
            ("_avx2", ["-mavx2", "-mfma", "-mbmi", "-mbmi2", "-mlzcnt"]),
            ("_avx512", ["-mavx512f", "-mavx512bw", "-mbmi", "-mbmi2", "-mlzcnt"]),
        ]
    if m in ("aarch64", "arm64"):
        return [("_default", []), ("_neon", ["-march=armv8-a+simd"])]
    if m.startswith("arm"):
        return [("_default", []), ("_neon", ["-march=armv7-a", "-mfpu=neon"])]
    return [("_default", [])]


def build_zxc() -> bool:  # pragma: no cover
    """Clone hellobertrand/zxc and compile its C + a shim into a shared lib.

    No-cover: needs git + a C compiler + network; exercised by ``bench setup``.
    """
    cc = shutil.which("cc") or shutil.which("gcc") or shutil.which("clang")
    git = shutil.which("git")
    if cc is None or git is None:
        return False
    _NATIVE_DIR.mkdir(exist_ok=True)
    lib = _ZXC_SRC / "src/lib"
    includes = [
        f"-I{_ZXC_SRC / 'include'}",
        f"-I{lib}",
        f"-I{lib / 'vendors'}",
    ]
    try:
        if not _ZXC_SRC.exists():
            subprocess.run(
                [git, "clone", "--depth", "1", _ZXC_REPO, str(_ZXC_SRC)], check=True
            )
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            objs: list[str] = []

            def cc_obj(src: Path, out: Path, extra: list[str]) -> None:
                subprocess.run(
                    [
                        cc,
                        "-O3",
                        "-fPIC",
                        "-c",
                        str(src),
                        "-o",
                        str(out),
                        *extra,
                        *includes,
                    ],
                    check=True,
                )
                objs.append(str(out))

            # Per-ISA variant objects (suffix-tagged function names).
            for suffix, flags in _zxc_variants():
                for name in _ZXC_VARIANT_SRCS:
                    out = tdp / f"{Path(name).stem}{suffix}.o"
                    cc_obj(lib / name, out, [f"-DZXC_FUNCTION_SUFFIX={suffix}", *flags])
            # Plain objects (the dispatcher + driver + the shim).
            for name in _ZXC_PLAIN_SRCS:
                cc_obj(lib / name, tdp / f"{Path(name).stem}.o", [])
            shim = tdp / "shim.c"
            shim.write_text(_ZXC_SHIM_C)
            cc_obj(shim, tdp / "shim.o", [])

            subprocess.run(
                [cc, "-shared", "-pthread", "-o", str(_ZXC_SO), *objs], check=True
            )
        return _ZXC_SO.exists()
    except Exception:
        return False


def _zxc_codecs() -> list["Codec"]:
    if not _ZXC_SO.exists():
        return []
    lib = ctypes.CDLL(str(_ZXC_SO))
    lib.shim_bound.restype = ctypes.c_uint64
    lib.shim_bound.argtypes = [ctypes.c_uint64]
    lib.shim_compress.restype = ctypes.c_longlong
    lib.shim_compress.argtypes = [
        ctypes.c_void_p,
        ctypes.c_uint64,
        ctypes.c_void_p,
        ctypes.c_uint64,
        ctypes.c_int,
        ctypes.c_int,
    ]
    lib.shim_decompress.restype = ctypes.c_longlong
    lib.shim_decompress.argtypes = [
        ctypes.c_void_p,
        ctypes.c_uint64,
        ctypes.c_void_p,
        ctypes.c_uint64,
    ]

    from .codecs import Codec as _C

    def make(level: int) -> "Codec":
        def _enc(data: bytes, n_threads: int) -> bytes:
            bound = lib.shim_bound(len(data))
            dst = ctypes.create_string_buffer(bound)
            n = lib.shim_compress(data, len(data), dst, bound, level, n_threads)
            if n < 0:
                raise RuntimeError(f"zxc compress failed ({n})")
            return dst.raw[:n]

        def compress(data: bytes) -> bytes:
            return _enc(data, 0)

        def mt_compress(data: bytes, nt: int) -> bytes:
            return _enc(data, nt)

        def decompress(comp: bytes, orig_len: int) -> bytes:
            dst = ctypes.create_string_buffer(orig_len)
            n = lib.shim_decompress(comp, len(comp), dst, orig_len)
            if n < 0:
                raise RuntimeError(f"zxc decompress failed ({n})")
            return dst.raw[:n]

        return _C(f"zxc-{level}", compress, decompress, mt_compress=mt_compress)

    return [make(lvl) for lvl in _ZXC_LEVELS]


def codecs() -> list["Codec"]:
    out: list[Codec] = []
    c = _lzav_codec()
    if c is not None:
        out.append(c)
    out.extend(_kanzi_codecs())
    out.extend(_zxc_codecs())
    return out
