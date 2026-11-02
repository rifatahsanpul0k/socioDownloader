//! Lightweight clipboard polling that remains testable without a desktop session.

use std::collections::HashSet;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::resolver::sanitizer::{sanitize_url, Platform};

pub fn extract_supported_clipboard_url(
    raw: &str,
    last_seen: &mut Option<String>,
) -> Option<String> {
    let candidate = raw.trim();
    if candidate.is_empty() || last_seen.as_deref() == Some(candidate) {
        return None;
    }
    *last_seen = Some(candidate.to_owned());
    let sanitized = sanitize_url(candidate).ok()?;
    if sanitized.platform != Platform::Generic || is_direct_media_url(&sanitized.url) {
        Some(sanitized.url.to_string())
    } else {
        None
    }
}

pub fn spawn_clipboard_watcher(mut enabled: watch::Receiver<bool>) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel(32);
    tokio::spawn(async move {
        let mut last_seen = None;
        let mut emitted = HashSet::new();
        loop {
            if !*enabled.borrow() {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(400)) => {}
                    changed = enabled.changed() => {
                        if changed.is_err() { break; }
                    }
                }
                continue;
            }
            if let Some(raw) = read_clipboard().await {
                if let Some(url) = extract_supported_clipboard_url(&raw, &mut last_seen) {
                    if emitted.insert(url.clone()) && sender.send(url).await.is_err() {
                        break;
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(400)) => {}
                changed = enabled.changed() => {
                    if changed.is_err() { break; }
                }
            }
        }
    });
    receiver
}

async fn read_clipboard() -> Option<String> {
    let output = Command::new("/usr/bin/pbpaste").output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn is_direct_media_url(url: &url::Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    [
        ".mp4", ".webm", ".m4a", ".mp3", ".aac", ".jpg", ".jpeg", ".png", ".gif", ".mov",
    ]
    .iter()
    .any(|extension| path.ends_with(extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_urls_and_suppresses_duplicates() {
        let mut last_seen = None;
        let first = extract_supported_clipboard_url(
            " https://youtu.be/video?si=tracking&t=12 ",
            &mut last_seen,
        );
        assert_eq!(first.as_deref(), Some("https://youtu.be/video?t=12"));
        assert!(extract_supported_clipboard_url(
            " https://youtu.be/video?si=tracking&t=12 ",
            &mut last_seen
        )
        .is_none());
    }

    #[test]
    fn accepts_direct_media_and_rejects_unrelated_generic_pages() {
        let mut last_seen = None;
        assert!(extract_supported_clipboard_url(
            "https://cdn.example.test/a/video.mp4?utm_source=x",
            &mut last_seen
        )
        .is_some());
        let mut last_seen = None;
        assert!(
            extract_supported_clipboard_url("https://example.test/article", &mut last_seen)
                .is_none()
        );
        assert!(extract_supported_clipboard_url("not a URL", &mut last_seen).is_none());
    }
}
