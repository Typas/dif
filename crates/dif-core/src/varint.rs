//! UTF-8-inspired variable-length integer used for palette indices.
//!
//! The scheme reuses the exact byte-length thresholds of UTF-8 so a palette
//! index is packed the way a Unicode scalar would be:
//!
//! | value range            | bytes |
//! |------------------------|-------|
//! | `0 ..= 127`            | 1     |
//! | `128 ..= 2_047`        | 2     |
//! | `2_048 ..= 65_535`     | 3     |
//! | `65_536 ..= 2_097_151` | 4     |
//!
//! The 4-byte form carries 21 bits, which covers the format's 1,112,064-color
//! ceiling (the count of valid Unicode scalars) and then some.

use alloc::vec::Vec;

use crate::error::{DifError, Result};

/// Largest index that fits in the 4-byte form (21 payload bits).
pub const MAX_INDEX: u32 = 0x1F_FFFF;

/// Append the UTF-8-style encoding of `value` to `out`.
pub fn write(out: &mut Vec<u8>, value: u32) {
    debug_assert!(value <= MAX_INDEX, "index {value} exceeds MAX_INDEX");
    match value {
        0..=0x7F => out.push(value as u8),
        0x80..=0x7FF => {
            out.push(0xC0 | (value >> 6) as u8);
            out.push(0x80 | (value & 0x3F) as u8);
        }
        0x800..=0xFFFF => {
            out.push(0xE0 | (value >> 12) as u8);
            out.push(0x80 | ((value >> 6) & 0x3F) as u8);
            out.push(0x80 | (value & 0x3F) as u8);
        }
        _ => {
            out.push(0xF0 | (value >> 18) as u8);
            out.push(0x80 | ((value >> 12) & 0x3F) as u8);
            out.push(0x80 | ((value >> 6) & 0x3F) as u8);
            out.push(0x80 | (value & 0x3F) as u8);
        }
    }
}

/// Read one UTF-8-style value starting at `bytes[*pos]`, advancing `*pos`.
pub fn read(bytes: &[u8], pos: &mut usize) -> Result<u32> {
    let b0 = *bytes.get(*pos).ok_or(DifError::UnexpectedEof)?;
    *pos += 1;
    let (mut value, extra) = match b0 {
        0x00..=0x7F => return Ok(b0 as u32),
        0xC0..=0xDF => ((b0 & 0x1F) as u32, 1),
        0xE0..=0xEF => ((b0 & 0x0F) as u32, 2),
        0xF0..=0xF7 => ((b0 & 0x07) as u32, 3),
        _ => return Err(DifError::BadVarint),
    };
    for _ in 0..extra {
        let b = *bytes.get(*pos).ok_or(DifError::UnexpectedEof)?;
        if b & 0xC0 != 0x80 {
            return Err(DifError::BadVarint);
        }
        value = (value << 6) | (b & 0x3F) as u32;
        *pos += 1;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_boundaries() {
        let cases = [
            0u32, 1, 127, 128, 2047, 2048, 65535, 65536, 1_112_063, 1_112_064, MAX_INDEX,
        ];
        for &v in &cases {
            let mut buf = Vec::new();
            write(&mut buf, v);
            let mut pos = 0;
            let got = read(&buf, &mut pos).unwrap();
            assert_eq!(got, v, "roundtrip {v}");
            assert_eq!(pos, buf.len(), "consumed all bytes for {v}");
        }
    }

    #[test]
    fn byte_lengths_match_utf8_thresholds() {
        let len = |v| {
            let mut b = Vec::new();
            write(&mut b, v);
            b.len()
        };
        assert_eq!(len(127), 1);
        assert_eq!(len(128), 2);
        assert_eq!(len(2047), 2);
        assert_eq!(len(2048), 3);
        assert_eq!(len(65535), 3);
        assert_eq!(len(65536), 4);
    }

    #[test]
    fn truncated_input_errors() {
        let mut buf = Vec::new();
        write(&mut buf, 65536); // 4 bytes
        buf.truncate(2);
        let mut pos = 0;
        assert_eq!(read(&buf, &mut pos), Err(DifError::UnexpectedEof));
    }

    #[test]
    fn bad_continuation_errors() {
        let buf = [0xE0, 0x00, 0x80]; // second byte missing 0b10 prefix
        let mut pos = 0;
        assert_eq!(read(&buf, &mut pos), Err(DifError::BadVarint));
    }
}
