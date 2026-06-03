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
- less memory usage on large frames

## Design Change

### Remove grayscale mode

- The grayscale mode will be removed, all has been in indexed mode.

### Constant length indexing

- metadata flags bit 0-1 regulate the index is in 8-bit (00), 16-bit (01), 32-bit (10), 64-bit (11).
- metadata flags bit 2-5 regluate the mapped color is in RGBA8 (0x0) or RGBA16 (0x1), others are reserved for future use.

### Other metadata change

- frame_count reduced to u16.

### Color palette ability definition

The color palette itself now contains "light", "dark", "high-contrast".
For each ability, the definition of the tag is "this scheme is capable to display under the tagged theme".
For "high-contrast", it is for "high-contrast"-capable or not. 
There are still 5 bits in reservation for capability. And then we have to define the base color of the scheme, in RGB8.
Total 4 bytes for each palette.

### Color palette picking

The scheme picker should pick the most suitable (least non-capable) color scheme mapping based on the tag in the browser/system themes.
When there are more than one scheme that are equally capable for the system theme, pick the nearest base color.

### Containers change
- magic: "DIF3...."/"DIFR3...", 8 bytes with version
- codec: u8, 4 bits for codec and 4 bits for level mapping
- flags: u8
- codec_palette: u8, the codec-level pair for palette
- codec_frame: u8, the codec-level pair for frame
- theme_count: u8 (offset = 1)
- reserved: u8
- frame_count: u16
- replay_count: u16 (how many times to replay; 0 = infinite, 1 = static)
- reserved: u16
- width: u32
- height: u32
- compressed_frame_long_offset: u32 (the offset of first frame in bytes, the upper 32 bits)
- compressed_frame_offset: u64 (the offset of first frame in bytes, the lower 64 bits)
- compressed_frame_alignment: u64 (the size of each frame after compressed, every frame will have same size with a multiple of 16B)
- index_count: u64 (see indexing, constant u64 for alignment)
- reserved: u64
- compressed_body[]

### Body change
The image will have intermediate state.
- Encoding: Raw Body -> Encode (Palette + Frame) -> Intermediate Body -> Encode -> Compressed Body
- Decoding: Compressed Body -> Decode -> Intermediate Body -> Decode (Palette + Frame) -> Raw Body

#### The motivation of intermediate body
- The Body Layout will mainly explode on two parts: palette and the frames.
- Each theme will only use 1 palette, while it can store up to 256.
- It will only render one frame at the same time, unfold everything creates extra memory use.
- Decoding one palette: Intermediate Body -> Decode (Palette) sequentially -> Meet target palette -> Stop when target palette decoded.
- Decoding one frame: Intermediate Body -> Decode (Frame) by calculated offset -> One frame decoded.

#### Intermediate Body

- themes[theme_count]:
- - abilities: u8
- - base_color:
- - - red: u8
- - - green: u8
- - - blue: u8
- post_theme_padding: {0, 4, 8, 12} Bytes to have alignment with 16 Bytes.
- compressed_palettes[]
- post_palette_padding: align to the multiple of 16 Bytes.
- frames[frame_count]:
- - size: u64 (from the offset of this frame to the end of compressed_content, in Bytes)
- - delay: u32 (in us; 0 = static)
- - compressed_content[]
- - padding: align to frame_compressed_alignment

#### Fully decompressed body layout

- themes[theme_count]:
- - abilities: u8
- - base_color:
- - - red: u8
- - - green: u8
- - - blue: u8
- post_theme_padding: {0, 4, 8, 12} Bytes to have alignment with 16 Bytes.
- palettes[theme_count][index_count]: RGBA8/RGBA16/... (see mapped color)
- post_palette_padding: align to the multiple of 16 Bytes.
- frames[frame_count]:
- - delay: u32 (in us; 0 = static)
- - indexed_bitmap[width][height]: u8/u16/u32/u64
- - padding: align to the multiple of 16 Bytes.

##### Raw Size Range
- container: constant 64 bytes
- themes: from 4 bytes to 1024 bytes
- palettes: from 1 (theme) \* 2 (index) \* 1 (byte/index) = 16 bytes (with padding)
to 2^8 (theme) \* 2^64 (index) \* 8 (byte/index) = 2^75 bytes 
- frames: from 1 (frame) \* (4 + 1 (width) \* 1 (height) \* 1 (byte/index)) = 16 bytes (with padding)
to 2^16 (frame) \* (4 + 2^32 (width) \* 2^32 (height) \* 8 (byte/index)) ~ 2^83 bytes

##### Determine Raw Size from Metadata
Let `c` be index_count, `s` be the mapped color size, `t` be theme_count, `f` be frame count, `i` for index size.
Also, `w` is the width and `h` is the height.
- themes: $align(4 * t, 16)$ bytes
- palettes: $align(t * c * s, 16)$ bytes
- frames: $f * align(w * h * i, 16)$ bytes

#### Palette offsets
Let `c` be index_count, `s` be the mapped color size, `t` be theme_count, `f` be frame count, `i` for index size.
And let the palette index be `k`.
The offset of the palette `k` is $64 + align(4 * t, 16) + k * c * s$ bytes.

#### Frame offsets
Let `c` be index_count, `s` be the mapped color size, `t` be theme_count, `f` be frame count, `i` for index size.
And let the frame index be `j`.
Also, `w` is the width and `h` is the height.
The offset of the frame `j` is $64 + align(4 * t, 16) + align(t * c* s, 16) + j * align(w * h * i, 16)$ bytes.

#### Intermediate Palette offsets
Only the starting point of the palette can be defined.
Let `t` be the theme_count.
The offset is $64 + align(4 * t, 16)$ bytes.

#### Intermediate Frame offsets
Let `u` be the compressed_frame_long_offset, `l` be the compressed_frame_offset, `z` be the compressed_frame_alignment.
Let `j` be the frame index.
The offset is $u * 2^64 + l + j * z$ bytes.

## Evaluation

- The compressed size should not bloat over 10%
- The encoding/decoding speed should be at most 10% slower
- The multi-thread version should have higher speed on decoding/rendering

