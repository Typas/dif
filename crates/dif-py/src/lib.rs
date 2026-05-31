//! Python bindings for `dif-core`, exposed as the `dif` extension module.
//!
//! Python builds palette/grayscale structures (typically from numpy) and uses
//! [`Image`] to encode to `.dif`/`.difr`, decode back, and render a theme.

use dif_core::{
    from_dif, from_difr, indexed_from_rgba8, to_dif, to_difr, CodecId, Content, DifError, DifImage,
    ModeTag, Rgba, SampleDepth, Theme,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

fn map_err(e: DifError) -> PyErr {
    PyValueError::new_err(e.to_string())
}

fn depth(bits: u32) -> PyResult<SampleDepth> {
    match bits {
        8 => Ok(SampleDepth::Eight),
        16 => Ok(SampleDepth::Sixteen),
        _ => Err(PyValueError::new_err("depth_bits must be 8 or 16")),
    }
}

fn mode_tag(s: &str) -> PyResult<ModeTag> {
    match s {
        "light" => Ok(ModeTag::Light),
        "dark" => Ok(ModeTag::Dark),
        "high-contrast" | "high_contrast" => Ok(ModeTag::HighContrast),
        _ => Err(PyValueError::new_err(
            "mode must be 'light', 'dark', or 'high-contrast'",
        )),
    }
}

/// Map a codec string to `(family, level)`. Accepts the study's 7 variant
/// strings; bare family names alias their study-chosen default level. This is
/// the single source of truth for per-family level semantics.
fn codec_id(s: &str) -> PyResult<(CodecId, u8)> {
    let pair = match s {
        "store" => (CodecId::Store, 0),
        "deflate" | "libdeflate" | "deflate-6" | "libdeflate-6" => (CodecId::Deflate, 6),
        "brotli" | "brotli-5" => (CodecId::Brotli, 5),
        "brotli-11" => (CodecId::Brotli, 11),
        "zstd" | "zstd-3" => (CodecId::Zstd, 3),
        "zstd-10" => (CodecId::Zstd, 10),
        "lz4" | "lz4-fast1" => (CodecId::Lz4, 1),
        "lzav" | "lzav-1" => (CodecId::Lzav, 1),
        _ => {
            return Err(PyValueError::new_err(
                "codec must be one of: store, deflate/libdeflate-6, brotli-5, brotli-11, \
                 zstd-3, zstd-10, lz4-fast1, lzav-1 \
                 (bare family names alias the study default level)",
            ))
        }
    };
    Ok(pair)
}

fn build_themes(themes: Vec<(u8, String)>) -> PyResult<Vec<Theme>> {
    themes
        .into_iter()
        .map(|(tag, name)| Ok(Theme { tag: ModeTag::from_u8(tag).map_err(map_err)?, name }))
        .collect()
}

fn themes_out(img: &DifImage) -> Vec<(u8, String)> {
    img.themes
        .iter()
        .map(|t| (t.tag as u8, t.name.clone()))
        .collect()
}

/// A DIF image wrapping the Rust `DifImage`.
#[pyclass]
pub struct Image {
    inner: DifImage,
}

#[pymethods]
impl Image {
    /// Build an indexed (palette) image.
    ///
    /// `themes`   : list of `(mode_tag, name)`; tag 0=light, 1=dark, 2=high-contrast.
    /// `palettes` : `palettes[theme]` is a list of `(r, g, b, a)` tuples, same
    ///              length for every theme.
    /// `frames`   : `frames[f]` is a flat row-major list of palette indices.
    #[staticmethod]
    #[pyo3(signature = (width, height, depth_bits, themes, palettes, frames, delays=None))]
    fn indexed(
        width: u32,
        height: u32,
        depth_bits: u32,
        themes: Vec<(u8, String)>,
        palettes: Vec<Vec<(u16, u16, u16, u16)>>,
        frames: Vec<Vec<u32>>,
        delays: Option<Vec<u16>>,
    ) -> PyResult<Image> {
        let palettes = palettes
            .into_iter()
            .map(|p| p.into_iter().map(|(r, g, b, a)| Rgba::new(r, g, b, a)).collect())
            .collect();
        let inner = DifImage {
            width,
            height,
            depth: depth(depth_bits)?,
            themes: build_themes(themes)?,
            content: Content::Indexed { palettes, frames },
            frame_delays: delays.unwrap_or_default(),
        };
        inner.validate().map_err(map_err)?;
        Ok(Image { inner })
    }

    /// Build a single-theme (light) indexed image straight from a packed RGBA8
    /// buffer (`4 * width * height` bytes). The palette dedup + index build run
    /// in Rust, so Python hands over the raw bitmap (like `png_encode(arr)`)
    /// instead of marshalling a per-pixel index list across the FFI boundary.
    /// Add a derived dark theme afterwards with [`Image::add_indexed_theme`].
    #[staticmethod]
    #[pyo3(signature = (width, height, depth_bits, rgba))]
    fn indexed_from_rgba8(
        width: u32,
        height: u32,
        depth_bits: u32,
        rgba: &[u8],
    ) -> PyResult<Image> {
        let inner =
            indexed_from_rgba8(width, height, depth(depth_bits)?, rgba).map_err(map_err)?;
        Ok(Image { inner })
    }

    /// One theme's palette as `(r, g, b, a)` tuples (small — cheap to marshal).
    /// Lets Python derive a dark palette from the Rust-built light one.
    fn palette(&self, theme: usize) -> PyResult<Vec<(u16, u16, u16, u16)>> {
        match &self.inner.content {
            Content::Indexed { palettes, .. } => {
                let p = palettes
                    .get(theme)
                    .ok_or_else(|| PyValueError::new_err("theme index out of range"))?;
                Ok(p.iter().map(|c| (c.r, c.g, c.b, c.a)).collect())
            }
            _ => Err(PyValueError::new_err("not an indexed image")),
        }
    }

    /// Append a theme and its palette (same length as the existing palettes).
    /// Used to attach the Python-derived dark theme to a Rust-built light image.
    fn add_indexed_theme(
        &mut self,
        tag: u8,
        name: String,
        palette: Vec<(u16, u16, u16, u16)>,
    ) -> PyResult<()> {
        let theme = Theme { tag: ModeTag::from_u8(tag).map_err(map_err)?, name };
        match &mut self.inner.content {
            Content::Indexed { palettes, .. } => {
                palettes.push(palette.into_iter().map(|(r, g, b, a)| Rgba::new(r, g, b, a)).collect());
            }
            _ => return Err(PyValueError::new_err("not an indexed image")),
        }
        self.inner.themes.push(theme);
        self.inner.validate().map_err(map_err)?;
        Ok(())
    }

    /// Build a grayscale image.
    ///
    /// `luts`   : `luts[theme]` maps a stored sample to a themed sample; length
    ///            equals `2**depth_bits` (256 or 65536). First theme is usually identity.
    /// `frames` : `frames[f]` is a flat row-major list of raw samples.
    #[staticmethod]
    #[pyo3(signature = (width, height, depth_bits, themes, luts, frames, delays=None))]
    fn grayscale(
        width: u32,
        height: u32,
        depth_bits: u32,
        themes: Vec<(u8, String)>,
        luts: Vec<Vec<u16>>,
        frames: Vec<Vec<u16>>,
        delays: Option<Vec<u16>>,
    ) -> PyResult<Image> {
        let inner = DifImage {
            width,
            height,
            depth: depth(depth_bits)?,
            themes: build_themes(themes)?,
            content: Content::Grayscale { luts, frames },
            frame_delays: delays.unwrap_or_default(),
        };
        inner.validate().map_err(map_err)?;
        Ok(Image { inner })
    }

    /// Encode to a compressed `.dif` container. `codec` is a single variant
    /// string (e.g. `"zstd-3"`, `"brotli-11"`, `"lz4-fast1"`); the level is
    /// carried by the string, so there is no separate level argument.
    #[pyo3(signature = (codec="zstd-3"))]
    fn to_dif<'py>(&self, py: Python<'py>, codec: &str) -> PyResult<Bound<'py, PyBytes>> {
        let (id, level) = codec_id(codec)?;
        let bytes = to_dif(&self.inner, id, level).map_err(map_err)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Encode to a raw, uncompressed `.difr`.
    fn to_difr<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = to_difr(&self.inner).map_err(map_err)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Render `frame` under the theme matching `mode` into packed RGBA8.
    /// Returns `(width, height, rgba_bytes)`.
    #[pyo3(signature = (mode="dark", frame=0))]
    fn render<'py>(
        &self,
        py: Python<'py>,
        mode: &str,
        frame: usize,
    ) -> PyResult<(u32, u32, Bound<'py, PyBytes>)> {
        let buf = self
            .inner
            .render_rgba8(mode_tag(mode)?, frame)
            .map_err(map_err)?;
        Ok((self.inner.width, self.inner.height, PyBytes::new(py, &buf)))
    }

    /// Decode a `.dif` container.
    #[staticmethod]
    fn from_dif(data: &[u8]) -> PyResult<Image> {
        Ok(Image { inner: from_dif(data).map_err(map_err)? })
    }

    /// Decode a raw `.difr`.
    #[staticmethod]
    fn from_difr(data: &[u8]) -> PyResult<Image> {
        Ok(Image { inner: from_difr(data).map_err(map_err)? })
    }

    #[getter]
    fn width(&self) -> u32 {
        self.inner.width
    }
    #[getter]
    fn height(&self) -> u32 {
        self.inner.height
    }
    #[getter]
    fn depth_bits(&self) -> u32 {
        match self.inner.depth {
            SampleDepth::Eight => 8,
            SampleDepth::Sixteen => 16,
        }
    }
    #[getter]
    fn frame_count(&self) -> usize {
        self.inner.frame_count()
    }
    #[getter]
    fn is_grayscale(&self) -> bool {
        matches!(self.inner.content, Content::Grayscale { .. })
    }
    #[getter]
    fn themes(&self) -> Vec<(u8, String)> {
        themes_out(&self.inner)
    }
}

#[pymodule]
fn dif(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Image>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
