//! Python bindings for `dif-core` (DIF v3), exposed as the `dif` extension module.
//!
//! Python builds palette/index structures (typically from numpy) and uses
//! [`Image`] to encode to `.dif`/`.difr`, decode back, and render a theme. v3 is
//! indexed-only: themes carry an `abilities` bitmask + RGB `base_color`, indices
//! are constant-width (8/16-bit), and the container compresses the palette and
//! each frame with their own codec under an outer pass.

use dif_core::{
    abilities, from_dif_workers, from_difr, indexed_from_rgba8, to_dif_workers, to_difr, Codec,
    ColorDepth, DifError, DifImage, Frame, IndexWidth, Rgba, Strategy, Theme, ThemeTag,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

fn map_err(e: DifError) -> PyErr {
    PyValueError::new_err(e.to_string())
}

fn color_depth(bits: u32) -> PyResult<ColorDepth> {
    match bits {
        8 => Ok(ColorDepth::Rgba8),
        16 => Ok(ColorDepth::Rgba16),
        _ => Err(PyValueError::new_err("color_bits must be 8 or 16")),
    }
}

/// Map a palette's `max_value` (255 or 65535) to its color depth. Used by the
/// module-level derivation helper, which takes `max_value` like the Python API.
fn depth_for_max(max_value: u16) -> PyResult<ColorDepth> {
    match max_value {
        255 => Ok(ColorDepth::Rgba8),
        65535 => Ok(ColorDepth::Rgba16),
        _ => Err(PyValueError::new_err("max_value must be 255 or 65535")),
    }
}

fn theme_tag(s: &str) -> PyResult<ThemeTag> {
    match s {
        "light" => Ok(ThemeTag::Light),
        "dark" => Ok(ThemeTag::Dark),
        "high-contrast" | "high_contrast" => Ok(ThemeTag::HighContrast),
        _ => Err(PyValueError::new_err(
            "mode must be 'light', 'dark', or 'high-contrast'",
        )),
    }
}

fn codec(s: &str) -> PyResult<Codec> {
    Codec::parse(s).map_err(map_err)
}

/// Validate a codec variant string against `Codec::parse` (the single source of
/// truth) without encoding; raises `ValueError` on an unknown variant or level.
/// Lets a caller reject a bad `--outer-codecs` spec up front, before the run.
#[pyfunction]
fn validate_codec(name: &str) -> PyResult<()> {
    codec(name).map(|_| ())
}

fn build_themes(themes: Vec<(u8, (u8, u8, u8))>) -> Vec<Theme> {
    themes
        .into_iter()
        .map(|(abilities, (r, g, b))| Theme {
            abilities,
            base_color: [r, g, b],
        })
        .collect()
}

fn themes_out(img: &DifImage) -> Vec<(u8, (u8, u8, u8))> {
    img.themes
        .iter()
        .map(|t| {
            (
                t.abilities,
                (t.base_color[0], t.base_color[1], t.base_color[2]),
            )
        })
        .collect()
}

/// A DIF image wrapping the Rust `DifImage`.
#[pyclass]
pub struct Image {
    inner: DifImage,
    /// Unique-color count of the source before palette quantization, or `None`
    /// when the image fit its index width losslessly. Only `indexed_from_rgba8`
    /// can set it; images loaded from a `.dif`/`.difr` carry `None`.
    source_colors: Option<u64>,
}

#[pymethods]
impl Image {
    /// Build an indexed image.
    ///
    /// `color_bits` : 8 (RGBA8) or 16 (RGBA16) palette channel depth.
    /// `themes`     : list of `(abilities, (r, g, b))`; abilities bit0=light,
    ///                bit1=dark, bit2=high-contrast; `(r,g,b)` is the base color.
    /// `palettes`   : `palettes[theme]` is a list of `(r, g, b, a)`, same length
    ///                for every theme; the index width is derived from that length.
    /// `frames`     : `frames[f]` is a flat row-major list of palette indices.
    /// `delays`     : per-frame display delay in microseconds (default 0).
    /// `replay_count`: 0 = infinite, 1 = static (default 1).
    #[staticmethod]
    #[pyo3(signature = (width, height, color_bits, themes, palettes, frames, delays=None, replay_count=1))]
    #[allow(clippy::too_many_arguments)]
    fn indexed(
        width: u32,
        height: u32,
        color_bits: u32,
        themes: Vec<(u8, (u8, u8, u8))>,
        palettes: Vec<Vec<(u16, u16, u16, u16)>>,
        frames: Vec<Vec<u64>>,
        delays: Option<Vec<u32>>,
        replay_count: u16,
    ) -> PyResult<Image> {
        let index_count = palettes.first().map_or(0, |p| p.len());
        // `for_count` may suggest an unsupported 32/64-bit width for a huge
        // explicit palette; `validate` below rejects it.
        let index_width = IndexWidth::for_count(index_count as u64);
        let palettes: Vec<Vec<Rgba>> = palettes
            .into_iter()
            .map(|p| {
                p.into_iter()
                    .map(|(r, g, b, a)| Rgba::new(r, g, b, a))
                    .collect()
            })
            .collect();
        let frames: Vec<Frame> = frames
            .into_iter()
            .enumerate()
            .map(|(i, indices)| Frame {
                delay_us: delays.as_ref().and_then(|d| d.get(i).copied()).unwrap_or(0),
                indices,
            })
            .collect();
        let inner = DifImage {
            width,
            height,
            color_depth: color_depth(color_bits)?,
            index_width,
            themes: build_themes(themes),
            palettes,
            frames,
            replay_count,
        };
        inner.validate().map_err(map_err)?;
        Ok(Image { inner, source_colors: None })
    }

    /// Build a single-theme (light) indexed image straight from a packed RGBA8
    /// buffer (`4 * width * height` bytes). The palette dedup + index build run in
    /// Rust, so Python hands over the raw bitmap instead of marshalling a per-pixel
    /// index list across the FFI boundary. Add a derived dark theme afterwards with
    /// [`Image::add_dark_theme`].
    ///
    /// `index_width` is `None` (auto-fit the smallest supported width, quantizing
    /// only when the source exceeds 16-bit), `8`, or `16` (force that width,
    /// quantizing down when the source has more colors than it can index). When the
    /// palette is quantized, [`Image::quantized`] is `True` and
    /// [`Image::source_colors`] reports the pre-quantization color count.
    #[staticmethod]
    #[pyo3(signature = (width, height, rgba, index_width=None))]
    fn indexed_from_rgba8(
        width: u32,
        height: u32,
        rgba: &[u8],
        index_width: Option<u32>,
    ) -> PyResult<Image> {
        let want = match index_width {
            None => None,
            Some(8) => Some(IndexWidth::Bit8),
            Some(16) => Some(IndexWidth::Bit16),
            Some(n) => {
                return Err(PyValueError::new_err(format!(
                    "index_width must be 8 or 16, got {n}"
                )))
            }
        };
        let (inner, source_colors) =
            indexed_from_rgba8(width, height, rgba, want).map_err(map_err)?;
        Ok(Image { inner, source_colors })
    }

    /// Derive a dark theme natively from theme 0's palette + base color and append
    /// it (abilities = dark). `strategy` is `"keep"`, `"invert"`, or
    /// `"arithmetic"`. No palette crosses the FFI boundary.
    fn add_dark_theme(&mut self, strategy: &str) -> PyResult<()> {
        let strat = Strategy::from_name(strategy).map_err(map_err)?;
        let depth = self.inner.color_depth;
        let light = self
            .inner
            .palettes
            .first()
            .ok_or_else(|| PyValueError::new_err("image has no source palette"))?;
        let dark = dif_core::derive_dark_palette(light, strat, depth);
        self.inner.palettes.push(dark);
        let base = self.inner.themes.first().map_or([0, 0, 0], |t| t.base_color);
        let dark_base = dif_core::derive_dark_base_color(base, strat);
        self.inner.themes.push(Theme {
            abilities: abilities::DARK,
            base_color: dark_base,
        });
        self.inner.validate().map_err(map_err)?;
        Ok(())
    }

    /// Encode to a compressed `.dif` container. `codec` is the outer whole-body
    /// codec; `palette_codec`/`frame_codec` compress the palette and frame sections
    /// (default `"store"` for the random-access layout). Each is a study variant
    /// string (e.g. `"zstd-3"`, `"brotli-11"`, `"lz4-fast1"`). `workers` > 0 runs
    /// the multithreaded zstd/brotli encoders; output is a standard container.
    #[pyo3(signature = (codec="zstd-3", palette_codec="store", frame_codec="store", workers=0))]
    fn to_dif<'py>(
        &self,
        py: Python<'py>,
        codec: &str,
        palette_codec: &str,
        frame_codec: &str,
        workers: u32,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let outer = self::codec(codec)?;
        let pal = self::codec(palette_codec)?;
        let frm = self::codec(frame_codec)?;
        let bytes = to_dif_workers(&self.inner, outer, pal, frm, workers).map_err(map_err)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Encode to a raw, uncompressed `.difr`.
    fn to_difr<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = to_difr(&self.inner).map_err(map_err)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Render `frame` under the theme matching `mode` + host `base_color` into
    /// packed RGBA8. Returns `(width, height, rgba_bytes)`.
    #[pyo3(signature = (mode="dark", base_color=(0, 0, 0), frame=0))]
    fn render<'py>(
        &self,
        py: Python<'py>,
        mode: &str,
        base_color: (u8, u8, u8),
        frame: usize,
    ) -> PyResult<(u32, u32, Bound<'py, PyBytes>)> {
        let base = [base_color.0, base_color.1, base_color.2];
        let buf = self
            .inner
            .render_rgba8(theme_tag(mode)?, base, frame)
            .map_err(map_err)?;
        Ok((self.inner.width, self.inner.height, PyBytes::new(py, &buf)))
    }

    /// Decode a `.dif` container. `workers` > 1 decodes frames in parallel
    /// (opt-in; default serial). The result is identical regardless of count.
    #[staticmethod]
    #[pyo3(signature = (data, workers=1))]
    fn from_dif(data: &[u8], workers: u32) -> PyResult<Image> {
        Ok(Image {
            inner: from_dif_workers(data, workers).map_err(map_err)?,
            source_colors: None,
        })
    }

    /// Decode a raw `.difr`.
    #[staticmethod]
    fn from_difr(data: &[u8]) -> PyResult<Image> {
        Ok(Image {
            inner: from_difr(data).map_err(map_err)?,
            source_colors: None,
        })
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
    fn color_bits(&self) -> u32 {
        match self.inner.color_depth {
            ColorDepth::Rgba8 => 8,
            ColorDepth::Rgba16 => 16,
        }
    }
    #[getter]
    fn index_bits(&self) -> u32 {
        (self.inner.index_width.bytes() * 8) as u32
    }
    /// `True` when the palette was reduced (OKLab-quantized) to fit the index
    /// width; `False` when the source fit losslessly.
    fn quantized(&self) -> bool {
        self.source_colors.is_some()
    }
    /// The source's unique-color count before quantization, or `None` if the
    /// palette was not quantized.
    #[getter]
    fn source_colors(&self) -> Option<u64> {
        self.source_colors
    }
    #[getter]
    fn frame_count(&self) -> usize {
        self.inner.frame_count()
    }
    #[getter]
    fn replay_count(&self) -> u16 {
        self.inner.replay_count
    }
    #[getter]
    fn themes(&self) -> Vec<(u8, (u8, u8, u8))> {
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
    let pal: Vec<Rgba> = colors
        .into_iter()
        .map(|(r, g, b, a)| Rgba::new(r, g, b, a))
        .collect();
    let out = dif_core::derive_dark_palette(&pal, strat, depth_for_max(max_value)?);
    Ok(out.into_iter().map(|c| (c.r, c.g, c.b, c.a)).collect())
}

/// Derive the dark theme's base color (RGB8) from a light base color under
/// `strategy`.
#[pyfunction]
fn derive_dark_base_color(base: (u8, u8, u8), strategy: &str) -> PyResult<(u8, u8, u8)> {
    let strat = Strategy::from_name(strategy).map_err(map_err)?;
    let out = dif_core::derive_dark_base_color([base.0, base.1, base.2], strat);
    Ok((out[0], out[1], out[2]))
}

#[pymodule]
fn dif(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Image>()?;
    m.add_function(wrap_pyfunction!(derive_dark_palette, m)?)?;
    m.add_function(wrap_pyfunction!(derive_dark_base_color, m)?)?;
    m.add_function(wrap_pyfunction!(validate_codec, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
