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
  :func:`build_libbsc` compiles them into a ``.so``. Exposed through the shim's
  parameterized ``_ex`` entry point so a spec mirrors the ``bsc`` CLI knobs:
  ``b<MB>`` block size (``-b``), ``m<n>`` block sorter (``-m``: 0=BWT, 3..8=ST),
  ``e<n>`` entropy coder (``-e``: 0=fast, 1=static, 2=adaptive). Codec names are
  ``libbsc-b25m0e1`` etc.; :func:`make_libbsc` builds any such spec on demand.
  ``m7``/``m8`` (ST7/ST8) are GPU-only: ``bench setup --cuda`` builds them (nvcc
  + an NVIDIA GPU), otherwise they're reported unavailable up front.

Run ``python -m bench setup`` to build them.
"""

from __future__ import annotations

import ctypes
import re
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

# --- libbsc -b/-m/-e spec parsing (mirrors bsc.cpp's ProcessSwitch) ----------
# A spec token is `b<MB>m<n>e<n>` (e.g. b25m0e1). The maps turn the bsc CLI's
# -m / -e digits into the LIBBSC_* enum values the shim forwards to bsc_compress.
_BME_RE = re.compile(r"^b(\d+)m(\d+)e(\d+)$")
# -m digit -> LIBBSC_BLOCKSORTER_* enum (see libbsc.h / bsc.cpp ProcessSwitch):
# m0 = BWT (enum 1); m3..m8 = ST3..ST8 (enum 3..8, identity). ST7/ST8 compile but
# need CUDA at runtime, so they're accepted here and caught by the probe.
_BSC_SORTER = {0: 1, 3: 3, 4: 4, 5: 5, 6: 6, 7: 7, 8: 8}
# -e digit -> LIBBSC_CODER_* enum. NOT the identity: the enum is STATIC=1,
# ADAPTIVE=2, FAST=3, but the CLI's -e digit order is fast/static/adaptive, so
# e0->FAST(3), e1->STATIC(1), e2->ADAPTIVE(2). Matches bsc.cpp's `-e` switch.
_BSC_CODER = {0: 3, 1: 1, 2: 2}
# LIBBSC_FEATURE_* bits passed to bsc_compress (see libbsc.h). FASTMODE is always
# on (as in the single-block path); CUDA is OR'd in only for ST7/ST8, the GPU-only
# sort transforms -- and only matters if the .so was built with `--cuda`.
_BSC_FEATURE_FASTMODE = 1
_BSC_FEATURE_CUDA = 8
_BSC_CUDA_SORTERS = (7, 8)  # -m7/-m8 = ST7/ST8: require a CUDA build + an NVIDIA GPU
# Default registry set (bare `bsc`/`libbsc` selects these): the three coders over
# a BWT block at the CLI's default 25 MB -- the old levels 1/2/3 by their specs.
_LIBBSC_DEFAULTS = ("b25m0e0", "b25m0e1", "b25m0e2")
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
int shim_bound_hi(int n){ return lzav_compress_bound_hi(n); }
int shim_compress(const void* s, void* d, int sl, int dl){ return lzav_compress_default(s,d,sl,dl); }
int shim_compress_hi(const void* s, void* d, int sl, int dl){ return lzav_compress_hi(s,d,sl,dl); }
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


def _lzav_codecs() -> "list[Codec]":
    """lzav-1 (default level) and lzav-2 (high ratio). Both decode via the same
    format-tagged ``lzav_decompress``. Empty when the shim isn't built."""
    if not _LZAV_SO.exists():
        return []
    lib = ctypes.CDLL(str(_LZAV_SO))
    for fn in (lib.shim_bound, lib.shim_bound_hi):
        fn.restype = ctypes.c_int
        fn.argtypes = [ctypes.c_int]
    for fn in (lib.shim_compress, lib.shim_compress_hi, lib.shim_decompress):
        fn.restype = ctypes.c_int
        fn.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]

    def decompress(comp: bytes, orig_len: int) -> bytes:
        dst = ctypes.create_string_buffer(orig_len)
        n = lib.shim_decompress(comp, dst, len(comp), orig_len)
        if n < 0:
            raise RuntimeError("lzav decompress failed")
        return dst.raw[:n]

    def _make_compress(bound_fn, compress_fn):
        def compress(data: bytes) -> bytes:
            bound = bound_fn(len(data))
            dst = ctypes.create_string_buffer(bound)
            n = compress_fn(data, dst, len(data), bound)
            if n <= 0:
                raise RuntimeError("lzav compress failed")
            return dst.raw[:n]

        return compress

    from .codecs import Codec as _C

    return [
        _C("lzav-1", _make_compress(lib.shim_bound, lib.shim_compress), decompress),
        _C(
            "lzav-2",
            _make_compress(lib.shim_bound_hi, lib.shim_compress_hi),
            decompress,
        ),
    ]


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


# GPU sort-transform sources (compiled with nvcc only for a `--cuda` build).
_LIBBSC_CU = ("st/st.cu", "bwt/libcubwt/libcubwt.cu")


def build_libbsc(cuda: bool = False) -> bool:  # pragma: no cover
    """Compile libbsc + the extern-"C" wrapper into a ctypes shared lib.

    Sources come from the ``crates/libbsc-shim/vendor/libbsc`` git submodule (run
    ``git submodule update --init`` first; no network at build); mirrors that
    crate's ``build.rs``. No-cover: needs a C and C++ compiler; exercised by
    ``bench setup``.

    With ``cuda=True`` the GPU sort transforms (``-m7``/``-m8`` = ST7/ST8) are
    enabled: the ``.cu`` kernels are built with ``nvcc``, every C++ TU gets
    ``-DLIBBSC_CUDA_SUPPORT``, and -- because libbsc's CUDA host locks need it
    (``CMakeLists.txt`` hard-requires OpenMP for CUDA) -- OpenMP is turned on too,
    with the CUDA runtime linked at the final step. OpenMP is a *system* package
    the user must install (``<omp.h>`` + libgomp/libomp via the toolchain, e.g.
    ``apt install libomp-dev`` or a gcc with libgomp); we don't vendor it. Returns
    ``False`` (so setup reports FAILED) if ``nvcc`` isn't on PATH or the OpenMP/
    CUDA toolchain is incomplete. The default CPU build leaves CUDA/OpenMP
    undefined, identical to what dif-core links.
    """
    cc = shutil.which("cc") or shutil.which("gcc") or shutil.which("clang")
    cxx = shutil.which("c++") or shutil.which("g++") or shutil.which("clang++")
    if cc is None or cxx is None:
        return False
    if not (_LIBBSC_VENDOR / "libbsc.h").exists():
        return False
    nvcc = shutil.which("nvcc") if cuda else None
    if cuda and nvcc is None:
        return False
    _NATIVE_DIR.mkdir(exist_ok=True)
    includes = [f"-I{_LIBBSC_ROOT}", f"-I{_LIBBSC_VENDOR}"]
    # CUDA: LIBBSC_CUDA_SUPPORT turns on the GPU dispatch in st.cpp/bwt.cpp, and
    # libbsc's CUDA host locks require OpenMP (CMake hard-requires it). OpenMP is a
    # *system* dependency: `-fopenmp` needs the toolchain's <omp.h> + libgomp/libomp
    # installed. LIBBSC_OPENMP_SUPPORT (not LIBBSC_OPENMP) is the right macro --
    # platform.h then includes <omp.h> when -fopenmp has defined _OPENMP, and
    # defines LIBBSC_OPENMP itself. Defining LIBBSC_OPENMP directly skips the header.
    # bwt.cpp calls libsais's `*_omp` variants under OpenMP, so LIBSAIS_OPENMP must
    # be defined when compiling both the C++ TUs (to see the declarations) and
    # libsais.c itself (to define them) -- mirrors CMake's PRIVATE LIBSAIS_OPENMP.
    cxx_cuda = (
        [
            "-DLIBBSC_CUDA_SUPPORT",
            "-DLIBBSC_OPENMP_SUPPORT",
            "-DLIBSAIS_OPENMP",
            "-fopenmp",
        ]
        if cuda
        else []
    )
    sais_cuda = ["-DLIBSAIS_OPENMP", "-fopenmp"] if cuda else []
    cuda_lib = None
    if nvcc is not None:  # set iff cuda; the check narrows nvcc to str for ty
        cuda_home = Path(nvcc).resolve().parent.parent  # .../bin/nvcc -> CUDA root
        cuda_lib = cuda_home / "lib64"
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
                    *sais_cuda,
                    "-c",
                    str(_LIBBSC_VENDOR / "bwt/libsais/libsais.c"),
                    "-o",
                    str(sais_o),
                    *includes,
                ],
                check=True,
            )
            objs.append(str(sais_o))

            # CUDA kernels via nvcc (-arch=native targets the build host's GPU,
            # matching the libbsc CMake default). -Xcompiler forwards host flags.
            if nvcc is not None:  # narrows nvcc to str for ty (set iff cuda)
                for name in _LIBBSC_CU:
                    src = _LIBBSC_VENDOR / name
                    out = tdp / f"{src.stem}.cu.o"
                    subprocess.run(
                        [
                            nvcc,
                            "-O3",
                            "-arch=native",
                            "-DLIBBSC_CUDA_SUPPORT",
                            "-DLIBBSC_OPENMP_SUPPORT",
                            # Vendored libcubwt.cu uses cub::Max/Min, deprecated in
                            # CUDA 12.x; silence the (cosmetic) deprecation noise on
                            # both device (--diag-suppress) and host (-Xcompiler).
                            "-Wno-deprecated-gpu-targets",
                            "--diag-suppress=1444",
                            "-Xcompiler",
                            "-fPIC",
                            "-Xcompiler",
                            "-fopenmp",
                            "-Xcompiler",
                            "-Wno-deprecated-declarations",
                            "-c",
                            str(src),
                            "-o",
                            str(out),
                            *includes,
                        ],
                        check=True,
                    )
                    objs.append(str(out))

            # libbsc C++ sources + the extern-"C" wrapper (unique basenames).
            for src in (*(_LIBBSC_VENDOR / n for n in _LIBBSC_CPP), _LIBBSC_WRAPPER):
                out = tdp / f"{src.stem}.o"
                subprocess.run(
                    [
                        cxx,
                        "-O3",
                        "-fPIC",
                        "-std=c++17",
                        *cxx_cuda,
                        "-c",
                        str(src),
                        "-o",
                        str(out),
                        *includes,
                    ],
                    check=True,
                )
                objs.append(str(out))

            link = [cxx, "-shared", "-o", str(_LIBBSC_SO), *objs]
            if cuda:
                link += ["-fopenmp", f"-L{cuda_lib}", "-lcudart"]
            subprocess.run(link, check=True)
        return _LIBBSC_SO.exists()
    except Exception:
        return False


_libbsc_lib = None  # cached CDLL handle (the .so is loaded at most once)


def _libbsc_load() -> "ctypes.CDLL | None":
    """Load + wire the libbsc shim's parameterized ``_ex`` entry points, once.

    ``None`` if the shim isn't built (``bench setup`` not run, or no compiler).
    """
    global _libbsc_lib
    if _libbsc_lib is not None:
        return _libbsc_lib
    if not _LIBBSC_SO.exists():
        return None
    lib = ctypes.CDLL(str(_LIBBSC_SO))
    lib.bscshim_bound_ex.restype = ctypes.c_int
    lib.bscshim_bound_ex.argtypes = [ctypes.c_int, ctypes.c_int]
    lib.bscshim_compress_ex.restype = ctypes.c_int
    lib.bscshim_compress_ex.argtypes = [
        ctypes.c_void_p,  # src
        ctypes.c_int,  # srclen
        ctypes.c_void_p,  # dst
        ctypes.c_int,  # dstcap
        ctypes.c_int,  # blockBytes
        ctypes.c_int,  # blockSorter (LIBBSC_* enum)
        ctypes.c_int,  # coder (LIBBSC_* enum)
        ctypes.c_int,  # features (LIBBSC_FEATURE_* bitmask)
    ]
    lib.bscshim_decompress_ex.restype = ctypes.c_int
    lib.bscshim_decompress_ex.argtypes = [
        ctypes.c_void_p,  # src
        ctypes.c_int,  # srclen
        ctypes.c_void_p,  # dst
        ctypes.c_int,  # rawlen
    ]
    _libbsc_lib = lib
    return lib


def make_libbsc(spec: str) -> "Codec | None":
    """Build a libbsc :class:`Codec` for a ``b<MB>m<n>e<n>`` spec (e.g. ``b25m0e1``).

    ``None`` when ``spec`` isn't a b/m/e triple (so the caller can fall through to
    other codecs) or the shim isn't built. ``ValueError`` when it *is* a b/m/e
    spec but a field is out of range -- a clear up-front error beats a per-image
    failure. Whether a parsed sorter actually runs (ST7/ST8 need a CUDA build +
    GPU) is decided later by :func:`unavailable_libbsc`, not here.
    """
    m = _BME_RE.match(spec)
    if m is None:
        return None
    block_mb, mdig, edig = (int(g) for g in m.groups())
    if not (1 <= block_mb <= 2047):
        raise ValueError(f"libbsc block size b{block_mb} out of range 1..2047 (MB)")
    if mdig not in _BSC_SORTER:
        raise ValueError(f"libbsc block sorter m{mdig} invalid (0=BWT, 3..8=ST3..ST8)")
    if edig not in _BSC_CODER:
        raise ValueError(f"libbsc coder e{edig} invalid (0=fast, 1=static, 2=adaptive)")
    sorter, coder = _BSC_SORTER[mdig], _BSC_CODER[edig]
    # ST7/ST8 only run on the GPU, so request the CUDA feature for them; harmless
    # (ignored) on a non-CUDA build, where the probe then reports them unavailable.
    features = _BSC_FEATURE_FASTMODE
    if mdig in _BSC_CUDA_SORTERS:
        features |= _BSC_FEATURE_CUDA
    block_bytes = block_mb * 1024 * 1024
    lib = _libbsc_load()
    if lib is None:
        return None

    from .codecs import Codec as _C

    def compress(data: bytes) -> bytes:
        bound = lib.bscshim_bound_ex(len(data), block_bytes)
        dst = ctypes.create_string_buffer(bound)
        n = lib.bscshim_compress_ex(
            data, len(data), dst, bound, block_bytes, sorter, coder, features
        )
        if n < 0:
            raise RuntimeError(f"libbsc compress failed ({n})")
        return dst.raw[:n]

    def decompress(comp: bytes, orig_len: int) -> bytes:
        dst = ctypes.create_string_buffer(max(orig_len, 1))
        n = lib.bscshim_decompress_ex(comp, len(comp), dst, orig_len)
        if n < 0:
            raise RuntimeError(f"libbsc decompress failed ({n})")
        return dst.raw[:n]

    return _C(f"libbsc-{spec}", compress, decompress)


# A small, compressible buffer the probe round-trips through each libbsc codec so
# the block sorter actually runs (an incompressible blob would be stored verbatim
# and never exercise -m). Large enough to drive the BWT/ST path, small enough to
# be instant.
_LIBBSC_PROBE = b"DIF libbsc sorter availability probe \x00\x01\x02\x03" * 64


def unavailable_libbsc(codecs: list["Codec"]) -> list[tuple[str, str]]:
    """Round-trip the probe buffer through each ``libbsc-*`` codec and return
    ``(name, reason)`` for any that fail -- chiefly ST7/ST8 (``-m7``/``-m8``),
    which return LIBBSC_NOT_SUPPORTED unless the shim was built with ``--cuda``
    *and* an NVIDIA GPU is present at runtime. Lets the CLI refuse a doomed run
    up front instead of dying mid-benchmark.
    """
    out: list[tuple[str, str]] = []
    for c in codecs:
        if not c.name.startswith("libbsc-"):
            continue
        try:
            if c.decompress(c.compress(_LIBBSC_PROBE), len(_LIBBSC_PROBE)) != (
                _LIBBSC_PROBE
            ):
                raise RuntimeError("roundtrip mismatch")
        except Exception as e:  # noqa: BLE001
            out.append((c.name, _libbsc_unavailable_reason(c.name, e)))
    return out


def _libbsc_unavailable_reason(name: str, err: Exception) -> str:
    """Friendly explanation for a failed libbsc probe; flags the CUDA-only
    sort transforms (m7/m8) specifically, else echoes the raw shim error."""
    m = _BME_RE.match(name.split("-", 1)[1] if "-" in name else "")
    if m is not None and int(m.group(2)) in _BSC_CUDA_SORTERS:
        return (
            f"block sorter m{m.group(2)} (ST{m.group(2)}) needs a CUDA build "
            "(`just bench-setup --cuda`) + an NVIDIA GPU"
        )
    return str(err)


def _libbsc_codecs() -> list["Codec"]:
    if _libbsc_load() is None:
        return []
    return [c for spec in _LIBBSC_DEFAULTS if (c := make_libbsc(spec)) is not None]


def codecs() -> list["Codec"]:
    out: list[Codec] = []
    out.extend(_lzav_codecs())
    out.extend(_kanzi_codecs())
    out.extend(_libbsc_codecs())
    return out
