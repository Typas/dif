# DIF Format V3

Date: 2026-06-02

## Status

## Problem

### Varint makes parallelism hard

The compression will greatly reduce the differrence of the UTF-8 style encoding and constant k-bit encoding. The varint method might have little contribution to size, but it has increased the difficulty on parallelism greatly. 

### Multi-palette per frame adds extra weight

After the varint elimiated, it is not reasonable to have multiple palettes to minimize the index length. Moreover, the extra metadata also increases the entropy. Therefore, removing the multi-palette is reasonable when the index plane is already in k-bit.

## Goal
- better decoding/encoding performance
- well-defined palette matching

## Design Change

### Remove grayscale mode

- The grayscale mode will be removed, all has been in indexed mode.

### Constant length indexing

- metadata flags bit 0-1 regulate the index is in 8-bit (00), 16-bit (01), 32-bit (10), 64-bit (11).
- metadata flags bit 2-5 regluate the mapped color is in RGBA8 (0x0) or RGBA16 (0x1), others are reserved for future use.

### Other metadata change

- frame_count reduced to u16.

### Color palette ability definition

The color palette itself now contains "light", "dark", "high-contrast". For each ability, the definition of the tag is "this scheme is capable to display under the tagged theme". For "high-contrast", it is for "high-contrast"-capable or not. 
There are still 5 bits in reservation for capability. And then we have to define the base color of the scheme, in RGB8. Total 4 bytes for each palette.

### Color palette picking

The scheme picker should pick the most suitable (least non-capable) color scheme mapping based on the tag in the browser/system themes. 
When there are more than one scheme that are equally capable for the system theme, pick the nearest base color.

### Body Layout change

- flags: u8
- theme_count: u8 (offset = 1)
- frame_count: u16
- width: u32
- height: u32
- replay_count: u16 (how many times to replay; 0 = infinite, 1 = static)
- palette_offset: u16 (the offset of the first palette, from the file start in byte step)
- color_count: u64 (see indexing, constant u64 for alignment)
- frame_long_offset: u64 (the offset of the first frame, from the file start in 16 EB step)
- frame_offset: u64 (the offset of the first frame, from the file start in byte step)
- reserved: u64 (for future use)
- themes[theme_count]:
    - abilities: u8
    - base_color:
        - red: u8
        - green: u8
        - blue: u8
- post_theme_padding: {0, 4, 8, 12} Bytes to have alignment with 16 Bytes.
- palette[theme_count][color_count]: RGBA8/RGBA16/... (see mapped color)
- post_palette_padding: align to the multiple of 16 Bytes.
- frames[frame_count]:
    - delay: u32 (in us; 0 = static)
    - indexed_bitmap[width][height]: u8/u16/u32/u64
    - padding: align to the multiple of 16 Bytes.

#### Palette offsets

For selected palette with palette index `p`, the first palette with recorded offset `k`, the total color_count `c`, the color size `s`, the offset of the palette is $ k_i = k + p * c * s $ bytes.

#### Frame offsets

For selected frame with frame index `f`, the first frame with recorded large offset `l` and offset `k`, the width `w`, the height `h`, the index size `s`, the offset of the palette is $ k_i = l * 2^64 + k + f * ) $ bytes.

## Evaluation

- The compressed size should not bloat over 10%
- The encoding/decoding speed should be at most 10% slower
- The multi-thread version should have higher speed on decoding/rendering

