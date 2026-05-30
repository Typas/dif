"""Quick ctypes self-test of the kanzi shim (run via `uv run python ...`)."""

import ctypes
from pathlib import Path

so = Path(__file__).parent / "target/release/libkanzi_shim.so"
lib = ctypes.CDLL(str(so))
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

data = bytes(range(256)) * 400  # ~100 KB with structure
for lvl in (1, 2):
    bound = lib.kanzi_bound(len(data))
    dst = ctypes.create_string_buffer(bound)
    n = lib.kanzi_compress(data, len(data), dst, bound, lvl)
    comp = dst.raw[:n]
    out = ctypes.create_string_buffer(len(data))
    m = lib.kanzi_decompress(comp, len(comp), out, len(data))
    ok = out.raw[:m] == data
    print(f"kanzi L{lvl}: comp={n} ratio={len(data) / n:.2f} lossless={ok}")
