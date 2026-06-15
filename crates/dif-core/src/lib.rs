//! Core codec for **DIF v3** --- the Diagram Image Format.
//!
//! DIF is a lossless, theme-aware, palette-indexed raster format. A single file
//! carries one or more *themes*; each theme is a full palette plus an
//! [`abilities`] bitmask (which host appearances it can display under) and a
//! `base_color`. The decoder picks the theme matching the host appearance and
//! background (see [`DifImage::pick_theme`]).
//!
//! v3 vs v2: grayscale mode and the UTF-8-style varint index are gone. Indices
//! are a constant-width plane (8- or 16-bit), the mapped color is RGBA8 or
//! RGBA16, and the body uses a two-stage codec (per-palette + per-frame sections
//! wrapped by an outer pass) so a decoder can inflate one palette / one frame on
//! demand. See [`codec`] for the 64-byte container and [`format`] for the body.
//!
//! This crate root is a table of contents only: the container value types live in
//! [`container`], the dark-theme synthesis in [`regional`] / [`derive`], and the
//! on-disk codec in [`codec`] / [`format`].
//!
//! # Build features
//!
//! `no_std` + `alloc` by default (store / deflate / lz4). `std` adds Brotli;
//! `native` adds zstd + a libdeflate encoder + the lzav C shim; `encode` adds the
//! encode-side dark-theme derivation.

// `no_std` for the real library build; tests need std for the libtest harness.
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

extern crate alloc;

#[cfg(feature = "encode")]
pub mod aa_detect;
pub mod codec;
pub mod container;
#[cfg(feature = "encode")]
pub mod derive;
pub mod error;
pub mod format;
#[cfg(feature = "encode")]
pub mod quantize;
#[cfg(feature = "encode")]
pub mod regional;
#[cfg(feature = "encode")]
pub mod regions;

pub use codec::{
    Codec, CodecId, from_dif, from_dif_workers, from_difr, to_dif, to_dif_workers, to_difr,
};
pub use container::{
    ColorDepth, DifImage, Frame, IndexWidth, Rgba, Theme, ThemeTag, abilities, indexed_from_rgba8,
};
#[cfg(feature = "encode")]
pub use derive::{
    RegionClass, Strategy, dark_color_for, derive_dark_base_color, derive_dark_palette,
    oklab_close, text_dark_for,
};
pub use error::{DifError, Result};
#[cfg(feature = "encode")]
pub use regional::{RegionalReport, build_regional};
