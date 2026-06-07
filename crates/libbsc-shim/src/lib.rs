//! Safe Rust wrapper over the vendored libbsc (IlyaGrebnov/libbsc, Apache-2.0),
//! compiled as a static C++ shim. Used by `dif-core`'s `bsc` codec path (family
//! 3); not built in the portable no_std/wasm tier. CPU single-thread BWT.
//!
//! Every blob carries a 1-byte tag (libbsc block vs. stored-raw), so decode only
//! needs the known `raw_len` --- matching dif's other section codecs.

use std::os::raw::{c_int, c_uchar};

unsafe extern "C" {
    fn bscshim_bound(n: c_int) -> c_int;
    fn bscshim_compress(
        src: *const c_uchar,
        srclen: c_int,
        dst: *mut c_uchar,
        dstcap: c_int,
        level: c_int,
    ) -> c_int;
    fn bscshim_decompress(
        src: *const c_uchar,
        srclen: c_int,
        dst: *mut c_uchar,
        rawlen: c_int,
    ) -> c_int;
}

/// Compress `src` at the given dif level (1=QLFC fast, 2=static, 3=adaptive),
/// all over a BWT block. `None` on failure or if the input exceeds libbsc's
/// `i32` block limit.
pub fn compress(src: &[u8], level: i32) -> Option<Vec<u8>> {
    if src.len() > c_int::MAX as usize {
        return None;
    }
    let bound = unsafe { bscshim_bound(src.len() as c_int) }.max(0) as usize;
    let mut dst = vec![0u8; bound];
    let n = unsafe {
        bscshim_compress(
            src.as_ptr(),
            src.len() as c_int,
            dst.as_mut_ptr(),
            bound as c_int,
            level as c_int,
        )
    };
    if n < 0 {
        return None;
    }
    dst.truncate(n as usize);
    Some(dst)
}

/// Decompress `src` into exactly `raw_len` bytes. `None` on failure.
pub fn decompress(src: &[u8], raw_len: usize) -> Option<Vec<u8>> {
    if raw_len > c_int::MAX as usize {
        return None;
    }
    let mut dst = vec![0u8; raw_len];
    let n = unsafe {
        bscshim_decompress(
            src.as_ptr(),
            src.len() as c_int,
            dst.as_mut_ptr(),
            raw_len as c_int,
        )
    };
    if n < 0 {
        return None;
    }
    dst.truncate(n as usize);
    Some(dst)
}
