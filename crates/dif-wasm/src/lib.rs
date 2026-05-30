//! WebAssembly decoder for DIF.
//!
//! Reuses `dif-core` unchanged. The host passes its preferred appearance
//! (`"light"` / `"dark"` / `"high-contrast"`, e.g. from
//! `matchMedia('(prefers-color-scheme: dark)')`) and gets back packed RGBA8
//! ready for a `<canvas>` `ImageData`.

use dif_core::{from_dif, from_difr, DifImage, ModeTag};
use wasm_bindgen::prelude::*;

fn mode_tag(s: &str) -> ModeTag {
    match s {
        "dark" => ModeTag::Dark,
        "high-contrast" | "high_contrast" => ModeTag::HighContrast,
        _ => ModeTag::Light,
    }
}

fn to_js<E: std::fmt::Display>(e: E) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// A decoded DIF image. Construct with [`Image::from_bytes`].
#[wasm_bindgen]
pub struct Image {
    inner: DifImage,
}

#[wasm_bindgen]
impl Image {
    /// Decode either a `.dif` (compressed) or `.difr` (raw) byte buffer.
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Result<Image, JsValue> {
        let inner = if data.starts_with(b"DIF1") {
            from_dif(data).map_err(to_js)?
        } else {
            from_difr(data).map_err(to_js)?
        };
        Ok(Image { inner })
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.inner.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.inner.height
    }

    #[wasm_bindgen(getter, js_name = frameCount)]
    pub fn frame_count(&self) -> usize {
        self.inner.frame_count()
    }

    /// Theme names joined by `\n`, in file order (JS-friendly, no array glue).
    #[wasm_bindgen(js_name = themeNames)]
    pub fn theme_names(&self) -> String {
        self.inner
            .themes
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render `frame` for the host `mode` as packed RGBA8 (`4*w*h` bytes).
    pub fn render(&self, mode: &str, frame: usize) -> Result<Vec<u8>, JsValue> {
        self.inner
            .render_rgba8(mode_tag(mode), frame)
            .map_err(to_js)
    }
}
