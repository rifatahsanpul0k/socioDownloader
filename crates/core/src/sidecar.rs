//! Self-bootstrapping management of the external media tools used by rust-dl.

use std::path::{Component, Path, PathBuf};
use std::process::Output;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc::Sender;

pub const YT_DLP_BINARY_NAME: &str = "yt-dlp_macos";
pub const FFMPEG_BINARY_NAME: &str = "ffmpeg";

const YT_DLP_DOWNLOAD_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos";

fn ffmpeg_download_url() -> &'static str {
    if std::env::consts::ARCH == "x86_64" {
        "https://ffmpeg.martin-riedl.de/redirect/latest/macos/amd64/release/ffmpeg.zip"
    } else {
        "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip"
    }
}

#[derive(Debug, Error)]
pub enum SidecarError {
    #[error("could not resolve the macOS application support directory")]
    ApplicationSupportDirectoryUnavailable,

    #[error("sidecar binaries are only supported on macOS")]
    UnsupportedPlatform,

    #[error("could not {operation} '{path}': {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("HTTP request for {binary} failed: {source}")]
    Request {
        binary: &'static str,
        #[source]
        source: reqwest::Error,
    },

    #[error("download for {binary} returned HTTP status {status}")]
    HttpStatus {
        binary: &'static str,
        status: StatusCode,
    },

    #[error("download for {binary} was empty")]
    EmptyDownload { binary: &'static str },

    #[error("could not install '{binary}': downloaded file was not a regular file")]
    InvalidBinary { binary: &'static str },

    #[error("could not extract FFmpeg archive: {message}")]
    Archive { message: String },

    #[error("{binary} exited unsuccessfully while checking its version: {stderr}")]
    VersionCommandFailed {
        binary: &'static str,
        stderr: String,
    },

    #[error("{binary} did not return a version string")]
    MissingVersion { binary: &'static str },

    #[error("yt-dlp updater exited unsuccessfully: {stderr}")]
    UpdateCommandFailed { stderr: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DownloadProgress {
    Started {
        binary: String,
        total_bytes: Option<u64>,
    },
    Bytes {
        binary: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Completed {
        binary: String,
        sha256: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryVersions {
    pub yt_dlp: String,
    pub ffmpeg: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarUpdateReport {
    pub previous_version: String,
    pub current_version: String,
    pub updated: bool,
    pub output_summary: String,
}

#[derive(Debug, Clone, Copy)]
struct BinarySpec {
    name: &'static str,
    url: &'static str,
    archive: bool,
}

const SHARED_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";

/// Creates the shared `reqwest::Client` used across the workspace.
pub fn create_default_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(SHARED_USER_AGENT)
        .build()
}

/// Manages the private, per-user directory containing rust-dl's sidecars.
#[derive(Clone)]
pub struct SidecarManager {
    bin_dir: PathBuf,
    client: reqwest::Client,
}

impl SidecarManager {
    /// Creates a manager rooted at `~/Library/Application Support/rust-dl/bin` with the default HTTP client.
    pub fn new() -> Result<Self, SidecarError> {
        let client = create_default_client().map_err(|source| SidecarError::Request {
            binary: "sidecar-client",
            source,
        })?;
        Self::new_with_client(client)
    }

    /// Creates a manager rooted at `~/Library/Application Support/rust-dl/bin` sharing an existing `reqwest::Client`.
    pub fn new_with_client(client: reqwest::Client) -> Result<Self, SidecarError> {
        if !cfg!(target_os = "macos") {
            return Err(SidecarError::UnsupportedPlatform);
        }

        let application_support =
            dirs::data_dir().ok_or(SidecarError::ApplicationSupportDirectoryUnavailable)?;
        Ok(Self {
            bin_dir: application_support.join("rust-dl").join("bin"),
            client,
        })
    }

    /// Returns a reference to the shared HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Returns the directory in which the managed binaries are installed.
    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
    }

    /// Returns the absolute path of a managed sidecar executable.
    pub fn binary_path(&self, name: &str) -> PathBuf {
        self.bin_dir.join(name)
    }

    /// Ensures both sidecars are installed and executable.
    ///
    /// Progress events are advisory: a caller closing the receiver does not
    /// interrupt a binary download.
    pub async fn ensure_binaries(
        &self,
        progress: Sender<DownloadProgress>,
    ) -> Result<(), SidecarError> {
        fs::create_dir_all(&self.bin_dir)
            .await
            .map_err(|source| SidecarError::Io {
                operation: "create sidecar directory",
                path: self.bin_dir.clone(),
                source,
            })?;

        let specs = binary_specs()?;
        for spec in specs {
            let destination = self.bin_dir.join(spec.name);
            if self.is_ready(&destination).await? {
                continue;
            }

            if spec.name == FFMPEG_BINARY_NAME {
                if let Some(system_path) = find_system_ffmpeg().await {
                    if fs::copy(&system_path, &destination).await.is_ok() {
                        let _ = set_executable(&destination).await;
                        if self.is_ready(&destination).await.unwrap_or(false) {
                            send_progress(
                                &progress,
                                DownloadProgress::Completed {
                                    binary: spec.name.to_owned(),
                                    sha256: "system-binary".to_owned(),
                                },
                            )
                            .await;
                            continue;
                        }
                    }
                }
            }

            if spec.archive {
                self.download_archive(spec, &destination, &progress).await?;
            } else {
                self.download_binary(spec, &destination, &progress).await?;
            }
        }

        Ok(())
    }

    /// Runs both managed tools asynchronously and returns their version lines.
    pub async fn verify_binaries(&self) -> Result<BinaryVersions, SidecarError> {
        let yt_dlp_path = self.bin_dir.join(YT_DLP_BINARY_NAME);
        let ffmpeg_path = self.bin_dir.join(FFMPEG_BINARY_NAME);

        let (yt_dlp, ffmpeg) = tokio::try_join!(
            run_version(&yt_dlp_path, YT_DLP_BINARY_NAME, "--version"),
            run_version(&ffmpeg_path, FFMPEG_BINARY_NAME, "-version"),
        )?;

        Ok(BinaryVersions { yt_dlp, ffmpeg })
    }

    pub async fn update_ytdlp(&self) -> Result<SidecarUpdateReport, crate::CoreError> {
        let path = self.bin_dir.join(YT_DLP_BINARY_NAME);
        let previous_version = run_version(&path, YT_DLP_BINARY_NAME, "--version").await?;
        let output = Command::new(&path)
            .arg("-U")
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|source| SidecarError::Io {
                operation: "run yt-dlp updater",
                path: path.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(SidecarError::UpdateCommandFailed {
                stderr: command_error(&output),
            }
            .into());
        }
        let output_summary = command_error(&output);
        let current_version = run_version(&path, YT_DLP_BINARY_NAME, "--version").await?;
        Ok(SidecarUpdateReport {
            updated: previous_version != current_version,
            previous_version,
            current_version,
            output_summary,
        })
    }

    async fn is_ready(&self, path: &Path) -> Result<bool, SidecarError> {
        let metadata = match fs::metadata(path).await {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(SidecarError::Io {
                    operation: "inspect sidecar",
                    path: path.to_path_buf(),
                    source,
                });
            }
        };

        if !metadata.is_file() {
            return Err(SidecarError::InvalidBinary {
                binary: binary_name(path),
            });
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if metadata.permissions().mode() & 0o111 == 0 {
                set_executable(path).await?;
            }
        }

        #[cfg(not(unix))]
        {
            return Err(SidecarError::UnsupportedPlatform);
        }

        Ok(true)
    }

    async fn download_binary(
        &self,
        spec: BinarySpec,
        destination: &Path,
        progress: &Sender<DownloadProgress>,
    ) -> Result<(), SidecarError> {
        let temporary = self.bin_dir.join(format!(".{}.download", spec.name));
        let digest = match self.download_to_file(spec, &temporary, progress).await {
            Ok(digest) => digest,
            Err(error) => {
                remove_file_if_present(&temporary).await;
                return Err(error);
            }
        };

        if let Err(error) = set_executable(&temporary).await {
            remove_file_if_present(&temporary).await;
            return Err(error);
        }

        if let Err(source) = fs::rename(&temporary, destination).await {
            remove_file_if_present(&temporary).await;
            return Err(SidecarError::Io {
                operation: "install sidecar",
                path: destination.to_path_buf(),
                source,
            });
        }
        send_progress(
            progress,
            DownloadProgress::Completed {
                binary: spec.name.to_owned(),
                sha256: digest,
            },
        )
        .await;
        Ok(())
    }

    async fn download_archive(
        &self,
        spec: BinarySpec,
        destination: &Path,
        progress: &Sender<DownloadProgress>,
    ) -> Result<(), SidecarError> {
        let archive_path = self.bin_dir.join(format!(".{}.zip", spec.name));
        let extract_dir = self.bin_dir.join(format!(".{}-extract", spec.name));

        let digest = match self.download_to_file(spec, &archive_path, progress).await {
            Ok(digest) => digest,
            Err(error) => {
                remove_file_if_present(&archive_path).await;
                return Err(error);
            }
        };

        let result = async {
            validate_archive_entries(&archive_path).await?;
            remove_dir_if_present(&extract_dir).await;
            fs::create_dir(&extract_dir)
                .await
                .map_err(|source| SidecarError::Io {
                    operation: "create archive extraction directory",
                    path: extract_dir.clone(),
                    source,
                })?;

            let output = Command::new("/usr/bin/unzip")
                .arg("-q")
                .arg("-o")
                .arg(&archive_path)
                .arg("-d")
                .arg(&extract_dir)
                .output()
                .await
                .map_err(|source| SidecarError::Io {
                    operation: "extract FFmpeg archive",
                    path: archive_path.clone(),
                    source,
                })?;
            if !output.status.success() {
                return Err(SidecarError::Archive {
                    message: command_error(&output),
                });
            }

            let extracted = find_file_named(&extract_dir, FFMPEG_BINARY_NAME)
                .await?
                .ok_or_else(|| SidecarError::Archive {
                    message: "archive did not contain an ffmpeg executable".to_owned(),
                })?;
            let temporary = self.bin_dir.join(format!(".{}.download", spec.name));
            fs::copy(&extracted, &temporary)
                .await
                .map_err(|source| SidecarError::Io {
                    operation: "stage extracted FFmpeg binary",
                    path: temporary.clone(),
                    source,
                })?;
            set_executable(&temporary).await?;
            fs::rename(&temporary, destination)
                .await
                .map_err(|source| SidecarError::Io {
                    operation: "install FFmpeg",
                    path: destination.to_path_buf(),
                    source,
                })?;
            send_progress(
                progress,
                DownloadProgress::Completed {
                    binary: spec.name.to_owned(),
                    sha256: digest.clone(),
                },
            )
            .await;
            Ok::<(), SidecarError>(())
        }
        .await;

        remove_file_if_present(&archive_path).await;
        remove_dir_if_present(&extract_dir).await;
        remove_file_if_present(&self.bin_dir.join(format!(".{}.download", spec.name))).await;
        result
    }

    async fn download_to_file(
        &self,
        spec: BinarySpec,
        destination: &Path,
        progress: &Sender<DownloadProgress>,
    ) -> Result<String, SidecarError> {
        let response =
            self.client
                .get(spec.url)
                .send()
                .await
                .map_err(|source| SidecarError::Request {
                    binary: spec.name,
                    source,
                })?;
        if !response.status().is_success() {
            return Err(SidecarError::HttpStatus {
                binary: spec.name,
                status: response.status(),
            });
        }

        let total_bytes = response.content_length();
        send_progress(
            progress,
            DownloadProgress::Started {
                binary: spec.name.to_owned(),
                total_bytes,
            },
        )
        .await;

        let mut response = response;
        let mut file = File::create(destination)
            .await
            .map_err(|source| SidecarError::Io {
                operation: "create download file",
                path: destination.to_path_buf(),
                source,
            })?;
        let mut hasher = Sha256::new();
        let mut downloaded_bytes = 0_u64;

        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|source| SidecarError::Request {
                binary: spec.name,
                source,
            })?
        {
            if chunk.is_empty() {
                continue;
            }
            file.write_all(&chunk)
                .await
                .map_err(|source| SidecarError::Io {
                    operation: "write download file",
                    path: destination.to_path_buf(),
                    source,
                })?;
            hasher.update(&chunk);
            downloaded_bytes += chunk.len() as u64;
            send_progress(
                progress,
                DownloadProgress::Bytes {
                    binary: spec.name.to_owned(),
                    downloaded_bytes,
                    total_bytes,
                },
            )
            .await;
        }

        file.flush().await.map_err(|source| SidecarError::Io {
            operation: "flush download file",
            path: destination.to_path_buf(),
            source,
        })?;
        drop(file);

        if downloaded_bytes == 0 {
            return Err(SidecarError::EmptyDownload { binary: spec.name });
        }

        let digest = hex_digest(hasher.finalize());
        Ok(digest)
    }
}

fn binary_specs() -> Result<[BinarySpec; 2], SidecarError> {
    #[cfg(target_os = "macos")]
    {
        Ok([
            BinarySpec {
                name: YT_DLP_BINARY_NAME,
                url: YT_DLP_DOWNLOAD_URL,
                archive: false,
            },
            BinarySpec {
                name: FFMPEG_BINARY_NAME,
                url: ffmpeg_download_url(),
                archive: true,
            },
        ])
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(SidecarError::UnsupportedPlatform)
    }
}

async fn run_version(
    path: &Path,
    binary: &'static str,
    version_argument: &'static str,
) -> Result<String, SidecarError> {
    let output = Command::new(path)
        .arg(version_argument)
        .output()
        .await
        .map_err(|source| SidecarError::Io {
            operation: "run sidecar version check",
            path: path.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(SidecarError::VersionCommandFailed {
            binary,
            stderr: command_error(&output),
        });
    }

    let version = output_text(&output)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
        .ok_or(SidecarError::MissingVersion { binary })?;
    Ok(version)
}

fn output_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).into_owned()
    } else {
        stdout
    }
}

fn command_error(output: &Output) -> String {
    let text = output_text(output).trim().to_owned();
    if text.is_empty() {
        "no diagnostic output".to_owned()
    } else {
        text
    }
}

async fn send_progress(progress: &Sender<DownloadProgress>, event: DownloadProgress) {
    let _ = progress.send(event).await;
}

#[cfg(unix)]
async fn set_executable(path: &Path) -> Result<(), SidecarError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .await
        .map_err(|source| SidecarError::Io {
            operation: "set executable permissions on",
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
async fn set_executable(_path: &Path) -> Result<(), SidecarError> {
    Err(SidecarError::UnsupportedPlatform)
}

async fn remove_file_if_present(path: &Path) {
    match fs::remove_file(path).await {
        Ok(()) | Err(_) => {}
    }
}

async fn remove_dir_if_present(path: &Path) {
    match fs::remove_dir_all(path).await {
        Ok(()) | Err(_) => {}
    }
}

async fn validate_archive_entries(path: &Path) -> Result<(), SidecarError> {
    let output = Command::new("/usr/bin/unzip")
        .arg("-Z1")
        .arg(path)
        .output()
        .await
        .map_err(|source| SidecarError::Io {
            operation: "inspect FFmpeg archive",
            path: path.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(SidecarError::Archive {
            message: command_error(&output),
        });
    }

    for entry in String::from_utf8_lossy(&output.stdout).lines() {
        let entry_path = Path::new(entry);
        if entry_path.is_absolute()
            || entry_path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(SidecarError::Archive {
                message: format!("archive contains unsafe path '{entry}'"),
            });
        }
    }
    Ok(())
}

async fn find_file_named(directory: &Path, name: &str) -> Result<Option<PathBuf>, SidecarError> {
    let mut entries = fs::read_dir(directory)
        .await
        .map_err(|source| SidecarError::Io {
            operation: "read archive extraction directory",
            path: directory.to_path_buf(),
            source,
        })?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|source| SidecarError::Io {
            operation: "read archive extraction entry",
            path: directory.to_path_buf(),
            source,
        })?
    {
        let path = entry.path();
        let metadata = fs::metadata(&path)
            .await
            .map_err(|source| SidecarError::Io {
                operation: "inspect archive extraction entry",
                path: path.clone(),
                source,
            })?;
        if metadata.is_file() && path.file_name().and_then(|value| value.to_str()) == Some(name) {
            return Ok(Some(path));
        }
        if metadata.is_dir() {
            if let Some(found) = Box::pin(find_file_named(&path, name)).await? {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn binary_name(path: &Path) -> &'static str {
    match path.file_name().and_then(|value| value.to_str()) {
        Some(FFMPEG_BINARY_NAME) => FFMPEG_BINARY_NAME,
        Some(YT_DLP_BINARY_NAME) => YT_DLP_BINARY_NAME,
        _ => "sidecar",
    }
}

async fn find_system_ffmpeg() -> Option<PathBuf> {
    let candidates = [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/opt/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ];
    for candidate in candidates {
        let path = Path::new(candidate);
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("ffmpeg");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
