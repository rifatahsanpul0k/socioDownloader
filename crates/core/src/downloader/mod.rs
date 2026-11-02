//! Native media delivery, range downloading, and FFmpeg orchestration.

pub mod engine;
pub mod fs;
pub mod http;
pub mod types;

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("download cancelled")]
    Cancelled,
    #[error(transparent)]
    Filesystem(#[from] fs::FsError),
    #[error(transparent)]
    Http(#[from] http::HttpDownloadError),
    #[error("FFmpeg process failed: {stderr}")]
    Ffmpeg { stderr: String },
    #[error("could not start FFmpeg: {0}")]
    FfmpegSpawn(#[source] std::io::Error),
    #[error("FFmpeg output was not created at '{0}'")]
    MissingOutput(PathBuf),
    #[error("unsupported or empty media stream")]
    EmptyStream,
    #[error("download size mismatch: expected {expected} bytes, wrote {actual} bytes")]
    SizeMismatch { expected: u64, actual: u64 },
}

pub use engine::DownloadEngine;
pub use types::{DownloadPhase, DownloadProgress};
