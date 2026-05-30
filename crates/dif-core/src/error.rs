use core::fmt;

/// Errors produced while encoding, decoding, or (de)serializing DIF data.
#[derive(Debug, PartialEq, Eq)]
pub enum DifError {
    /// Input bytes ended before a full structure could be read.
    UnexpectedEof,
    /// File magic did not match `DIFR` (raw) or `DIF1` (compressed).
    BadMagic([u8; 4]),
    /// Unsupported container/format version byte.
    BadVersion(u8),
    /// Unknown compression codec id in a `.dif` container.
    BadCodec(u8),
    /// A varint index used more than 4 bytes or had malformed continuation bits.
    BadVarint,
    /// Theme count was 0 or exceeded 128.
    BadThemeCount(usize),
    /// A field violated an invariant (e.g. palette length mismatch); carries a reason.
    Invalid(&'static str),
    /// The embedded compression library failed.
    CompressionFailed,
}

impl fmt::Display for DifError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DifError::UnexpectedEof => write!(f, "unexpected end of input"),
            DifError::BadMagic(m) => write!(f, "bad magic: {m:?}"),
            DifError::BadVersion(v) => write!(f, "unsupported version: {v}"),
            DifError::BadCodec(c) => write!(f, "unknown codec id: {c}"),
            DifError::BadVarint => write!(f, "malformed varint index"),
            DifError::BadThemeCount(n) => write!(f, "theme count {n} out of range 1..=128"),
            DifError::Invalid(why) => write!(f, "invalid DIF data: {why}"),
            DifError::CompressionFailed => write!(f, "compression/decompression failed"),
        }
    }
}

impl core::error::Error for DifError {}

pub type Result<T> = core::result::Result<T, DifError>;
