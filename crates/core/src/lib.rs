//! Platform services for the rust-dl media downloader.

pub mod auth;
pub mod bundler;
pub mod clipboard;
pub mod downloader;
pub mod queue;
pub mod resolver;
pub mod sidecar;
pub mod storage;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Sidecar(#[from] sidecar::SidecarError),

    #[error(transparent)]
    Bundler(#[from] bundler::BundlerError),

    #[error(transparent)]
    Resolver(resolver::ResolverError),

    #[error("authentication required for {platform}: {message}")]
    AuthenticationRequired {
        platform: resolver::Platform,
        message: String,
    },

    #[error(transparent)]
    Download(#[from] downloader::DownloadError),

    #[error(transparent)]
    Queue(#[from] queue::QueueError),

    #[error(transparent)]
    Auth(#[from] auth::AuthError),

    #[error(transparent)]
    Storage(#[from] storage::StorageError),
}

pub use auth::{AuthError, BrowserKind, CookieSource};
pub use resolver::{MediaManifest, MediaStream, Platform, Resolver, StreamDelivery};
pub use sidecar::{
    create_default_client, BinaryVersions, DownloadProgress, SidecarError, SidecarManager,
    SidecarUpdateReport, FFMPEG_BINARY_NAME, YT_DLP_BINARY_NAME,
};
pub use storage::StorageError;

impl From<resolver::ResolverError> for CoreError {
    fn from(error: resolver::ResolverError) -> Self {
        match error {
            resolver::ResolverError::AuthenticationRequired { platform, message } => {
                Self::AuthenticationRequired { platform, message }
            }
            other => Self::Resolver(other),
        }
    }
}
