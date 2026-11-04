//! Safe output naming, collision handling, and temporary-file helpers.

use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::fs;

#[derive(Debug, Error)]
pub enum FsError {
    #[error("could not resolve the Downloads directory")]
    DownloadDirectoryUnavailable,
    #[error("could not inspect output path '{path}': {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not rename '{from}' to '{to}': {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not create output directory '{path}': {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn default_download_dir() -> Result<PathBuf, FsError> {
    dirs::download_dir().ok_or(FsError::DownloadDirectoryUnavailable)
}

pub fn sanitize_filename(title: &str) -> String {
    let replaced: String = title
        .chars()
        .map(|character| {
            if matches!(character, '/' | ':' | '\\' | '\0') || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = replaced.trim_matches(|character| character == '.' || character == ' ');
    let mut result: String = trimmed.chars().take(200).collect();
    result = result
        .trim_matches(|character| character == '.' || character == ' ')
        .to_owned();
    if result.is_empty() {
        "download".to_owned()
    } else {
        result
    }
}

pub async fn resolve_output_path(
    directory: Option<&Path>,
    title: &str,
    extension: &str,
) -> Result<PathBuf, FsError> {
    let directory = match directory {
        Some(directory) => directory.to_path_buf(),
        None => default_download_dir()?,
    };
    let extension = extension.trim_start_matches('.');
    let base = sanitize_filename(title);
    let initial = directory.join(format!("{base}.{extension}"));
    if !exists(&initial).await? {
        return Ok(initial);
    }

    let (stem, suffix) = match initial.file_name().and_then(|name| name.to_str()) {
        Some(name) => match name.rsplit_once('.') {
            Some((stem, extension)) => (stem.to_owned(), format!(".{extension}")),
            None => (name.to_owned(), String::new()),
        },
        None => (base, format!(".{extension}")),
    };
    let mut index = 1_u64;
    loop {
        let candidate = directory.join(format!("{stem} ({index}){suffix}"));
        if !exists(&candidate).await? {
            return Ok(candidate);
        }
        index = index.saturating_add(1);
    }
}

pub fn part_path(output: &Path) -> PathBuf {
    append_suffix(output, ".part")
}

pub fn auxiliary_part_path(output: &Path, suffix: &str) -> PathBuf {
    append_suffix(output, &format!(".{suffix}.part"))
}

/// RAII guard that automatically cleans up in-flight temporary `.part` files
/// when dropped, unless explicitly disarmed. This guarantees `.part` file
/// cleanup even on cancellation, panic, or unhandled task drops.
#[derive(Debug, Default)]
pub struct PartCleanupGuard {
    paths: Vec<PathBuf>,
    disarmed: bool,
}

impl PartCleanupGuard {
    pub fn new() -> Self {
        Self {
            paths: Vec::new(),
            disarmed: false,
        }
    }

    pub fn track(&mut self, path: impl Into<PathBuf>) {
        self.paths.push(path.into());
    }

    pub fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for PartCleanupGuard {
    fn drop(&mut self) {
        if !self.disarmed {
            for path in &self.paths {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

pub async fn create_directory(path: &Path) -> Result<(), FsError> {
    fs::create_dir_all(path)
        .await
        .map_err(|source| FsError::CreateDirectory {
            path: path.to_path_buf(),
            source,
        })
}

pub async fn remove_if_present(path: &Path) {
    match fs::remove_file(path).await {
        Ok(()) | Err(_) => {}
    }
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

async fn exists(path: &Path) -> Result<bool, FsError> {
    match fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(FsError::Inspect {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_macos_unsafe_characters_and_caps_unicode_characters() {
        let title = format!(".. bad/name: {}{}", '\0', "é".repeat(210));
        let value = sanitize_filename(&title);
        assert!(!value.contains('/'));
        assert!(!value.contains(':'));
        assert!(!value.contains('\0'));
        assert!(value.chars().count() <= 200);
        assert!(!value.starts_with('.'));
    }

    #[tokio::test]
    async fn appends_collision_suffix_before_extension() {
        let directory = tempfile_dir().await;
        let first = directory.join("clip.mp4");
        assert!(fs::write(&first, b"existing").await.is_ok());
        let result = resolve_output_path(Some(&directory), "clip", "mp4").await;
        assert!(result.is_ok());
        assert_eq!(
            result.ok().as_deref(),
            Some(directory.join("clip (1).mp4").as_path())
        );
        remove_if_present(&first).await;
        match fs::remove_dir(&directory).await {
            Ok(()) | Err(_) => {}
        }
    }

    async fn tempfile_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("rust-dl-fs-{}", std::process::id()));
        match fs::remove_dir_all(&path).await {
            Ok(()) | Err(_) => {}
        }
        let created = fs::create_dir_all(&path).await;
        assert!(created.is_ok());
        path
    }
}
