//! Rust cdylib that re-exports the kanzi C-ABI shim (see `wrapper.cpp`).
//!
//! The actual work is done by `wrapper.cpp` + the compiled kanzi-cpp sources
//! (linked in by `build.rs`). These thin `#[no_mangle]` wrappers guarantee the
//! symbols land in the cdylib's dynamic symbol table for `ctypes` to load.

use std::os::raw::{c_int, c_long, c_uchar};

extern "C" {
    fn kanzishim_bound(srclen: usize) -> usize;
    fn kanzishim_compress(
        src: *const c_uchar,
        srclen: usize,
        dst: *mut c_uchar,
        dstcap: usize,
        level: c_int,
    ) -> c_long;
    fn kanzishim_decompress(
        src: *const c_uchar,
        srclen: usize,
        dst: *mut c_uchar,
        dstcap: usize,
    ) -> c_long;
}

/// Upper bound on the compressed size of `srclen` bytes.
#[no_mangle]
pub extern "C" fn kanzi_bound(srclen: usize) -> usize {
    unsafe { kanzishim_bound(srclen) }
}

/// Compress `src[..srclen]` into `dst[..dstcap]` at `level`. Returns the
/// compressed length, or a negative error code.
///
/// # Safety
/// `src`/`dst` must be valid for `srclen`/`dstcap` bytes.
#[no_mangle]
pub unsafe extern "C" fn kanzi_compress(
    src: *const c_uchar,
    srclen: usize,
    dst: *mut c_uchar,
    dstcap: usize,
    level: c_int,
) -> c_long {
    kanzishim_compress(src, srclen, dst, dstcap, level)
}

/// Decompress `src[..srclen]` into `dst[..dstcap]`. Returns the decompressed
/// length, or a negative error code.
///
/// # Safety
/// `src`/`dst` must be valid for `srclen`/`dstcap` bytes.
#[no_mangle]
pub unsafe extern "C" fn kanzi_decompress(
    src: *const c_uchar,
    srclen: usize,
    dst: *mut c_uchar,
    dstcap: usize,
) -> c_long {
    kanzishim_decompress(src, srclen, dst, dstcap)
}
