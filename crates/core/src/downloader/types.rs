//! Progress and lifecycle types for downloads.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::Sender;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadPhase {
    Probing,
    Downloading,
    Muxing,
    Completed { output_path: PathBuf },
    Failed { error: String },
    Cancelled,
}

impl DownloadPhase {
    pub(crate) fn kind(&self) -> PhaseKind {
        match self {
            Self::Probing => PhaseKind::Probing,
            Self::Downloading => PhaseKind::Downloading,
            Self::Muxing => PhaseKind::Muxing,
            Self::Completed { .. } => PhaseKind::Completed,
            Self::Failed { .. } => PhaseKind::Failed,
            Self::Cancelled => PhaseKind::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhaseKind {
    Probing,
    Downloading,
    Muxing,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub phase: DownloadPhase,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_sec: f64,
    pub eta_secs: Option<u64>,
}

#[derive(Clone)]
pub(crate) struct ProgressReporter {
    inner: Arc<ProgressInner>,
}

struct ProgressInner {
    sender: Sender<DownloadProgress>,
    downloaded_bytes: AtomicU64,
    total_bytes: AtomicU64,
    started_at: Instant,
    last_emission: Mutex<Option<Instant>>,
    last_phase: Mutex<Option<PhaseKind>>,
}

impl ProgressReporter {
    pub(crate) fn new(sender: Sender<DownloadProgress>) -> Self {
        Self {
            inner: Arc::new(ProgressInner {
                sender,
                downloaded_bytes: AtomicU64::new(0),
                total_bytes: AtomicU64::new(0),
                started_at: Instant::now(),
                last_emission: Mutex::new(None),
                last_phase: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn add_total(&self, bytes: u64) {
        self.inner.total_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn add_downloaded(&self, bytes: u64) {
        self.inner
            .downloaded_bytes
            .fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn set_downloaded(&self, bytes: u64) {
        self.inner.downloaded_bytes.store(bytes, Ordering::Relaxed);
    }

    pub(crate) async fn emit(&self, phase: DownloadPhase, force: bool) {
        let kind = phase.kind();
        let transition = match self.inner.last_phase.lock() {
            Ok(mut last_phase) => {
                let transition = *last_phase != Some(kind);
                if transition {
                    *last_phase = Some(kind);
                }
                transition
            }
            Err(poisoned) => {
                let mut last_phase = poisoned.into_inner();
                let transition = *last_phase != Some(kind);
                if transition {
                    *last_phase = Some(kind);
                }
                transition
            }
        };
        let now = Instant::now();
        let elapsed_since_emit = match self.inner.last_emission.lock() {
            Ok(last) => last.map(|value| now.duration_since(value)),
            Err(poisoned) => poisoned.into_inner().map(|value| now.duration_since(value)),
        };
        if !force
            && !transition
            && elapsed_since_emit
                .map(|elapsed| elapsed < Duration::from_millis(80))
                .unwrap_or(false)
        {
            return;
        }

        let downloaded_bytes = self.inner.downloaded_bytes.load(Ordering::Relaxed);
        let total_raw = self.inner.total_bytes.load(Ordering::Relaxed);
        let total_bytes = (total_raw != 0).then_some(total_raw);
        let speed =
            downloaded_bytes as f64 / self.inner.started_at.elapsed().as_secs_f64().max(0.001);
        let eta_secs = total_bytes.and_then(|total| {
            if speed > 0.0 && downloaded_bytes < total {
                Some(((total - downloaded_bytes) as f64 / speed).ceil() as u64)
            } else {
                None
            }
        });
        let progress = DownloadProgress {
            phase,
            downloaded_bytes,
            total_bytes,
            speed_bytes_per_sec: speed,
            eta_secs,
        };
        if transition {
            let _ = self.inner.sender.send(progress).await;
            self.record_emission(now);
        } else if self.inner.sender.try_send(progress).is_ok() {
            self.record_emission(now);
        }
    }

    fn record_emission(&self, now: Instant) {
        match self.inner.last_emission.lock() {
            Ok(mut last) => *last = Some(now),
            Err(poisoned) => *poisoned.into_inner() = Some(now),
        }
    }
}
