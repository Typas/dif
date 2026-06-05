"""Optional native codecs that have no PyPI wheel: lzav, kanzi and libbsc.

All are exposed to the benchmark as ``ctypes``-loaded C-ABI shared libraries, and
all source from git submodules (``git submodule update --init`` first; no network
at build).

- **lzav**: a single public-domain header from the ``crates/lzav-shim/vendor/lzav``
  submodule (shared with dif-core's family-6 codec). :func:`build_lzav` compiles a
  tiny shim into ``bench/_native``.
- **kanzi**: adapted from kanzi-cpp via the Rust crate ``crates/kanzi-shim`` (a
  cdylib wrapping kanzi's C API), sources from its ``vendor/kanzi-cpp`` submodule.
  :func:`build_kanzi` ``cargo build``s the shim; levels 1 and 2 are exposed.
- **libbsc** (DIF family 3): the C/C++ sources + extern-"C" wrapper used by
  ``dif-core``'s ``bsc`` codec (the ``crates/libbsc-shim`` submodule).
  :func:`build_libbsc` compiles them into a ``.so``; levels 1–3.

Run ``python -m bench setup`` to build them.
"""

from __future__ import annotations

import ctypes
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .codecs import Codec

_ROOT = Path(__file__).parent.parent.parent
_NATIVE_DIR = Path(__file__).parent / "_native"
# lzav.h comes from the crates/lzav-shim/vendor/lzav git submodule (shared with
# dif-core's family-6 codec), not a download.
_LZAV_VENDOR = _ROOT / "crates/lzav-shim/vendor/lzav"
_LZAV_SO = _NATIVE_DIR / "liblzavshim.so"

_KANZI_DIR = _ROOT / "crates/kanzi-shim"
# `.cargo/config.toml` redirects every build (incl. excluded shim crates) to the
# repo-root target/, so kanzi-shim's cdylib lands there, not under its own crate.
_KANZI_SO = _ROOT / "target/release/libkanzi_shim.so"
# kanzi-cpp sources come from the crates/kanzi-shim/vendor/kanzi-cpp git submodule.
_KANZI_VENDOR = _KANZI_DIR / "vendor/kanzi-cpp"

# libbsc (family 3): reuse the C/C++ sources + extern-"C" wrapper that dif-core's
# `bsc` codec links (the crates/libbsc-shim/vendor/libbsc git submodule), compiled
# here into a ctypes .so. BWT block compressor; CPU single-thread (CUDA/OpenMP
# left undefined). The library tree is the submodule's `libbsc/` subdir.
_LIBBSC_DIR = _ROOT / "crates/libbsc-shim"
_LIBBSC_ROOT = _LIBBSC_DIR / "vendor/libbsc"  # submodule repo root
_LIBBSC_VENDOR = _LIBBSC_ROOT / "libbsc"  # source tree inside it
_LIBBSC_WRAPPER = _LIBBSC_DIR / "wrapper.cpp"
_LIBBSC_SO = _NATIVE_DIR / "libbscshim.so"
_LIBBSC_LEVELS = (1, 2, 3)  # QLFC coder: fast / static / adaptive
# C++ sources (mirrors crates/libbsc-shim/build.rs); libsais.c built separately as C.
_LIBBSC_CPP = (
    "adler32/adler32.cpp",
    "bwt/bwt.cpp",
    "coder/coder.cpp",
    "coder/qlfc/qlfc.cpp",
    "coder/qlfc/qlfc_model.cpp",
    "filters/detectors.cpp",
    "filters/preprocessing.cpp",
    "libbsc/libbsc.cpp",
    "lzp/lzp.cpp",
    "platform/platform.cpp",
    "st/st.cpp",
)

_SHIM_C = """
#include "lzav.h"
int shim_bound(int n){ return lzav_compress_bound(n); }
int shim_compress(const void* s, void* d, int sl, int dl){ return lzav_compress_default(s,d,sl,dl); }
int shim_decompress(const void* s, void* d, int sl, int dl){ return lzav_decompress(s,d,sl,dl); }
"""


def build_lzav() -> bool:  # pragma: no cover
    """Compile the lzav shim from the vendored submodule header. True on success.

    No-cover: needs a C compiler; exercised by ``just bench-setup``.
    """
    cc = shutil.which("cc") or shutil.which("gcc") or shutil.which("clang")
    if cc is None:
        return False
    if not (_LZAV_VENDOR / "lzav.h").exists():  # submodule not initialized
        return False
    _NATIVE_DIR.mkdir(exist_ok=True)
    try:
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
                    f"-I{_LZAV_VENDOR}",
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
    """Cargo-build the kanzi-cpp shim cdylib from the vendored submodule. True on
    success.

    No-cover: needs cargo; exercised by ``just bench-setup``.
    """
    cargo = shutil.which("cargo")
    if cargo is None:
        return False
    if not (_KANZI_VENDOR / "src").exists():  # submodule not initialized
        return False
    try:
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


def build_libbsc() -> bool:  # pragma: no cover
    """Compile libbsc + the extern-"C" wrapper into a ctypes shared lib.

    Sources come from the ``crates/libbsc-shim/vendor/libbsc`` git submodule (run
    ``git submodule update --init`` first; no network at build); mirrors that
    crate's ``build.rs``. No-cover: needs a C and C++ compiler; exercised by
    ``bench setup``.
    """
    cc = shutil.which("cc") or shutil.which("gcc") or shutil.which("clang")
    cxx = shutil.which("c++") or shutil.which("g++") or shutil.which("clang++")
    if cc is None or cxx is None:
        return False
    if not (_LIBBSC_VENDOR / "libbsc.h").exists():
        return False
    _NATIVE_DIR.mkdir(exist_ok=True)
    includes = [f"-I{_LIBBSC_ROOT}", f"-I{_LIBBSC_VENDOR}"]
    try:
        with tempfile.TemporaryDirectory() as td:
            tdp = Path(td)
            objs: list[str] = []

            # libsais is C99 -> compile as C.
            sais_o = tdp / "libsais.o"
            subprocess.run(
                [
                    cc,
                    "-O3",
                    "-fPIC",
                    "-c",
                    str(_LIBBSC_VENDOR / "bwt/libsais/libsais.c"),
                    "-o",
                    str(sais_o),
                    *includes,
                ],
                check=True,
            )
            objs.append(str(sais_o))

            # libbsc C++ sources + the extern-"C" wrapper (unique basenames).
            for src in (*(_LIBBSC_VENDOR / n for n in _LIBBSC_CPP), _LIBBSC_WRAPPER):
                out = tdp / f"{src.stem}.o"
                subprocess.run(
                    [
                        cxx,
                        "-O3",
                        "-fPIC",
                        "-std=c++17",
                        "-c",
                        str(src),
                        "-o",
                        str(out),
                        *includes,
                    ],
                    check=True,
                )
                objs.append(str(out))

            subprocess.run([cxx, "-shared", "-o", str(_LIBBSC_SO), *objs], check=True)
        return _LIBBSC_SO.exists()
    except Exception:
        return False


def _libbsc_codecs() -> list["Codec"]:
    if not _LIBBSC_SO.exists():
        return []
    lib = ctypes.CDLL(str(_LIBBSC_SO))
    lib.bscshim_bound.restype = ctypes.c_int
    lib.bscshim_bound.argtypes = [ctypes.c_int]
    lib.bscshim_compress.restype = ctypes.c_int
    lib.bscshim_compress.argtypes = [
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_int,
    ]
    lib.bscshim_decompress.restype = ctypes.c_int
    lib.bscshim_decompress.argtypes = [
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_void_p,
        ctypes.c_int,
    ]

    from .codecs import Codec as _C

    def make(level: int) -> "Codec":
        def compress(data: bytes) -> bytes:
            bound = lib.bscshim_bound(len(data))
            dst = ctypes.create_string_buffer(bound)
            n = lib.bscshim_compress(data, len(data), dst, bound, level)
            if n < 0:
                raise RuntimeError(f"libbsc compress failed ({n})")
            return dst.raw[:n]

        def decompress(comp: bytes, orig_len: int) -> bytes:
            dst = ctypes.create_string_buffer(orig_len)
            n = lib.bscshim_decompress(comp, len(comp), dst, orig_len)
            if n < 0:
                raise RuntimeError(f"libbsc decompress failed ({n})")
            return dst.raw[:n]

        return _C(f"libbsc-{level}", compress, decompress)

    return [make(lvl) for lvl in _LIBBSC_LEVELS]


def codecs() -> list["Codec"]:
    out: list[Codec] = []
    c = _lzav_codec()
    if c is not None:
        out.append(c)
    out.extend(_kanzi_codecs())
    out.extend(_libbsc_codecs())
    return out
