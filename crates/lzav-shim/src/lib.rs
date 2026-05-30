//! Safe Rust wrapper over the vendored `lzav.h` (avaneev/lzav), compiled as a
//! static shim. Used by `dif-core`'s `native` codec path; not built in the
//! portable no_std/wasm tier. Exposes lzav's default level (the study's
//! `lzav-1` variant).

use std::os::raw::{c_int, c_void};

extern "C" {
    fn lzavshim_bound(srclen: c_int) -> c_int;
    fn lzavshim_compress(
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

/// Upper bound on the compressed size for `srclen` input bytes.
pub fn compress_bound(srclen: usize) -> usize {
    unsafe { lzavshim_bound(srclen as c_int).max(0) as usize }
}

/// Compress `src` at lzav's default level (`lzav-1`). `None` on failure.
pub fn compress(src: &[u8]) -> Option<Vec<u8>> {
    let bound = compress_bound(src.len());
    let mut dst = vec![0u8; bound];
    let n = unsafe {
        lzavshim_compress(
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
