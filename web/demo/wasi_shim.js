// Minimal wasi_snapshot_preview1 shim for the wasip1-built DIF decoder.
//
// The decoder is compiled to wasm32-wasip1 so the C codecs (zstd, lzav) link
// against wasi-libc's malloc. The wasm module therefore *imports* a few wasi
// functions that Rust's std runtime references --- but the decode path does no
// I/O and the module is a cdylib (no `main`), so none of these run during
// instantiation or decoding. They exist only to satisfy the imports; the only
// ones a fault could reach are fd_write / proc_exit on a panic.

const WASI_ESUCCESS = 0;

// Args/environment: report empty. Never invoked unless code reads env (it
// doesn't); returns success and writes zero counts when called.
export function environ_sizes_get() {
  return WASI_ESUCCESS;
}
export function environ_get() {
  return WASI_ESUCCESS;
}

// Only reachable via a Rust panic's message print. Pretend the write succeeded.
export function fd_write() {
  return WASI_ESUCCESS;
}

// Reachable only on abort/panic. Surface it instead of silently continuing.
export function proc_exit(code) {
  throw new Error(`wasm proc_exit(${code}) --- decoder aborted`);
}
