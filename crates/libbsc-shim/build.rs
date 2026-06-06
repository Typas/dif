// Compile libbsc (IlyaGrebnov/libbsc, Apache-2.0) C/C++ sources plus a thin
// extern-"C" wrapper into static libs linked by dif-core's `bsc` codec path.
// CPU-only: CUDA (LIBBSC_CUDA_SUPPORT) and OpenMP (LIBBSC_OPENMP) are left
// undefined, so this is the single-thread BWT build. The sources come from the
// `vendor/libbsc` git submodule (repo root); the library tree is its `libbsc/`
// subdir.

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let v = root.join("vendor/libbsc/libbsc");
    if !v.join("libbsc.h").exists() {
        panic!(
            "libbsc sources not found at {}. Init the submodule:\n  \
             git submodule update --init crates/libbsc-shim/vendor/libbsc",
            v.display()
        );
    }

    // libsais is C99 — compile it as C in its own static lib.
    let mut c = cc::Build::new();
    c.file(v.join("bwt/libsais/libsais.c"))
        .warnings(false)
        .flag_if_supported("-O3");
    c.compile("sais");

    // libbsc C++ sources + the wrapper.
    let cpp = [
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
    ];
    let mut b = cc::Build::new();
    b.cpp(true).std("c++17").warnings(false);
    for f in cpp {
        b.file(v.join(f));
    }
    b.file(root.join("wrapper.cpp"))
        .include(root.join("vendor/libbsc"))
        .include(&v)
        .flag_if_supported("-O3");
    b.compile("bscshim");

    println!("cargo:rerun-if-changed=wrapper.cpp");
    println!("cargo:rerun-if-changed=vendor/libbsc/libbsc");
}
