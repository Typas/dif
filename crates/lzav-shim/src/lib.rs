//! Safe Rust wrapper over the vendored `lzav.h` (avaneev/lzav), compiled as a
//! static shim. Used by `dif-core`'s `native` codec path; not built in the
//! portable no_std/wasm tier. Exposes lzav's default level (`lzav-1`) and its
//! high-ratio level (`lzav-2`); decompression is format-tagged, so a single
//! entry point decodes either.

use std::os::raw::{c_int, c_void};

type RawCompress = unsafe extern "C" fn(*const c_void, *mut c_void, c_int, c_int) -> c_int;

unsafe extern "C" {
    fn lzavshim_bound(srclen: c_int) -> c_int;
    fn lzavshim_bound_hi(srclen: c_int) -> c_int;
    fn lzavshim_compress(
        src: *const c_void,
        dst: *mut c_void,
        srclen: c_int,
        dstlen: c_int,
    ) -> c_int;
    fn lzavshim_compress_hi(
        src: *const c_void,
        dst: *mut c_void,
        srclen: c_int,
        dstlen: c_int,
    ) -> c_int;
    fn lzavshim_decompress(
        src: *const c_void,
        dst: *mut c_void,
        srclen: c_int,
        dstlen: c_int,
    ) -> c_int;
}

/// Upper bound on the compressed size for `srclen` input bytes (default level).
pub fn compress_bound(srclen: usize) -> usize {
    unsafe { lzavshim_bound(srclen as c_int).max(0) as usize }
}

/// Upper bound on the compressed size for `srclen` input bytes (high-ratio level).
pub fn compress_bound_hi(srclen: usize) -> usize {
    unsafe { lzavshim_bound_hi(srclen as c_int).max(0) as usize }
}

fn compress_with(src: &[u8], bound: usize, raw: RawCompress) -> Option<Vec<u8>> {
    let mut dst = vec![0u8; bound];
    let n = unsafe {
        raw(
            src.as_ptr() as *const c_void,
            dst.as_mut_ptr() as *mut c_void,
            src.len() as c_int,
            bound as c_int,
        )
    };
    // lzav returns 0 only for empty input; >0 otherwise, <0 never (it returns
    // 0 on the degenerate case). Treat n<=0 with non-empty input as failure.
    if n < 0 || (n == 0 && !src.is_empty()) {
        return None;
    }
    dst.truncate(n as usize);
    Some(dst)
}

/// Compress `src` at lzav's default level (`lzav-1`). `None` on failure.
pub fn compress(src: &[u8]) -> Option<Vec<u8>> {
    compress_with(src, compress_bound(src.len()), lzavshim_compress)
}

/// Compress `src` at lzav's high-ratio level (`lzav-2`). `None` on failure.
pub fn compress_hi(src: &[u8]) -> Option<Vec<u8>> {
    compress_with(src, compress_bound_hi(src.len()), lzavshim_compress_hi)
}

/// Decompress `src` into exactly `raw_len` bytes. `None` on failure.
pub fn decompress(src: &[u8], raw_len: usize) -> Option<Vec<u8>> {
    let mut dst = vec![0u8; raw_len];
    let n = unsafe {
        lzavshim_decompress(
            src.as_ptr() as *const c_void,
            dst.as_mut_ptr() as *mut c_void,
            src.len() as c_int,
            raw_len as c_int,
        )
    };
    if n < 0 {
        return None;
    }
    dst.truncate(n as usize);
    Some(dst)
}
