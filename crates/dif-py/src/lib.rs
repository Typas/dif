//! Python bindings for `dif-core`, exposed as the `dif` extension module.
//!
//! Python builds palette/grayscale structures (typically from numpy) and uses
//! [`Image`] to encode to `.dif`/`.difr`, decode back, and render a theme.

// `derive_dark_palette` / `derive_dark_lut` are called fully-qualified
// (`dif_core::...`) so the same names can be exposed as module-level pyfunctions.
use dif_core::{
    from_dif, from_difr, grayscale_from_samples, indexed_from_rgba8, to_dif_workers, to_difr,
    CodecId,
    Content, DifError, DifImage, ModeTag, Rgba, SampleDepth, Strategy, Theme,
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

/// Map a palette's `max_value` (255 or 65535) to its sample depth. Used by the
/// module-level derivation helpers, which take `max_value` like the Python API.
fn depth_for_max(max_value: u16) -> PyResult<SampleDepth> {
    match max_value {
        255 => Ok(SampleDepth::Eight),
        65535 => Ok(SampleDepth::Sixteen),
        _ => Err(PyValueError::new_err("max_value must be 255 or 65535")),
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
        "zstd-22" => (CodecId::Zstd, 22),
        "lz4" | "lz4-fast1" => (CodecId::Lz4, 1),
        "lzav" | "lzav-1" => (CodecId::Lzav, 1),
        _ => {
            return Err(PyValueError::new_err(
                "codec must be one of: store, deflate/libdeflate-6, brotli-5, brotli-11, \
                 zstd-3, zstd-10, zstd-22, lz4-fast1, lzav-1 \
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

    /// Derive a dark theme natively and append it. For an indexed image the dark
    /// palette is derived from theme 0's palette; for grayscale a dark tone LUT is
    /// built. No palette/LUT crosses the FFI boundary — the converter just calls
    /// this after building the light image. `strategy` is `"keep"`, `"invert"`,
    /// or `"arithmetic"` (`"keep"` is a no-op caller-side and not expected here).
    fn add_dark_theme(&mut self, strategy: &str) -> PyResult<()> {
        let strat = Strategy::from_name(strategy).map_err(map_err)?;
        let depth = self.inner.depth;
        match &mut self.inner.content {
            Content::Indexed { palettes, .. } => {
                let light = palettes
                    .first()
                    .ok_or_else(|| PyValueError::new_err("image has no source palette"))?;
                let dark = dif_core::derive_dark_palette(light, strat, depth);
                palettes.push(dark);
            }
            Content::Grayscale { luts, .. } => {
                luts.push(dif_core::derive_dark_lut(strat, depth));
            }
        }
        self.inner
            .themes
            .push(Theme { tag: ModeTag::Dark, name: String::from("dark") });
        self.inner.validate().map_err(map_err)?;
        Ok(())
    }

    /// Build a single-theme (light) grayscale image straight from a packed sample
    /// buffer: `width*height` bytes for 8-bit, or `2*width*height`
    /// **little-endian** bytes for 16-bit. Mirrors [`Image::indexed_from_rgba8`]
    /// so Python hands over the raw bitmap instead of marshalling a per-pixel
    /// sample list. Add a dark theme afterwards with [`Image::add_dark_theme`].
    #[staticmethod]
    #[pyo3(signature = (width, height, depth_bits, samples))]
    fn grayscale_from_samples(
        width: u32,
        height: u32,
        depth_bits: u32,
        samples: &[u8],
    ) -> PyResult<Image> {
        let inner =
            grayscale_from_samples(width, height, depth(depth_bits)?, samples).map_err(map_err)?;
        Ok(Image { inner })
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
    /// carried by the string, so there is no separate level argument. `workers`
    /// > 0 runs the multithreaded zstd encoder (other codecs ignore it); the
    /// output is a standard container, decoded identically — no format change.
    #[pyo3(signature = (codec="zstd-3", workers=0))]
    fn to_dif<'py>(
        &self,
        py: Python<'py>,
        codec: &str,
        workers: u32,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let (id, level) = codec_id(codec)?;
        let bytes = to_dif_workers(&self.inner, id, level, workers).map_err(map_err)?;
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

/// Derive a dark-theme palette from a light one (native OKLab). `colors` is a
/// list of `(r, g, b, a)`; `strategy` is `"keep"`/`"invert"`/`"arithmetic"`;
/// `max_value` is 255 (8-bit) or 65535 (16-bit). Single source of truth for the
/// Python `dif_tools.themes.derive_palette` wrapper.
#[pyfunction]
fn derive_dark_palette(
    colors: Vec<(u16, u16, u16, u16)>,
    strategy: &str,
    max_value: u16,
) -> PyResult<Vec<(u16, u16, u16, u16)>> {
    let strat = Strategy::from_name(strategy).map_err(map_err)?;
    let pal: Vec<Rgba> = colors.into_iter().map(|(r, g, b, a)| Rgba::new(r, g, b, a)).collect();
    let out = dif_core::derive_dark_palette(&pal, strat, depth_for_max(max_value)?);
    Ok(out.into_iter().map(|c| (c.r, c.g, c.b, c.a)).collect())
}

/// Build the dark-theme grayscale LUT (native OKLab). Length is `max_value + 1`
/// (256 or 65536). Backs the Python `dif_tools.themes.derive_lut` wrapper.
#[pyfunction]
fn derive_dark_lut(strategy: &str, max_value: u16) -> PyResult<Vec<u16>> {
    let strat = Strategy::from_name(strategy).map_err(map_err)?;
    Ok(dif_core::derive_dark_lut(strat, depth_for_max(max_value)?))
}

#[pymodule]
fn dif(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Image>()?;
    m.add_function(wrap_pyfunction!(derive_dark_palette, m)?)?;
    m.add_function(wrap_pyfunction!(derive_dark_lut, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
