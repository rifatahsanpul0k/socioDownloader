//! Concurrent, observable download queue.

mod types;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::{broadcast, watch, Mutex, RwLock, Semaphore};

use crate::auth::CookieSource;
use crate::downloader::{DownloadEngine, DownloadError};
use crate::resolver::sanitizer::{extract_batch_urls, sanitize_url};
use crate::resolver::{MediaManifest, MediaStream, Platform, Resolver, StreamDelivery};

pub use types::{JobId, JobStatus, QueueEvent, QueueJob};

#[derive(Debug, Error)]
pub enum QueueError {
    #[error(transparent)]
    Resolver(#[from] crate::resolver::ResolverError),
    #[error("job {0} was not found")]
    JobNotFound(JobId),
    #[error("job {0} is not ready for download")]
    NotReady(JobId),
    #[error("format '{format_id}' was not found in job {job_id}")]
    FormatNotFound { job_id: JobId, format_id: String },
    #[error("a job with ID {0} is already active")]
    AlreadyActive(JobId),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("batch resolution failed: {0}")]
    Resolution(String),
}

#[derive(Clone)]
pub struct QueueManager {
    inner: Arc<QueueInner>,
}

struct QueueInner {
    next_id: AtomicU64,
    jobs: RwLock<HashMap<JobId, QueueJob>>,
    cancellations: Mutex<HashMap<JobId, watch::Sender<bool>>>,
    permits: RwLock<Arc<Semaphore>>,
    cookie_source: RwLock<CookieSource>,
    resolver: Resolver,
    engine: DownloadEngine,
    output_dir: PathBuf,
    events: broadcast::Sender<QueueEvent>,
}

impl QueueManager {
    pub fn new(
        resolver: Resolver,
        engine: DownloadEngine,
        output_dir: PathBuf,
        max_concurrent_downloads: usize,
    ) -> Self {
        let (events, _) = broadcast::channel(128);
        Self {
            inner: Arc::new(QueueInner {
                next_id: AtomicU64::new(1),
                jobs: RwLock::new(HashMap::new()),
                cancellations: Mutex::new(HashMap::new()),
                permits: RwLock::new(Arc::new(Semaphore::new(
                    max_concurrent_downloads.clamp(1, 8),
                ))),
                cookie_source: RwLock::new(CookieSource::None),
                resolver,
                engine,
                output_dir,
                events,
            }),
        }
    }

    pub fn default_concurrency(
        resolver: Resolver,
        engine: DownloadEngine,
        output_dir: PathBuf,
    ) -> Self {
        Self::new(resolver, engine, output_dir, 3)
    }

    pub async fn set_cookie_source(&self, cookie_source: CookieSource) {
        *self.inner.cookie_source.write().await = cookie_source;
    }

    pub async fn cookie_source(&self) -> CookieSource {
        self.inner.cookie_source.read().await.clone()
    }

    pub async fn set_max_concurrent_downloads(&self, max_concurrent_downloads: usize) {
        let permits = Arc::new(Semaphore::new(max_concurrent_downloads.clamp(1, 8)));
        *self.inner.permits.write().await = permits;
    }

    pub fn subscribe(&self) -> broadcast::Receiver<QueueEvent> {
        self.inner.events.subscribe()
    }

    pub async fn snapshot(&self) -> Vec<QueueJob> {
        self.inner.jobs.read().await.values().cloned().collect()
    }

    pub async fn enqueue_auto(
        &self,
        url: impl Into<String>,
        audio_only: bool,
    ) -> Result<JobId, QueueError> {
        self.enqueue_auto_limited(url.into(), audio_only, None)
            .await
    }

    pub async fn enqueue_auto_max(
        &self,
        url: impl Into<String>,
        max_height: u32,
        audio_only: bool,
    ) -> Result<JobId, QueueError> {
        self.enqueue_auto_limited(url.into(), audio_only, Some(max_height))
            .await
    }

    pub async fn enqueue_batch_auto(
        &self,
        input: impl Into<String>,
        max_height: Option<u32>,
        audio_only: bool,
        allow_playlist: bool,
    ) -> Result<Vec<JobId>, QueueError> {
        let urls = extract_batch_urls(&input.into());
        if urls.is_empty() {
            return Err(QueueError::Resolution(
                "no valid media URLs were found in the input".to_owned(),
            ));
        }
        let cookies = self.inner.cookie_source.read().await.clone();
        let mut ids = Vec::new();
        for url in urls {
            let manifests = self
                .inner
                .resolver
                .resolve_all(&url, &cookies, allow_playlist)
                .await
                .map_err(|error| QueueError::Resolution(error.to_string()))?;
            for manifest in manifests {
                let job = self.insert_job(url.clone(), manifest.platform).await;
                self.set_manifest(job.id, manifest.clone()).await;
                let stream = choose_stream(&manifest, audio_only, max_height).ok_or_else(|| {
                    QueueError::Resolution("no stream matched the requested quality".to_owned())
                })?;
                self.prepare_manifest_download(job.id, manifest, stream)
                    .await;
                ids.push(job.id);
            }
        }
        Ok(ids)
    }

    pub async fn enqueue_batch_for_selection(
        &self,
        input: impl Into<String>,
        allow_playlist: bool,
    ) -> Result<Vec<JobId>, QueueError> {
        let urls = extract_batch_urls(&input.into());
        if urls.is_empty() {
            return Err(QueueError::Resolution(
                "no valid media URLs were found in the input".to_owned(),
            ));
        }
        let cookies = self.inner.cookie_source.read().await.clone();
        let mut ids = Vec::new();
        for url in urls {
            let manifests = self
                .inner
                .resolver
                .resolve_all(&url, &cookies, allow_playlist)
                .await
                .map_err(|error| QueueError::Resolution(error.to_string()))?;
            for manifest in manifests {
                let job = self.insert_job(url.clone(), manifest.platform).await;
                self.set_manifest(job.id, manifest).await;
                ids.push(job.id);
            }
        }
        Ok(ids)
    }

    async fn enqueue_auto_limited(
        &self,
        url: String,
        audio_only: bool,
        max_height: Option<u32>,
    ) -> Result<JobId, QueueError> {
        let sanitized = sanitize_url(&url)?;
        let job = self.insert_job(url, sanitized.platform).await;
        self.spawn_resolution(job.id, true, audio_only, max_height);
        Ok(job.id)
    }

    pub async fn resolve_for_selection(&self, url: impl Into<String>) -> Result<JobId, QueueError> {
        let url = url.into();
        let sanitized = sanitize_url(&url)?;
        let job = self.insert_job(url, sanitized.platform).await;
        self.spawn_resolution(job.id, false, false, None);
        Ok(job.id)
    }

    pub fn start_job(&self, id: JobId, format_id: Option<String>) -> Result<(), QueueError> {
        let manager = self.clone();
        tokio::spawn(async move {
            let result = manager.start_ready_job(id, format_id).await;
            if let Err(error) = result {
                manager.fail_job(id, error.to_string()).await;
            }
        });
        Ok(())
    }

    pub fn cancel_job(&self, id: JobId) -> Result<(), QueueError> {
        let manager = self.clone();
        tokio::spawn(async move {
            let sender = manager.inner.cancellations.lock().await.get(&id).cloned();
            if let Some(sender) = sender {
                let _ = sender.send(true);
                manager.set_status(id, JobStatus::Cancelled).await;
            } else {
                let exists = manager.inner.jobs.read().await.contains_key(&id);
                if exists {
                    manager.set_status(id, JobStatus::Cancelled).await;
                } else {
                    manager
                        .fail_job(id, QueueError::JobNotFound(id).to_string())
                        .await;
                }
            }
        });
        Ok(())
    }

    pub fn retry_job(&self, id: JobId) -> Result<(), QueueError> {
        let manager = self.clone();
        tokio::spawn(async move {
            let job = manager.inner.jobs.read().await.get(&id).cloned();
            let Some(job) = job else {
                manager
                    .fail_job(id, QueueError::JobNotFound(id).to_string())
                    .await;
                return;
            };
            if !matches!(job.status, JobStatus::Failed { .. } | JobStatus::Cancelled) {
                manager
                    .fail_job(id, QueueError::AlreadyActive(id).to_string())
                    .await;
                return;
            }
            manager.set_status(id, JobStatus::Queued).await;
            manager.spawn_resolution(id, true, false, None);
        });
        Ok(())
    }

    pub fn remove_job(&self, id: JobId) -> Result<(), QueueError> {
        let manager = self.clone();
        tokio::spawn(async move {
            if let Some(sender) = manager.inner.cancellations.lock().await.remove(&id) {
                let _ = sender.send(true);
            }
            let removed = manager.inner.jobs.write().await.remove(&id).is_some();
            if removed {
                let _ = manager.inner.events.send(QueueEvent::Removed(id));
            }
        });
        Ok(())
    }

    pub fn clear_completed(&self) -> Result<(), QueueError> {
        let manager = self.clone();
        tokio::spawn(async move {
            let completed = manager
                .inner
                .jobs
                .read()
                .await
                .values()
                .filter(|job| matches!(job.status, JobStatus::Completed { .. }))
                .map(|job| job.id)
                .collect::<Vec<_>>();
            for id in completed {
                let _ = manager.remove_job(id);
            }
        });
        Ok(())
    }

    pub fn cancel_all(&self) -> Result<(), QueueError> {
        let manager = self.clone();
        tokio::spawn(async move {
            let ids = manager
                .inner
                .jobs
                .read()
                .await
                .values()
                .filter(|job| {
                    !matches!(
                        job.status,
                        JobStatus::Completed { .. } | JobStatus::Cancelled
                    )
                })
                .map(|job| job.id)
                .collect::<Vec<_>>();
            for id in ids {
                let _ = manager.cancel_job(id);
            }
        });
        Ok(())
    }

    async fn insert_job(&self, url: String, platform: Platform) -> QueueJob {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let job = QueueJob {
            id,
            url,
            platform,
            title: None,
            selected_format_id: None,
            output_dir: self.inner.output_dir.clone(),
            status: JobStatus::Queued,
        };
        self.inner.jobs.write().await.insert(id, job.clone());
        let _ = self
            .inner
            .events
            .send(QueueEvent::Updated(Box::new(job.clone())));
        job
    }

    fn spawn_resolution(
        &self,
        id: JobId,
        auto_start: bool,
        audio_only: bool,
        max_height: Option<u32>,
    ) {
        let manager = self.clone();
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        tokio::spawn(async move {
            manager
                .inner
                .cancellations
                .lock()
                .await
                .insert(id, cancel_sender);
            manager.set_status(id, JobStatus::Resolving).await;
            let job = manager.inner.jobs.read().await.get(&id).cloned();
            let Some(job) = job else {
                manager.remove_cancellation(id).await;
                return;
            };
            let cookie_source = manager.inner.cookie_source.read().await.clone();
            match manager
                .inner
                .resolver
                .resolve_with_cookies(&job.url, Some(&cookie_source))
                .await
            {
                Ok(manifest) => {
                    if *cancel_receiver.borrow() {
                        manager.set_status(id, JobStatus::Cancelled).await;
                        manager.remove_cancellation(id).await;
                        return;
                    }
                    manager.set_manifest(id, manifest.clone()).await;
                    if auto_start {
                        let stream = choose_stream(&manifest, audio_only, max_height);
                        if let Some(stream) = stream {
                            manager
                                .download_stream(id, manifest, stream, cancel_receiver)
                                .await;
                        } else {
                            manager
                                .fail_job(id, "no stream matched the requested quality".to_owned())
                                .await;
                            manager.remove_cancellation(id).await;
                        }
                    } else {
                        manager.remove_cancellation(id).await;
                    }
                }
                Err(error) => {
                    manager.fail_job(id, resolver_error_message(error)).await;
                    manager.remove_cancellation(id).await;
                }
            }
        });
    }

    async fn set_manifest(&self, id: JobId, manifest: MediaManifest) {
        let mut jobs = self.inner.jobs.write().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.title = Some(manifest.title.clone());
            job.status = JobStatus::Ready { manifest };
            let _ = self
                .inner
                .events
                .send(QueueEvent::Updated(Box::new(job.clone())));
        }
    }

    async fn set_selected_format(&self, id: JobId, format_id: Option<String>) {
        let mut jobs = self.inner.jobs.write().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.selected_format_id = format_id;
            let _ = self
                .inner
                .events
                .send(QueueEvent::Updated(Box::new(job.clone())));
        }
    }

    async fn start_ready_job(
        &self,
        id: JobId,
        format_id: Option<String>,
    ) -> Result<(), QueueError> {
        let job = self
            .inner
            .jobs
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or(QueueError::JobNotFound(id))?;
        let JobStatus::Ready { manifest } = job.status else {
            return Err(QueueError::NotReady(id));
        };
        let chosen = format_id.or(job.selected_format_id);
        let selected_format_id = chosen.clone();
        let stream = match chosen {
            Some(format_id) if format_id != "best" => manifest
                .streams
                .iter()
                .find(|stream| stream.format_id == format_id),
            _ => manifest.streams.first(),
        }
        .ok_or_else(|| QueueError::FormatNotFound {
            job_id: id,
            format_id: selected_format_id.unwrap_or_else(|| "best".to_owned()),
        })?
        .clone();
        self.set_selected_format(id, Some(stream.format_id.clone()))
            .await;
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        self.inner
            .cancellations
            .lock()
            .await
            .insert(id, cancel_sender);
        self.download_stream(id, manifest, stream, cancel_receiver)
            .await;
        Ok(())
    }

    async fn download_stream(
        &self,
        id: JobId,
        manifest: MediaManifest,
        stream: MediaStream,
        cancel_receiver: watch::Receiver<bool>,
    ) {
        let permits = self.inner.permits.read().await.clone();
        let permit = tokio::select! {
            permit = permits.acquire_owned() => permit.ok(),
            changed = wait_for_cancel(cancel_receiver.clone()) => {
                let _ = changed;
                None
            }
        };
        let Some(permit) = permit else {
            self.set_status(id, JobStatus::Cancelled).await;
            self.remove_cancellation(id).await;
            return;
        };
        let (progress_sender, mut progress_receiver) = tokio::sync::mpsc::channel(64);
        let job = self.inner.jobs.read().await.get(&id).cloned();
        let Some(job) = job else {
            drop(permit);
            self.remove_cancellation(id).await;
            return;
        };
        let output_dir = job.output_dir.clone();
        let engine = self.inner.engine.clone();
        let mut download = Box::pin(engine.download(
            &manifest,
            &stream,
            Some(&output_dir),
            cancel_receiver,
            progress_sender,
        ));
        let result = loop {
            tokio::select! {
                result = &mut download => break result,
                progress = progress_receiver.recv() => {
                    if let Some(progress) = progress {
                        self.set_status(id, JobStatus::Downloading { progress }).await;
                    }
                }
            }
        };
        drop(permit);
        self.remove_cancellation(id).await;
        match result {
            Ok(path) => {
                self.set_status(id, JobStatus::Completed { output_path: path })
                    .await
            }
            Err(DownloadError::Cancelled) => self.set_status(id, JobStatus::Cancelled).await,
            Err(error) => self.fail_job(id, error.to_string()).await,
        }
    }

    async fn prepare_manifest_download(
        &self,
        id: JobId,
        manifest: MediaManifest,
        stream: MediaStream,
    ) {
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        self.inner
            .cancellations
            .lock()
            .await
            .insert(id, cancel_sender);
        let manager = self.clone();
        tokio::spawn(async move {
            manager
                .download_stream(id, manifest, stream, cancel_receiver)
                .await;
        });
    }

    async fn remove_cancellation(&self, id: JobId) {
        self.inner.cancellations.lock().await.remove(&id);
    }

    async fn fail_job(&self, id: JobId, error: String) {
        self.set_status(id, JobStatus::Failed { error }).await;
    }

    async fn set_status(&self, id: JobId, status: JobStatus) {
        let mut jobs = self.inner.jobs.write().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.status = status;
            let _ = self
                .inner
                .events
                .send(QueueEvent::Updated(Box::new(job.clone())));
        }
    }
}

async fn wait_for_cancel(mut receiver: watch::Receiver<bool>) -> bool {
    if *receiver.borrow() {
        return true;
    }
    receiver.changed().await.is_ok() && *receiver.borrow()
}

fn is_pure_audio_stream(stream: &MediaStream) -> bool {
    let no_vcodec = stream
        .vcodec
        .as_deref()
        .map(|v| v == "none" || v.is_empty())
        .unwrap_or(true);
    let has_acodec = stream
        .acodec
        .as_deref()
        .map(|a| a != "none" && !a.is_empty())
        .unwrap_or(false);
    let audio_container = matches!(
        stream.container.as_str(),
        "m4a" | "mp3" | "aac" | "opus" | "ogg" | "flac" | "wav"
    );
    (no_vcodec && has_acodec) || (no_vcodec && audio_container)
}

fn choose_stream(
    manifest: &MediaManifest,
    audio_only: bool,
    max_height: Option<u32>,
) -> Option<MediaStream> {
    if audio_only {
        // 1. Dedicated audio-only stream (preferring Apple-compatible m4a / AAC)
        let pure_audio_streams: Vec<&MediaStream> = manifest
            .streams
            .iter()
            .filter(|stream| is_pure_audio_stream(stream))
            .collect();

        if let Some(preferred) = pure_audio_streams.iter().find(|s| {
            s.container == "m4a"
                || s.acodec
                    .as_deref()
                    .map(|a| a.contains("aac") || a.contains("mp4a"))
                    .unwrap_or(false)
        }) {
            return Some((*preferred).clone());
        }

        if let Some(first_audio) = pure_audio_streams.first() {
            return Some((*first_audio).clone());
        }

        // 2. SplitDash stream: extract the audio stream from audio_url directly
        for stream in &manifest.streams {
            if let StreamDelivery::SplitDash { audio_url, .. } = &stream.delivery {
                return Some(MediaStream {
                    format_id: format!("{}_audio", stream.format_id),
                    delivery: StreamDelivery::Direct {
                        url: audio_url.clone(),
                    },
                    container: "m4a".to_owned(),
                    resolution: None,
                    fps: None,
                    filesize_bytes: None,
                    vcodec: None,
                    acodec: stream.acodec.clone().or_else(|| Some("aac".to_owned())),
                    headers: stream.headers.clone(),
                });
            }
        }

        // 3. Multiplexed video with audio (e.g. Instagram Reels, Facebook, Pinterest)
        // Mark container as m4a so downloader engine extracts the audio cleanly
        for stream in &manifest.streams {
            let has_audio = stream
                .acodec
                .as_deref()
                .map(|a| a != "none" && !a.is_empty())
                .unwrap_or(true);
            if has_audio && stream.container != "jpg" && stream.container != "png" {
                let mut audio_derived = stream.clone();
                audio_derived.container = "m4a".to_owned();
                return Some(audio_derived);
            }
        }

        // 4. Fallback: First stream if available
        if let Some(first) = manifest.streams.first() {
            let mut audio_derived = first.clone();
            audio_derived.container = "m4a".to_owned();
            return Some(audio_derived);
        }

        return None;
    }

    if let Some(max_height) = max_height {
        if let Some(stream) = manifest.streams.iter().find(|stream| {
            stream
                .resolution
                .as_deref()
                .and_then(resolution_height)
                .map(|height| height <= max_height)
                .unwrap_or(false)
                && stream.vcodec.is_some()
        }) {
            return Some(stream.clone());
        }
    }
    manifest.streams.first().cloned()
}

fn resolution_height(value: &str) -> Option<u32> {
    let value = value.to_ascii_lowercase();
    if value.contains("4k") {
        return Some(2160);
    }
    value
        .split('x')
        .next_back()
        .unwrap_or_default()
        .trim_end_matches('p')
        .parse()
        .ok()
}

fn resolver_error_message(error: crate::resolver::ResolverError) -> String {
    match error {
        crate::resolver::ResolverError::AuthenticationRequired { platform, message } => {
            format!("Authentication required for {platform}: {message}")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::StreamDelivery;

    #[test]
    fn selects_audio_and_quality_limited_streams() {
        let manifest = MediaManifest {
            id: "id".to_owned(),
            title: "title".to_owned(),
            uploader: "uploader".to_owned(),
            duration_secs: None,
            thumbnail_url: None,
            platform: Platform::YouTube,
            streams: vec![
                stream("1080", Some("1920x1080"), Some("h264"), Some("aac")),
                stream("720", Some("1280x720"), Some("h264"), Some("aac")),
                stream("audio", None, None, Some("aac")),
            ],
        };
        assert_eq!(
            choose_stream(&manifest, true, None).map(|stream| stream.format_id),
            Some("audio".to_owned())
        );
        assert_eq!(
            choose_stream(&manifest, false, Some(720)).map(|stream| stream.format_id),
            Some("720".to_owned())
        );
    }

    #[test]
    fn selects_audio_when_vcodec_is_some_none_or_splitdash() {
        // Case 1: yt-dlp style format where vcodec is "none" string
        let manifest_none_str = MediaManifest {
            id: "id1".to_owned(),
            title: "title".to_owned(),
            uploader: "uploader".to_owned(),
            duration_secs: None,
            thumbnail_url: None,
            platform: Platform::YouTube,
            streams: vec![
                stream("1080", Some("1920x1080"), Some("h264"), Some("aac")),
                stream("audio-140", None, Some("none"), Some("mp4a.40.2")),
            ],
        };
        assert_eq!(
            choose_stream(&manifest_none_str, true, None).map(|s| s.format_id),
            Some("audio-140".to_owned())
        );

        // Case 2: Only SplitDash stream available
        let splitdash_stream = MediaStream {
            format_id: "137+140".to_owned(),
            delivery: StreamDelivery::SplitDash {
                video_url: "https://example.test/video".to_owned(),
                audio_url: "https://example.test/audio".to_owned(),
            },
            container: "mp4".to_owned(),
            resolution: Some("1920x1080".to_owned()),
            fps: Some(30.0),
            filesize_bytes: Some(100),
            vcodec: Some("h264".to_owned()),
            acodec: Some("aac".to_owned()),
            headers: HashMap::new(),
        };
        let manifest_split = MediaManifest {
            id: "id2".to_owned(),
            title: "title".to_owned(),
            uploader: "uploader".to_owned(),
            duration_secs: None,
            thumbnail_url: None,
            platform: Platform::YouTube,
            streams: vec![splitdash_stream],
        };
        let audio_res = choose_stream(&manifest_split, true, None);
        assert!(audio_res.is_some());
        let a = audio_res.unwrap();
        assert_eq!(a.container, "m4a");
        assert!(matches!(a.delivery, StreamDelivery::Direct { .. }));

        // Case 3: Multiplexed Instagram video
        let manifest_insta = MediaManifest {
            id: "id3".to_owned(),
            title: "reel".to_owned(),
            uploader: "uploader".to_owned(),
            duration_secs: None,
            thumbnail_url: None,
            platform: Platform::Instagram,
            streams: vec![stream("reel_video", Some("1080x1920"), Some("h264"), Some("aac"))],
        };
        let insta_audio = choose_stream(&manifest_insta, true, None);
        assert!(insta_audio.is_some());
        assert_eq!(insta_audio.unwrap().container, "m4a");
    }

    fn stream(
        id: &str,
        resolution: Option<&str>,
        vcodec: Option<&str>,
        acodec: Option<&str>,
    ) -> MediaStream {
        MediaStream {
            format_id: id.to_owned(),
            delivery: StreamDelivery::Direct {
                url: "https://example.test/media".to_owned(),
            },
            container: "mp4".to_owned(),
            resolution: resolution.map(str::to_owned),
            fps: None,
            filesize_bytes: None,
            vcodec: vcodec.map(str::to_owned),
            acodec: acodec.map(str::to_owned),
            headers: HashMap::new(),
        }
    }
}
