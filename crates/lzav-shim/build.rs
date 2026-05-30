// Compile the lzav shim (vendored single-header lzav.h + wrapper.c) into a
// static lib linked by dif-core's `native` codec path.

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vendor = root.join("vendor");
    if !vendor.join("lzav.h").exists() {
        panic!(
            "lzav.h not found at {}. Fetch with:\n  \
             curl -sSL -o {}/lzav.h https://raw.githubusercontent.com/avaneev/lzav/master/lzav.h",
            vendor.display(),
            vendor.display()
        );
    }

    let mut build = cc::Build::new();
    build
        .file(root.join("wrapper.c"))
        .include(&vendor)
        .warnings(false);
    build.flag_if_supported("-O3");
    build.compile("lzavshim");

    println!("cargo:rerun-if-changed=wrapper.c");
    println!("cargo:rerun-if-changed=vendor/lzav.h");
}
