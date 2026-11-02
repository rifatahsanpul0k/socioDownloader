//! Queue domain types and observable job events.

use std::path::PathBuf;

use crate::downloader::DownloadProgress;
use crate::resolver::{MediaManifest, Platform};

pub type JobId = u64;

#[derive(Debug, Clone)]
pub enum JobStatus {
    Queued,
    Resolving,
    Ready { manifest: MediaManifest },
    Downloading { progress: DownloadProgress },
    Completed { output_path: PathBuf },
    Failed { error: String },
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct QueueJob {
    pub id: JobId,
    pub url: String,
    pub platform: Platform,
    pub title: Option<String>,
    pub selected_format_id: Option<String>,
    pub output_dir: PathBuf,
    pub status: JobStatus,
}

#[derive(Debug, Clone)]
pub enum QueueEvent {
    Updated(Box<QueueJob>),
    Removed(JobId),
}
