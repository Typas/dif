// Compile kanzi-cpp (the library sources + its C API) together with our
// in-memory wrapper into this crate's cdylib, so the C-ABI shim symbols are
// available to ctypes.

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("vendor/kanzi-cpp/src");
    if !src.exists() {
        panic!(
            "kanzi-cpp sources not found at {}. Clone with:\n  \
             git clone --depth 1 https://github.com/flanglet/kanzi-cpp {}/vendor/kanzi-cpp",
            src.display(),
            root.display()
        );
    }

    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").include(&src).warnings(false);
    build.flag_if_supported("-O3");
    build.flag_if_supported("-w");

    // All kanzi library sources, excluding the CLI app and tests.
    let pattern = format!("{}/**/*.cpp", src.display());
    for entry in glob::glob(&pattern).expect("glob kanzi sources") {
        let path = entry.expect("glob entry");
        let s = path.to_string_lossy().replace('\\', "/");
        if s.contains("/app/") || s.contains("/test/") || s.contains("/msvc/") {
            continue;
        }
        build.file(&path);
    }

    build.file(root.join("wrapper.cpp"));
    build.compile("kanzishim_native");

    println!("cargo:rerun-if-changed=wrapper.cpp");
    println!("cargo:rerun-if-changed={}", src.display());
}
