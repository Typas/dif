use core::fmt;

/// Errors produced while encoding, decoding, or (de)serializing DIF data.
#[derive(Debug, PartialEq, Eq)]
pub enum DifError {
    /// Input bytes ended before a full structure could be read.
    UnexpectedEof,
    /// File magic did not match `DIF3` (compressed) or `DIFR3` (raw).
    BadMagic([u8; 8]),
    /// Unsupported container/format version byte.
    BadVersion(u8),
    /// Unknown or unsupported codec family / level in a container byte.
    BadCodec(u8),
    /// The flags byte requested an index width that is defined but unsupported
    /// (32-/64-bit) or otherwise invalid.
    BadIndexWidth(u8),
    /// The flags byte requested a mapped-color depth that is reserved/unknown.
    BadColorDepth(u8),
    /// Theme count was 0 or exceeded 256.
    BadThemeCount(usize),
    /// A theme abilities byte set a reserved bit.
    BadAbilities(u8),
    /// A header field that must be 16-byte aligned was not.
    Unaligned(u64),
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
            DifError::BadCodec(c) => write!(f, "unknown/unsupported codec byte: {c}"),
            DifError::BadIndexWidth(w) => write!(f, "unsupported index width bits: {w}"),
            DifError::BadColorDepth(d) => write!(f, "reserved/unknown color depth: {d}"),
            DifError::BadThemeCount(n) => write!(f, "theme count {n} out of range 1..=256"),
            DifError::BadAbilities(a) => write!(f, "theme abilities set a reserved bit: {a:#04x}"),
            DifError::Unaligned(o) => write!(f, "offset {o} is not 16-byte aligned"),
            DifError::Invalid(why) => write!(f, "invalid DIF data: {why}"),
            DifError::CompressionFailed => write!(f, "compression/decompression failed"),
        }
    }
}

impl core::error::Error for DifError {}

pub type Result<T> = core::result::Result<T, DifError>;
