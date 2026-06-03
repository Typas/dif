//! WebAssembly decoder for DIF v3.
//!
//! Reuses `dif-core` unchanged. The host passes its preferred appearance
//! (`"light"` / `"dark"` / `"high-contrast"`, e.g. from
//! `matchMedia('(prefers-color-scheme: dark)')`) plus its background color, and
//! gets back packed RGBA8 ready for a `<canvas>` `ImageData`. The background color
//! tie-breaks between equally-capable themes (see `dif_core::DifImage::pick_theme`).

use dif_core::{abilities, from_dif, from_difr, DifImage, ThemeTag};
use wasm_bindgen::prelude::*;

fn theme_tag(s: &str) -> ThemeTag {
    match s {
        "dark" => ThemeTag::Dark,
        "high-contrast" | "high_contrast" => ThemeTag::HighContrast,
        _ => ThemeTag::Light,
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
        let inner = if data.starts_with(b"DIF3") {
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

    #[wasm_bindgen(getter, js_name = themeCount)]
    pub fn theme_count(&self) -> usize {
        self.inner.themes.len()
    }

    /// How many times to replay the animation: `0` = infinite, `1` = static.
    #[wasm_bindgen(getter, js_name = replayCount)]
    pub fn replay_count(&self) -> u16 {
        self.inner.replay_count
    }

    /// Display delay of `frame` in microseconds (`0` for static).
    #[wasm_bindgen(js_name = frameDelay)]
    pub fn frame_delay(&self, frame: usize) -> u32 {
        self.inner.frames.get(frame).map_or(0, |f| f.delay_us)
    }

    /// A human-readable, newline-joined description of each theme:
    /// `"light+dark #rrggbb"` (capabilities then base color), in file order.
    #[wasm_bindgen(js_name = themesDescription)]
    pub fn themes_description(&self) -> String {
        self.inner
            .themes
            .iter()
            .map(|t| {
                let mut caps: Vec<&str> = Vec::new();
                if t.abilities & abilities::LIGHT != 0 {
                    caps.push("light");
                }
                if t.abilities & abilities::DARK != 0 {
                    caps.push("dark");
                }
                if t.abilities & abilities::HIGH_CONTRAST != 0 {
                    caps.push("high-contrast");
                }
                let caps = if caps.is_empty() {
                    "none".to_string()
                } else {
                    caps.join("+")
                };
                let [r, g, b] = t.base_color;
                format!("{caps} #{r:02x}{g:02x}{b:02x}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render `frame` for the host `mode` and background color `(r, g, b)` as
    /// packed RGBA8 (`4*w*h` bytes).
    pub fn render(
        &self,
        mode: &str,
        r: u8,
        g: u8,
        b: u8,
        frame: usize,
    ) -> Result<Vec<u8>, JsValue> {
        self.inner
            .render_rgba8(theme_tag(mode), [r, g, b], frame)
            .map_err(to_js)
    }
}
