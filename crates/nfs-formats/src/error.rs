use thiserror::Error;

/// Errors produced by all `nfs-formats` parsers.
///
/// Every variant carries enough context (offset, structure, expected/actual) to locate the
/// problem in the original file without a debugger.
#[derive(Debug, Error)]
pub enum FormatError {
    #[error("{what}: unknown magic at offset {offset:#x}: expected {expected:?}, found {found:?}")]
    UnknownMagic { what: &'static str, offset: usize, expected: Vec<u8>, found: Vec<u8> },

    #[error("{what}: unsupported version {found} (supported: {supported})")]
    UnsupportedVersion { what: &'static str, found: u32, supported: &'static str },

    #[error("{what}: file truncated at offset {offset:#x} (needed {needed} bytes)")]
    Truncated { what: &'static str, offset: usize, needed: usize },

    #[error("{what}: invalid offset {offset:#x} (file length {len:#x})")]
    InvalidOffset { what: &'static str, offset: usize, len: usize },

    #[error("{what}: unexpected value at offset {offset:#x}: expected {expected}, found {found}")]
    UnexpectedValue { what: &'static str, offset: usize, expected: String, found: String },

    #[error("{what}: unknown record type {code:#x} at offset {offset:#x}")]
    UnknownRecordType { what: &'static str, offset: usize, code: u32 },

    #[error("RefPack stream at offset {offset:#x}: {reason}")]
    RefPack { offset: usize, reason: &'static str },

    #[error("{what}: declared size {declared} does not match actual {actual}")]
    SizeMismatch { what: &'static str, declared: usize, actual: usize },
}
