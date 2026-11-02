//! Bounded parallel HTTP range downloading with a sequential fallback.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, RANGE,
};
use reqwest::{Client, StatusCode};
use thiserror::Error;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{mpsc, watch, Mutex};

use super::types::ProgressReporter;

const DEFAULT_CONNECTIONS: usize = 6;
const SEGMENT_SIZE: u64 = 4 * 1024 * 1024;
const PARALLEL_THRESHOLD: u64 = 2 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum HttpDownloadError {
    #[error("HTTP request for '{url}' failed: {source}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("HTTP request for '{url}' returned status {status}")]
    Status { url: String, status: StatusCode },
    #[error("invalid HTTP header '{name}': {message}")]
    InvalidHeader { name: String, message: String },
    #[error("could not access download part '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("server did not return byte range {start}-{end} for '{url}'")]
    RangeUnsupported { url: String, start: u64, end: u64 },
    #[error("range response for '{url}' contained {actual} bytes; expected {expected}")]
    RangeSizeMismatch {
        url: String,
        actual: u64,
        expected: u64,
    },
    #[error("download cancelled")]
    Cancelled,
    #[error("download worker failed: {message}")]
    WorkerFailed { message: String },
}

#[derive(Debug, Clone, Copy)]
struct ProbeInfo {
    total_bytes: Option<u64>,
    range_supported: bool,
}

#[derive(Clone)]
pub struct HttpDownloader {
    client: Client,
    max_connections: usize,
}

impl HttpDownloader {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            max_connections: DEFAULT_CONNECTIONS,
        }
    }

    pub(crate) fn with_max_connections(mut self, max_connections: usize) -> Self {
        self.max_connections = max_connections.max(1);
        self
    }

    pub(crate) async fn download(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        expected_size: Option<u64>,
        part_path: &Path,
        reporter: &ProgressReporter,
        cancel: watch::Receiver<bool>,
    ) -> Result<u64, HttpDownloadError> {
        if *cancel.borrow() {
            return Err(HttpDownloadError::Cancelled);
        }
        let probe = self.probe(url, headers).await?;
        let total_bytes = expected_size.or(probe.total_bytes);
        if let Some(total) = total_bytes {
            reporter.add_total(total);
        }

        let result = if probe.range_supported
            && total_bytes
                .map(|total| total > PARALLEL_THRESHOLD)
                .unwrap_or(false)
        {
            self.download_parallel(url, headers, total_bytes, part_path, reporter, cancel)
                .await
        } else {
            self.download_sequential(url, headers, total_bytes, part_path, reporter, cancel)
                .await
        };
        if result.is_err() {
            super::fs::remove_if_present(part_path).await;
        }
        result
    }

    async fn probe(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<ProbeInfo, HttpDownloadError> {
        let header_map = build_headers(headers)?;
        if let Ok(response) = self
            .client
            .head(url)
            .headers(header_map.clone())
            .send()
            .await
        {
            if response.status().is_success() {
                let total_bytes = response
                    .headers()
                    .get(CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok());
                let range_supported = response
                    .headers()
                    .get(ACCEPT_RANGES)
                    .and_then(|value| value.to_str().ok())
                    .map(|value| value.eq_ignore_ascii_case("bytes"))
                    .unwrap_or(false);
                if range_supported || total_bytes.is_some() {
                    return Ok(ProbeInfo {
                        total_bytes,
                        range_supported,
                    });
                }
            }
        }

        let response = self
            .client
            .get(url)
            .headers(header_map)
            .header(RANGE, "bytes=0-0")
            .send()
            .await
            .map_err(|source| HttpDownloadError::Request {
                url: url.to_owned(),
                source,
            })?;
        let total_bytes = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range_total)
            .or_else(|| {
                response
                    .headers()
                    .get(CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
            });
        let range_supported = response.status() == StatusCode::PARTIAL_CONTENT
            && response.headers().get(CONTENT_RANGE).is_some();
        if response.status().is_success() || response.status() == StatusCode::PARTIAL_CONTENT {
            Ok(ProbeInfo {
                total_bytes,
                range_supported,
            })
        } else {
            Err(HttpDownloadError::Status {
                url: url.to_owned(),
                status: response.status(),
            })
        }
    }

    async fn download_sequential(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        total_bytes: Option<u64>,
        part_path: &Path,
        reporter: &ProgressReporter,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<u64, HttpDownloadError> {
        let response = request(&self.client, url, headers, None, &mut cancel).await?;
        if !response.status().is_success() {
            return Err(HttpDownloadError::Status {
                url: url.to_owned(),
                status: response.status(),
            });
        }
        let mut writer = BufWriter::new(File::create(part_path).await.map_err(|source| {
            HttpDownloadError::Io {
                path: part_path.to_path_buf(),
                source,
            }
        })?);
        let mut response = response;
        let mut downloaded = 0_u64;
        loop {
            if *cancel.borrow() {
                return Err(HttpDownloadError::Cancelled);
            }
            let next = tokio::select! {
                result = response.chunk() => result.map_err(|source| HttpDownloadError::Request { url: url.to_owned(), source })?,
                changed = cancel.changed() => {
                    if changed.is_ok() && *cancel.borrow() { return Err(HttpDownloadError::Cancelled); }
                    continue;
                }
            };
            let Some(chunk) = next else { break };
            writer
                .write_all(&chunk)
                .await
                .map_err(|source| HttpDownloadError::Io {
                    path: part_path.to_path_buf(),
                    source,
                })?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            reporter.add_downloaded(chunk.len() as u64);
            reporter
                .emit(super::types::DownloadPhase::Downloading, false)
                .await;
        }
        writer
            .flush()
            .await
            .map_err(|source| HttpDownloadError::Io {
                path: part_path.to_path_buf(),
                source,
            })?;
        if let Some(expected) = total_bytes {
            if downloaded != expected {
                return Err(HttpDownloadError::RangeSizeMismatch {
                    url: url.to_owned(),
                    actual: downloaded,
                    expected,
                });
            }
        }
        Ok(downloaded)
    }

    async fn download_parallel(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        total_bytes: Option<u64>,
        part_path: &Path,
        reporter: &ProgressReporter,
        cancel: watch::Receiver<bool>,
    ) -> Result<u64, HttpDownloadError> {
        let total = total_bytes.ok_or_else(|| HttpDownloadError::WorkerFailed {
            message: "parallel download requires a known size".to_owned(),
        })?;
        let async_file = File::create(part_path)
            .await
            .map_err(|source| HttpDownloadError::Io {
                path: part_path.to_path_buf(),
                source,
            })?;
        async_file
            .set_len(total)
            .await
            .map_err(|source| HttpDownloadError::Io {
                path: part_path.to_path_buf(),
                source,
            })?;
        drop(async_file);
        let positioned_file = Arc::new(
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(part_path)
                .map_err(|source| HttpDownloadError::Io {
                    path: part_path.to_path_buf(),
                    source,
                })?,
        );

        let (segment_tx, segment_rx) = mpsc::channel::<(u64, u64)>(self.max_connections);
        let receiver = Arc::new(Mutex::new(segment_rx));
        let mut workers = Vec::new();
        for _ in 0..self.max_connections {
            let receiver = Arc::clone(&receiver);
            let file = Arc::clone(&positioned_file);
            let client = self.client.clone();
            let request_headers = headers.clone();
            let worker_url = url.to_owned();
            let worker_reporter = reporter.clone();
            let worker_cancel = cancel.clone();
            workers.push(tokio::spawn(async move {
                loop {
                    if *worker_cancel.borrow() {
                        return Err(HttpDownloadError::Cancelled);
                    }
                    let segment = receiver.lock().await.recv().await;
                    let Some((start, end)) = segment else {
                        return Ok(());
                    };
                    download_segment(
                        &client,
                        &worker_url,
                        &request_headers,
                        start,
                        end,
                        Arc::clone(&file),
                        &worker_reporter,
                        worker_cancel.clone(),
                    )
                    .await?;
                }
            }));
        }

        let mut start = 0_u64;
        while start < total {
            if *cancel.borrow() {
                drop(segment_tx);
                for worker in workers {
                    let _ = worker.await;
                }
                return Err(HttpDownloadError::Cancelled);
            }
            let end = start
                .saturating_add(SEGMENT_SIZE)
                .saturating_sub(1)
                .min(total.saturating_sub(1));
            if segment_tx.send((start, end)).await.is_err() {
                break;
            }
            start = end.saturating_add(1);
        }
        drop(segment_tx);
        let mut first_error = None;
        for worker in workers {
            match worker.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) if first_error.is_none() => first_error = Some(error),
                Ok(Err(_)) => {}
                Err(error) if first_error.is_none() => {
                    first_error = Some(HttpDownloadError::WorkerFailed {
                        message: error.to_string(),
                    });
                }
                Err(_) => {}
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(total)
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_segment(
    client: &Client,
    url: &str,
    headers: &HashMap<String, String>,
    start: u64,
    end: u64,
    file: Arc<std::fs::File>,
    reporter: &ProgressReporter,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), HttpDownloadError> {
    let expected = end.saturating_sub(start).saturating_add(1);
    let mut attempt = 0_u32;
    loop {
        if *cancel.borrow() {
            return Err(HttpDownloadError::Cancelled);
        }
        let result = download_segment_attempt(
            client,
            url,
            headers,
            start,
            end,
            expected,
            Arc::clone(&file),
            reporter,
            &mut cancel,
        )
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(error) if is_retryable(&error) && attempt < 3 => {
                let delay =
                    Duration::from_millis(100_u64.saturating_mul(2_u64.saturating_pow(attempt)));
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    changed = cancel.changed() => {
                        if changed.is_ok() && *cancel.borrow() { return Err(HttpDownloadError::Cancelled); }
                    }
                }
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_segment_attempt(
    client: &Client,
    url: &str,
    headers: &HashMap<String, String>,
    start: u64,
    end: u64,
    expected: u64,
    file: Arc<std::fs::File>,
    reporter: &ProgressReporter,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), HttpDownloadError> {
    let range = format!("bytes={start}-{end}");
    let response = request(client, url, headers, Some(&range), cancel).await?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(HttpDownloadError::RangeUnsupported {
            url: url.to_owned(),
            start,
            end,
        });
    }
    let mut response = response;
    let mut offset = start;
    let mut written = 0_u64;
    loop {
        if *cancel.borrow() {
            return Err(HttpDownloadError::Cancelled);
        }
        let next = tokio::select! {
            result = response.chunk() => result.map_err(|source| HttpDownloadError::Request { url: url.to_owned(), source })?,
            changed = cancel.changed() => {
                if changed.is_ok() && *cancel.borrow() { return Err(HttpDownloadError::Cancelled); }
                continue;
            }
        };
        let Some(chunk) = next else { break };
        let chunk_len = chunk.len() as u64;
        if written.saturating_add(chunk_len) > expected {
            return Err(HttpDownloadError::RangeSizeMismatch {
                url: url.to_owned(),
                actual: written.saturating_add(chunk_len),
                expected,
            });
        }
        let bytes = chunk.to_vec();
        let write_offset = offset;
        let file = Arc::clone(&file);
        tokio::task::spawn_blocking(move || write_at(file.as_ref(), &bytes, write_offset))
            .await
            .map_err(|error| HttpDownloadError::WorkerFailed {
                message: error.to_string(),
            })??;
        offset = offset.saturating_add(chunk_len);
        written = written.saturating_add(chunk_len);
        reporter.add_downloaded(chunk_len);
        reporter
            .emit(super::types::DownloadPhase::Downloading, false)
            .await;
    }
    if written != expected {
        return Err(HttpDownloadError::RangeSizeMismatch {
            url: url.to_owned(),
            actual: written,
            expected,
        });
    }
    Ok(())
}

async fn request(
    client: &Client,
    url: &str,
    headers: &HashMap<String, String>,
    range: Option<&str>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<reqwest::Response, HttpDownloadError> {
    loop {
        let mut request = client.get(url).headers(build_headers(headers)?);
        if let Some(range) = range {
            request = request.header(RANGE, range);
        }
        tokio::select! {
            response = request.send() => return response.map_err(|source| HttpDownloadError::Request { url: url.to_owned(), source }),
            changed = cancel.changed() => {
                if changed.is_ok() && *cancel.borrow() { return Err(HttpDownloadError::Cancelled); }
            }
        }
    }
}

fn build_headers(headers: &HashMap<String, String>) -> Result<HeaderMap, HttpDownloadError> {
    let mut result = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            HttpDownloadError::InvalidHeader {
                name: name.clone(),
                message: error.to_string(),
            }
        })?;
        let value =
            HeaderValue::from_str(value).map_err(|error| HttpDownloadError::InvalidHeader {
                name: name.to_string(),
                message: error.to_string(),
            })?;
        result.insert(name, value);
    }
    Ok(result)
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    value.split_once('/')?.1.parse::<u64>().ok()
}

fn is_retryable(error: &HttpDownloadError) -> bool {
    match error {
        HttpDownloadError::Request { .. } => true,
        HttpDownloadError::Status { status, .. } => {
            status.is_server_error() || matches!(status.as_u16(), 408 | 429)
        }
        _ => false,
    }
}

#[cfg(unix)]
fn write_at(file: &std::fs::File, bytes: &[u8], offset: u64) -> Result<(), HttpDownloadError> {
    use std::os::unix::fs::FileExt;

    file.write_all_at(bytes, offset)
        .map_err(|source| HttpDownloadError::Io {
            path: PathBuf::from("parallel download part"),
            source,
        })
}

#[cfg(not(unix))]
fn write_at(_file: &std::fs::File, _bytes: &[u8], _offset: u64) -> Result<(), HttpDownloadError> {
    Err(HttpDownloadError::WorkerFailed {
        message: "positioned writes require a Unix target".to_owned(),
    })
}
