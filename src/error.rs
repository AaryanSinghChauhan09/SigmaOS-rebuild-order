//! Error types for the rebuild-order component.

/// Errors that can occur while computing a rebuild order.
#[derive(Debug, thiserror::Error)]
pub enum RebuildError {
    /// A package was not found in the sync databases.
    #[error("package not found in syncdb")]
    PackageNotFound,

    /// The pacman database could not be read.
    #[error("pacman db error: {0}")]
    PacmanDb(String),

    /// An I/O error occurred (tar archive entries surface as io errors).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
