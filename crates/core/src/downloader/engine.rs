//! Delivery orchestration for direct, DASH, and HLS streams.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use reqwest::Client;
use tokio::fs as tokio_fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc::Sender, watch};

use super::fs;
use super::http::{HttpDownloadError, HttpDownloader};
use super::types::{DownloadPhase, DownloadProgress, ProgressReporter};
use super::DownloadError;
use crate::resolver::{MediaManifest, MediaStream, StreamDelivery};
use crate::sidecar::{SidecarManager, FFMPEG_BINARY_NAME};

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";

#[derive(Clone)]
pub struct DownloadEngine {
    http: HttpDownloader,
    sidecar: SidecarManager,
}

impl DownloadEngine {
    pub fn new(sidecar: SidecarManager) -> Result<Self, DownloadError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| DownloadError::Ffmpeg {
                stderr: format!("could not build HTTP client: {error}"),
            })?;
        Ok(Self {
            http: HttpDownloader::new(client).with_max_connections(6),
            sidecar,
        })
    }

    pub async fn download(
        &self,
        manifest: &MediaManifest,
        stream: &MediaStream,
        output_directory: Option<&Path>,
        cancel: watch::Receiver<bool>,
        progress_sender: Sender<DownloadProgress>,
    ) -> Result<PathBuf, DownloadError> {
        let directory = match output_directory {
            Some(path) => path.to_path_buf(),
            None => fs::default_download_dir()?,
        };
        fs::create_directory(&directory).await?;
        let output =
            fs::resolve_output_path(Some(&directory), &manifest.title, &stream.container).await?;
        let reporter = ProgressReporter::new(progress_sender);
        reporter.emit(DownloadPhase::Probing, true).await;
        let result = self
            .download_inner(manifest, stream, &output, &reporter, cancel.clone())
            .await;

        match result {
            Ok(path) => {
                reporter
                    .emit(
                        DownloadPhase::Completed {
                            output_path: path.clone(),
                        },
                        true,
                    )
                    .await;
                Ok(path)
            }
            Err(DownloadError::Cancelled) => {
                cleanup_download_parts(&output).await;
                reporter.emit(DownloadPhase::Cancelled, true).await;
                Err(DownloadError::Cancelled)
            }
            Err(error) => {
                cleanup_download_parts(&output).await;
                reporter
                    .emit(
                        DownloadPhase::Failed {
                            error: error.to_string(),
                        },
                        true,
                    )
                    .await;
                Err(error)
            }
        }
    }

    async fn download_inner(
        &self,
        _manifest: &MediaManifest,
        stream: &MediaStream,
        output: &Path,
        reporter: &ProgressReporter,
        cancel: watch::Receiver<bool>,
    ) -> Result<PathBuf, DownloadError> {
        if *cancel.borrow() {
            return Err(DownloadError::Cancelled);
        }
        match &stream.delivery {
            StreamDelivery::Direct { url } => {
                reporter.emit(DownloadPhase::Downloading, true).await;
                let part = fs::part_path(output);
                map_http(
                    self.http
                        .download(
                            url,
                            &stream.headers,
                            stream.filesize_bytes,
                            &part,
                            reporter,
                            cancel,
                        )
                        .await,
                )?;
                rename_part(&part, output).await?;
                Ok(output.to_path_buf())
            }
            StreamDelivery::SplitDash {
                video_url,
                audio_url,
            } => {
                reporter.emit(DownloadPhase::Downloading, true).await;
                let video_part = fs::auxiliary_part_path(output, "video");
                let audio_part = fs::auxiliary_part_path(output, "audio");
                map_http(
                    self.http
                        .download(
                            video_url,
                            &stream.headers,
                            None,
                            &video_part,
                            reporter,
                            cancel.clone(),
                        )
                        .await,
                )?;
                map_http(
                    self.http
                        .download(
                            audio_url,
                            &stream.headers,
                            None,
                            &audio_part,
                            reporter,
                            cancel.clone(),
                        )
                        .await,
                )?;
                if *cancel.borrow() {
                    return Err(DownloadError::Cancelled);
                }
                reporter.emit(DownloadPhase::Muxing, true).await;
                let mux_part = fs::auxiliary_part_path(output, "mux");
                run_ffmpeg(
                    &self.sidecar.binary_path(FFMPEG_BINARY_NAME),
                    vec![
                        "-y".to_owned(),
                        "-i".to_owned(),
                        video_part.to_string_lossy().into_owned(),
                        "-i".to_owned(),
                        audio_part.to_string_lossy().into_owned(),
                        "-c".to_owned(),
                        "copy".to_owned(),
                        "-movflags".to_owned(),
                        "+faststart".to_owned(),
                        "-f".to_owned(),
                        "mp4".to_owned(),
                        "-loglevel".to_owned(),
                        "error".to_owned(),
                        mux_part.to_string_lossy().into_owned(),
                    ],
                    cancel,
                    reporter,
                )
                .await?;
                rename_part(&mux_part, output).await?;
                Ok(output.to_path_buf())
            }
            StreamDelivery::Hls { manifest_url } => {
                reporter.emit(DownloadPhase::Muxing, true).await;
                let mux_part = fs::auxiliary_part_path(output, "mux");
                let mut args = vec!["-y".to_owned()];
                if let Some(headers) = ffmpeg_headers(&stream.headers) {
                    args.extend(["-headers".to_owned(), headers]);
                }
                args.extend([
                    "-i".to_owned(),
                    manifest_url.clone(),
                    "-c".to_owned(),
                    "copy".to_owned(),
                    "-bsf:a".to_owned(),
                    "aac_adtstoasc".to_owned(),
                    "-movflags".to_owned(),
                    "+faststart".to_owned(),
                    "-f".to_owned(),
                    "mp4".to_owned(),
                    "-loglevel".to_owned(),
                    "error".to_owned(),
                ]);
                args.push(mux_part.to_string_lossy().into_owned());
                run_ffmpeg(
                    &self.sidecar.binary_path(FFMPEG_BINARY_NAME),
                    args,
                    cancel,
                    reporter,
                )
                .await?;
                rename_part(&mux_part, output).await?;
                Ok(output.to_path_buf())
            }
        }
    }
}

fn map_http(result: Result<u64, HttpDownloadError>) -> Result<u64, DownloadError> {
    match result {
        Ok(bytes) => Ok(bytes),
        Err(HttpDownloadError::Cancelled) => Err(DownloadError::Cancelled),
        Err(error) => Err(DownloadError::Http(error)),
    }
}

async fn rename_part(part: &Path, output: &Path) -> Result<(), DownloadError> {
    tokio_fs::rename(part, output).await.map_err(|source| {
        DownloadError::Filesystem(super::fs::FsError::Rename {
            from: part.to_path_buf(),
            to: output.to_path_buf(),
            source,
        })
    })
}

async fn cleanup_download_parts(output: &Path) {
    fs::remove_if_present(&fs::part_path(output)).await;
    fs::remove_if_present(&fs::auxiliary_part_path(output, "video")).await;
    fs::remove_if_present(&fs::auxiliary_part_path(output, "audio")).await;
    fs::remove_if_present(&fs::auxiliary_part_path(output, "mux")).await;
}

async fn run_ffmpeg(
    binary: &Path,
    args: Vec<String>,
    mut cancel: watch::Receiver<bool>,
    reporter: &ProgressReporter,
) -> Result<(), DownloadError> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if Path::new("/etc/ssl/cert.pem").is_file() {
        command.env("SSL_CERT_FILE", "/etc/ssl/cert.pem");
    }
    let mut child = command.spawn().map_err(DownloadError::FfmpegSpawn)?;
    let stderr = child.stderr.take();
    let stderr_task = stderr.map(|stream| {
        let reporter = reporter.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            let mut output = String::new();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        update_ffmpeg_progress(&line, &reporter).await;
                        output.push_str(&line);
                        output.push('\n');
                    }
                    Ok(None) => break,
                    Err(error) => {
                        output.push_str(&error.to_string());
                        break;
                    }
                }
            }
            output
        })
    });

    let status = loop {
        tokio::select! {
            result = child.wait() => break result.map_err(DownloadError::FfmpegSpawn)?,
            changed = cancel.changed() => {
                if changed.is_ok() && *cancel.borrow() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    if let Some(task) = stderr_task { task.abort(); }
                    return Err(DownloadError::Cancelled);
                }
            }
        }
    };
    let stderr = match stderr_task {
        Some(task) => match task.await {
            Ok(output) => output,
            Err(error) => error.to_string(),
        },
        None => String::new(),
    };
    if status.success() {
        Ok(())
    } else {
        Err(DownloadError::Ffmpeg { stderr })
    }
}

async fn update_ffmpeg_progress(line: &str, reporter: &ProgressReporter) {
    for key in ["size=", "total_size="] {
        if let Some(value) = line
            .split_once(key)
            .and_then(|(_, value)| value.split_whitespace().next())
        {
            let value = value.trim_end_matches('k').parse::<u64>().ok();
            if let Some(value) = value {
                let bytes = if line.contains("size=") && !line.contains("total_size=") {
                    value.saturating_mul(1024)
                } else {
                    value
                };
                reporter.set_downloaded(bytes);
                reporter.emit(DownloadPhase::Muxing, false).await;
                return;
            }
        }
    }
    if let Some(value) = line
        .split_once("time=")
        .and_then(|(_, value)| value.split_whitespace().next())
    {
        if parse_ffmpeg_time(value).is_some() {
            reporter.emit(DownloadPhase::Muxing, false).await;
        }
    }
}

fn parse_ffmpeg_time(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<f64>().ok()?;
    let minutes = parts.next()?.parse::<f64>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

fn ffmpeg_headers(headers: &std::collections::HashMap<String, String>) -> Option<String> {
    if headers.is_empty() {
        return None;
    }
    let value = headers
        .iter()
        .map(|(key, value)| format!("{key}: {value}\r\n"))
        .collect::<String>();
    Some(value)
}
