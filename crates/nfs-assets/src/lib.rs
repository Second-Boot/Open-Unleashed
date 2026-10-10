//! Locating and loading original game assets (read-only) into engine-level types.
//!
//! This crate is the only place that knows both the original file formats (`nfs-formats`) and
//! the engine's data types; everything above it (renderer, physics, game) consumes the converted
//! structures.
pub mod car;
pub mod mesh;
pub mod paths;
pub mod platform;
pub mod texture;
pub mod track;

pub use paths::GamePaths;

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("{0}")]
    NotFound(#[from] paths::GameNotFound),
    #[error("missing file {name} in {dir}")]
    MissingFile { dir: std::path::PathBuf, name: String },
    #[error("{path}: {source}")]
    Io { path: std::path::PathBuf, #[source] source: std::io::Error },
    /// A decode worker thread panicked (a corrupt pack); the panic is reported, not re-raised.
    #[error("{path}: a texture decode worker panicked")]
    Worker { path: std::path::PathBuf },
    #[error("{path}: {source}")]
    Format { path: std::path::PathBuf, #[source] source: nfs_formats::FormatError },
}

pub(crate) fn read(path: &std::path::Path) -> Result<Vec<u8>, AssetError> {
    std::fs::read(path).map_err(|source| AssetError::Io { path: path.to_path_buf(), source })
}

/// Read a game file and unpack it when it is a RefPack stream — the pair of lines every loader
/// wrote out by hand. An uncompressed file is not copied a second time.
pub(crate) fn read_pack(path: &std::path::Path) -> Result<Vec<u8>, AssetError> {
    nfs_formats::refpack::decompress_vec(read(path)?).map_err(fmt_err(path))
}

pub(crate) fn fmt_err(path: &std::path::Path) -> impl FnOnce(nfs_formats::FormatError) -> AssetError + '_ {
    move |source| AssetError::Format { path: path.to_path_buf(), source }
}
