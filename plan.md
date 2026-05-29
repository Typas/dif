# Modern Diagram Image Format (.dif)

## Motivation
The legacy format GIF contains only 256 colors, while the diagram usually uses less than 256 colors.
Recent website has supported the "dark mode", but the images stay constant. When it comes to the screenshot of terminal or website. The display usually result in hyper-contrast or invisible that makes my eyes sick.

## The Format
Inspired by GIF, use color palette to create mapping. One new thing, color theme -- the named color palette.

### The Mapping

#### UTF-8 Mode
- UTF-8 style variable length color: 128-Color for 1 Byte, 2048-Color for 2B, 65536-Color for 3B, 1112064-Color for 4B.
- The color themes: up to 128 themes. Each color should have the corresponding color (8-bit-RGBA or 16-bit-RGBA) for each theme.
- The default theme is the first theme.
- Color palette size lower bound: 1 (theme) \* 128 (color) \* 4 bytes (8-bit-RGBA) + 128 \* 1 byte = 640 bytes.
- Color palette size upper bound: 128 (themes) \* 1112064 (color) \* 8 bytes (16-bit-RGBA) + 128 \* 1 byte + 1920 \* 2 bytes + 61440 \* 3 bytes + 1048576 \* 4 bytes = 1143136128 bytes = 1.06 GB.

#### Grayscale Mode
- Supports 8-bit and 16-bit grayscale.
- No alpha channel, no palette.

### The frame
- The frame processing is mimicking APNG or GIF.

### The compression
- Lossless compression, need codecs for testing the speed and size.
- Evaluation: $ M = log(5S/4) - log(C/4) - log(D) $, where $S = Size_Original / Size_Compressed$, $C = Memcpy_Speed/Compress_Speed$, $D = Memcpy_Speed/Decompress_Speed$. Higher is better.
- Candidates: libdeflate level 6 (baseline); brotli level 5, 11; bzip3 level 5; kanzi level 1, 2; lz4hc level 4, 9; lz4 fast level 1; lzav level 1; zstd fast level 1; zstd level 3, 22.

## Implementation

### Benchmarking codec
- A specialized extension `.difr`, the raw dif without any compression.
- The `.drawio` to `.dif` converter should implement the convertion to `.difr` first.

#### Standard files
Get the examples from https://www.drawio.com/docs/diagram-types/ . Need to cite this.

### The Codec of DIF 
See the specifications of The Format.

### The .drawio (diagram-specific xml)  to .dif converter
First generate png with the code from [drawio](https://github.com/jgraph/drawio/tree/v30.0.4).
And then as the general image to `.dif` converter.

### The general image to DIF converter
- Python script for reading the image
- DIF Codec to encode the image into `.dif`

### The Wasm Decoder
- Reuse the codec
- It will respect the "theme" provided by the browser.

### The VS Codium Extension
- The image display should change with respect to the theme, or the mode.

## Evaluation
1. The size comparison with the typical png, loseless jxl, loseless webp, loseless avif.
2. The theme-matching change.
3. The speed of encoding and decoding, with python benchmark code. Compare with gif, png, jxl, webp, avif.
For the existing codecs, use the existing library. Do not reinvent the wheel.
