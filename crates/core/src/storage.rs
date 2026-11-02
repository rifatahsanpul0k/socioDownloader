//! Atomic JSON configuration and bounded download history.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::auth::CookieSource;
use crate::resolver::Platform;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("could not resolve rust-dl application support directory")]
    RootUnavailable,
    #[error("could not read '{path}': {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write '{path}': {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse '{path}': {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub download_dir: PathBuf,
    pub max_concurrent_downloads: usize,
    pub auto_clipboard: bool,
    pub cookie_source: CookieSource,
    pub quality_preset: String,
    #[serde(default)]
    pub allow_playlists: bool,
    #[serde(default = "default_max_playlist_items")]
    pub max_playlist_items: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            download_dir: dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")),
            max_concurrent_downloads: 3,
            auto_clipboard: false,
            cookie_source: CookieSource::None,
            quality_preset: "best".to_owned(),
            allow_playlists: false,
            max_playlist_items: 25,
        }
    }
}

impl AppConfig {
    pub fn normalize(&mut self) {
        self.max_concurrent_downloads = self.max_concurrent_downloads.clamp(1, 8);
        if self.quality_preset.is_empty() {
            self.quality_preset = "best".to_owned();
        }
        self.max_playlist_items = self.max_playlist_items.clamp(1, 1000);
    }

    pub async fn load_default() -> Result<Self, StorageError> {
        Self::load_from(&state_root()?).await
    }

    pub async fn save_default(&self) -> Result<(), StorageError> {
        self.save_to(&state_root()?).await
    }

    pub async fn load_from(root: &Path) -> Result<Self, StorageError> {
        let path = root.join("config.json");
        match fs::read(&path).await {
            Ok(bytes) => {
                let mut config = serde_json::from_slice::<Self>(&bytes)
                    .map_err(|source| StorageError::Json { path, source })?;
                config.normalize();
                Ok(config)
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(StorageError::Read { path, source }),
        }
    }

    pub async fn save_to(&self, root: &Path) -> Result<(), StorageError> {
        let mut normalized = self.clone();
        normalized.normalize();
        atomic_json_write(root, "config.json", &normalized).await
    }
}

fn default_max_playlist_items() -> usize {
    25
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub title: String,
    pub url: String,
    pub platform: Platform,
    pub output_path: PathBuf,
    pub filesize_bytes: Option<u64>,
    pub completed_at_epoch_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryStore {
    pub entries: Vec<HistoryEntry>,
}

impl HistoryStore {
    pub async fn load_default() -> Result<Self, StorageError> {
        Self::load_from(&state_root()?).await
    }

    pub async fn save_default(&self) -> Result<(), StorageError> {
        self.save_to(&state_root()?).await
    }

    pub async fn load_from(root: &Path) -> Result<Self, StorageError> {
        let path = root.join("history.json");
        match fs::read(&path).await {
            Ok(bytes) => {
                let mut history = serde_json::from_slice::<Self>(&bytes)
                    .map_err(|source| StorageError::Json { path, source })?;
                history.entries.truncate(200);
                Ok(history)
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(StorageError::Read { path, source }),
        }
    }

    pub async fn save_to(&self, root: &Path) -> Result<(), StorageError> {
        atomic_json_write(root, "history.json", self).await
    }

    pub fn record(&mut self, entry: HistoryEntry) {
        self.entries.retain(|existing| {
            existing.url != entry.url && existing.output_path != entry.output_path
        });
        self.entries.insert(0, entry);
        self.entries.truncate(200);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

pub fn state_root() -> Result<PathBuf, StorageError> {
    dirs::data_dir()
        .map(|path| path.join("rust-dl"))
        .ok_or(StorageError::RootUnavailable)
}

pub fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

async fn atomic_json_write<T: Serialize>(
    root: &Path,
    filename: &str,
    value: &T,
) -> Result<(), StorageError> {
    fs::create_dir_all(root)
        .await
        .map_err(|source| StorageError::Write {
            path: root.to_path_buf(),
            source,
        })?;
    let path = root.join(filename);
    let temporary = root.join(format!("{filename}.tmp"));
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| StorageError::Json {
        path: path.clone(),
        source,
    })?;
    let mut file = fs::File::create(&temporary)
        .await
        .map_err(|source| StorageError::Write {
            path: temporary.clone(),
            source,
        })?;
    file.write_all(&bytes)
        .await
        .map_err(|source| StorageError::Write {
            path: temporary.clone(),
            source,
        })?;
    file.flush().await.map_err(|source| StorageError::Write {
        path: temporary.clone(),
        source,
    })?;
    drop(file);
    fs::rename(&temporary, &path)
        .await
        .map_err(|source| StorageError::Write { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn config_round_trips_and_clamps_concurrency() -> Result<(), Box<dyn std::error::Error>> {
        let root = temporary_root().await?;
        let config = AppConfig {
            max_concurrent_downloads: 99,
            ..AppConfig::default()
        };
        config.save_to(&root).await?;
        let loaded = AppConfig::load_from(&root).await?;
        assert_eq!(loaded.max_concurrent_downloads, 8);
        assert!(root.join("config.json").exists());
        remove_root(&root).await;
        Ok(())
    }

    #[tokio::test]
    async fn history_round_trips_deduplicates_and_evicts_old_entries(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = temporary_root().await?;
        let mut history = HistoryStore::default();
        for index in 0..201_u64 {
            history.record(HistoryEntry {
                title: format!("Title {index}"),
                url: format!("https://example.test/{index}"),
                platform: Platform::Generic,
                output_path: PathBuf::from(format!("/tmp/{index}.mp4")),
                filesize_bytes: Some(index),
                completed_at_epoch_secs: index,
            });
        }
        assert_eq!(history.entries.len(), 200);
        history.save_to(&root).await?;
        let loaded = HistoryStore::load_from(&root).await?;
        assert_eq!(loaded.entries.len(), 200);
        remove_root(&root).await;
        Ok(())
    }

    async fn temporary_root() -> Result<PathBuf, std::io::Error> {
        let root = std::env::temp_dir().join(format!("rust-dl-storage-{}", std::process::id()));
        match fs::remove_dir_all(&root).await {
            Ok(()) | Err(_) => {}
        }
        fs::create_dir_all(&root).await?;
        Ok(root)
    }

    async fn remove_root(root: &Path) {
        match fs::remove_dir_all(root).await {
            Ok(()) | Err(_) => {}
        }
    }
}
