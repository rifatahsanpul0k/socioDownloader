//! Headless E2E Test Harness for macOS Media Downloader GUI Rewrite.
//! Strictly derives expected contracts from ORIGINAL_REQUEST.md and PROJECT.md.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32};
use rust_dl_core::storage::HistoryEntry;
use rust_dl_core::{MediaManifest, MediaStream, Platform, StreamDelivery};

// ============================================================================
// HIG & Brand Design System Constants (Authoritative from PROJECT.md & R1-R5)
// ============================================================================

pub const MAC_OS_HEADER_HEIGHT: f32 = 34.0;
pub const MAC_OS_TRAFFIC_LIGHTS_INSET: f32 = 78.0;
pub const MAC_OS_CARD_CORNER_RADIUS: f32 = 10.0;

pub const COLOR_PROBING_BLUE: Color32 = Color32::from_rgb(0, 122, 255);       // #007AFF
pub const COLOR_DOWNLOADING_BLUE: Color32 = Color32::from_rgb(0, 122, 255);   // #007AFF
pub const COLOR_MUXING_PURPLE: Color32 = Color32::from_rgb(175, 82, 222);     // #AF52DE
pub const COLOR_COMPLETED_GREEN: Color32 = Color32::from_rgb(52, 199, 89);     // #34C759
pub const COLOR_AUTH_AMBER: Color32 = Color32::from_rgb(255, 149, 0);         // #FF9500
pub const COLOR_MAC_OS_DARK_BG: Color32 = Color32::from_rgb(30, 30, 30);       // #1E1E1E
pub const COLOR_MAC_OS_LIGHT_BG: Color32 = Color32::from_rgb(246, 246, 246);   // #F6F6F6

pub const CLEAR_COLOR_TRANSPARENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

pub const TEXT_PROBING_STATUS: &str = "Extracting Metadata...";
pub const TEXT_MUXING_STATUS: &str = "Merging Audio & Video...";
pub const TEXT_COMPLETED_STATUS: &str = "Completed.";
pub const TEXT_AUTH_REQUIRED_STATUS: &str = "Login required for this platform";
pub const TEXT_CLIPBOARD_TOAST: &str = "Clipboard link detected. Press Return to add.";

pub const BADGE_DASH_PRIMARY_DEFAULT: &str = "Highest Quality (1080p Video + High-Res Audio)";
pub const BADGE_DASH_SECONDARY_DEFAULT: &str = "🎬 H.264 + 🎵 AAC";

pub const BUTTON_CONNECT_COOKIES: &str = "Connect Browser Cookies";
pub const BUTTON_RETRY: &str = "Retry";
pub const BUTTON_SHOW_IN_FINDER: &str = "Show in Finder";
pub const BUTTON_UNDO: &str = "Undo";

pub const UNDO_TIMEOUT_SECS: f64 = 5.0;

// ============================================================================
// Staging Card Contract Types (PROJECT.md § staging.rs ↔ main.rs)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskCheckResult {
    pub required_bytes: u64,
    pub available_bytes: u64,
    pub is_sufficient: bool,
}

impl DiskCheckResult {
    pub fn new(required_bytes: u64, available_bytes: u64) -> Self {
        Self {
            required_bytes,
            available_bytes,
            is_sufficient: available_bytes >= required_bytes,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StagingState {
    Idle,
    Probing {
        url: String,
        started_at: std::time::Instant,
    },
    Staged {
        url: String,
        manifest: MediaManifest,
        thumbnail: Option<Vec<u8>>,
        selected_format_id: String,
        disk_check: DiskCheckResult,
    },
    Error {
        url: String,
        error: String,
    },
}

// ============================================================================
// Toast Management Contract Types (PROJECT.md § toast.rs ↔ main.rs)
// ============================================================================

#[derive(Debug, Clone)]
pub struct ToastManager {
    pub active_clipboard_url: Option<String>,
    pub last_clipboard_url: Option<String>,
    pub history_undo_entries: Vec<HistoryEntry>,
    pub history_undo_timer_remaining: f64,
}

impl Default for ToastManager {
    fn default() -> Self {
        Self {
            active_clipboard_url: None,
            last_clipboard_url: None,
            history_undo_entries: Vec::new(),
            history_undo_timer_remaining: 0.0,
        }
    }
}

impl ToastManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn trigger_clipboard_toast(&mut self, url: String) {
        if self.last_clipboard_url.as_deref() == Some(&url) {
            return; // Suppress duplicate
        }
        self.last_clipboard_url = Some(url.clone());
        self.active_clipboard_url = Some(url);
    }

    pub fn trigger_clear_history_toast(&mut self, cleared_items: Vec<HistoryEntry>) {
        if cleared_items.is_empty() {
            return;
        }
        self.history_undo_entries = cleared_items;
        self.history_undo_timer_remaining = UNDO_TIMEOUT_SECS;
    }

    pub fn tick(&mut self, delta_seconds: f64) {
        if self.history_undo_timer_remaining > 0.0 {
            self.history_undo_timer_remaining -= delta_seconds;
            if self.history_undo_timer_remaining <= 0.0 {
                self.history_undo_timer_remaining = 0.0;
                self.history_undo_entries.clear();
            }
        }
    }

    pub fn perform_undo(&mut self) -> Vec<HistoryEntry> {
        if self.history_undo_timer_remaining > 0.0 {
            self.history_undo_timer_remaining = 0.0;
            std::mem::take(&mut self.history_undo_entries)
        } else {
            Vec::new()
        }
    }

    pub fn handle_key_return(&mut self, input_has_focus: bool) -> Option<String> {
        if input_has_focus {
            return None;
        }
        self.active_clipboard_url.take()
    }
}

// ============================================================================
// Verification and Pure Contract Algorithms
// ============================================================================

pub fn evaluate_disk_space(available_bytes: u64, required_bytes: Option<u64>) -> DiskCheckResult {
    let req = required_bytes.unwrap_or(0);
    DiskCheckResult {
        required_bytes: req,
        available_bytes,
        is_sufficient: available_bytes >= req,
    }
}

pub fn derive_stream_badges(stream: &MediaStream) -> (String, String) {
    let is_split_dash = matches!(stream.delivery, StreamDelivery::SplitDash { .. });
    let is_1080p = stream.resolution.as_deref().unwrap_or("").contains("1080");

    if is_split_dash || (stream.format_id.contains("dash") && is_1080p) {
        (
            BADGE_DASH_PRIMARY_DEFAULT.to_string(),
            BADGE_DASH_SECONDARY_DEFAULT.to_string(),
        )
    } else if stream.vcodec.is_none() && stream.acodec.is_some() {
        (
            "Audio Only (High-Res M4A)".to_string(),
            "🎵 AAC".to_string(),
        )
    } else {
        let res = stream.resolution.as_deref().unwrap_or("Video");
        (
            format!("{} Video", res),
            format!("🎬 {}", stream.container.to_uppercase()),
        )
    }
}

pub fn mask_plain_language_error(raw_error: &str) -> String {
    let lower = raw_error.to_lowercase();
    if lower.contains("login required") || lower.contains("authentication required") || lower.contains("sign in") {
        TEXT_AUTH_REQUIRED_STATUS.to_string()
    } else if lower.contains("connection refused") || lower.contains("connection reset") || lower.contains("connect") || lower.contains("dns") || lower.contains("network") || lower.contains("timed out") || lower.contains("socket") {
        "Network connection error. Please check your internet connection.".to_string()
    } else if lower.contains("unsupported url") || lower.contains("not supported") {
        "Unsupported or invalid URL. Please check the media link.".to_string()
    } else if lower.contains("private video") || lower.contains("video unavailable") {
        "Media is private or unavailable.".to_string()
    } else if lower.contains("disk full") || lower.contains("no space left") {
        "Insufficient disk space to save the file.".to_string()
    } else {
        "Unable to complete download. Please check the link and try again.".to_string()
    }
}

pub fn format_download_metrics(downloaded_bytes: u64, total_bytes: Option<u64>, speed_bytes_per_sec: f64, eta_secs: Option<u64>) -> String {
    let human_downloaded = format_bytes(downloaded_bytes);
    let human_total = total_bytes.map(format_bytes).unwrap_or_else(|| "-- MB".to_string());
    let human_speed = format_speed(speed_bytes_per_sec);
    let human_eta = eta_secs.map(format_eta).unwrap_or_else(|| "-- left".to_string());

    format!("Downloading... {} / {} • {} • {}", human_downloaded, human_total, human_speed, human_eta)
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_speed(bytes_per_sec: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;

    if bytes_per_sec >= MB {
        format!("{:.1} MB/s", bytes_per_sec / MB)
    } else if bytes_per_sec >= KB {
        format!("{:.1} KB/s", bytes_per_sec / KB)
    } else if bytes_per_sec > 0.0 {
        format!("{:.0} B/s", bytes_per_sec)
    } else {
        "-- B/s".to_string()
    }
}

pub fn format_eta(secs: u64) -> String {
    if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{}h {}m left", hours, mins)
    } else if secs >= 60 {
        let mins = secs / 60;
        let remaining = secs % 60;
        format!("{}m {}s left", mins, remaining)
    } else {
        format!("{}s left", secs)
    }
}

// ============================================================================
// Headless Egui Testing Harness
// ============================================================================

pub struct HeadlessUiHarness {
    pub ctx: egui::Context,
}

impl Default for HeadlessUiHarness {
    fn default() -> Self {
        let ctx = egui::Context::default();
        Self { ctx }
    }
}

impl HeadlessUiHarness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render_frame<F, R>(&self, mut add_contents: F) -> (R, egui::FullOutput)
    where
        F: FnMut(&egui::Context) -> R,
    {
        let raw_input = egui::RawInput::default();
        let mut result = None;
        let output = self.ctx.run(raw_input, |ctx| {
            result = Some(add_contents(ctx));
        });
        (result.expect("frame produced result"), output)
    }

    pub fn render_frame_with_input<F, R>(&self, raw_input: egui::RawInput, mut add_contents: F) -> (R, egui::FullOutput)
    where
        F: FnMut(&egui::Context) -> R,
    {
        let mut result = None;
        let output = self.ctx.run(raw_input, |ctx| {
            result = Some(add_contents(ctx));
        });
        (result.expect("frame produced result"), output)
    }
}

// ============================================================================
// Codebase Static Analysis Helpers (Feature 18: Zero unwrap/expect)
// ============================================================================

#[derive(Debug, Default)]
pub struct CodeAuditReport {
    pub total_files_scanned: usize,
    pub unwrap_occurrences: Vec<(PathBuf, usize, String)>,
    pub expect_occurrences: Vec<(PathBuf, usize, String)>,
}

pub fn audit_gui_source_code(gui_src_dir: &Path) -> CodeAuditReport {
    let mut report = CodeAuditReport {
        total_files_scanned: 0,
        unwrap_occurrences: Vec::new(),
        expect_occurrences: Vec::new(),
    };

    if !gui_src_dir.exists() {
        return report;
    }

    let entries = match fs::read_dir(gui_src_dir) {
        Ok(e) => e,
        Err(_) => return report,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            report.total_files_scanned += 1;
            if let Ok(content) = fs::read_to_string(&path) {
                for (line_idx, line) in content.lines().enumerate() {
                    let trimmed = line.trim();
                    // Ignore comment lines
                    if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
                        continue;
                    }
                    if trimmed.contains(".unwrap()") {
                        report.unwrap_occurrences.push((path.clone(), line_idx + 1, trimmed.to_string()));
                    }
                    if trimmed.contains(".expect(") {
                        report.expect_occurrences.push((path.clone(), line_idx + 1, trimmed.to_string()));
                    }
                }
            }
        }
    }

    report
}

// ============================================================================
// Fixtures & Sample Manifests
// ============================================================================

pub fn make_history_entry(
    title: &str,
    url: &str,
    platform: Platform,
    path: &str,
    size: Option<u64>,
    epoch_secs: u64,
) -> HistoryEntry {
    HistoryEntry {
        title: title.to_string(),
        url: url.to_string(),
        platform,
        output_path: PathBuf::from(path),
        filesize_bytes: size,
        completed_at_epoch_secs: epoch_secs,
    }
}

pub fn sample_youtube_manifest() -> MediaManifest {
    MediaManifest {
        id: "dQw4w9WgXcQ".to_string(),
        title: "Rick Astley - Never Gonna Give You Up (Official Music Video)".to_string(),
        platform: Platform::YouTube,
        uploader: "Rick Astley".to_string(),
        duration_secs: Some(213.0),
        thumbnail_url: Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg".to_string()),
        streams: vec![
            MediaStream {
                format_id: "dash-1080p".to_string(),
                delivery: StreamDelivery::SplitDash {
                    video_url: "https://example.com/video-1080.mp4".to_string(),
                    audio_url: "https://example.com/audio.m4a".to_string(),
                },
                container: "mp4".to_string(),
                resolution: Some("1920x1080".to_string()),
                fps: Some(30.0),
                filesize_bytes: Some(45_000_000),
                vcodec: Some("avc1.640028".to_string()),
                acodec: Some("mp4a.40.2".to_string()),
                headers: HashMap::new(),
            },
            MediaStream {
                format_id: "audio-only".to_string(),
                delivery: StreamDelivery::Direct {
                    url: "https://example.com/audio.m4a".to_string(),
                },
                container: "m4a".to_string(),
                resolution: None,
                fps: None,
                filesize_bytes: Some(3_500_000),
                vcodec: None,
                acodec: Some("mp4a.40.2".to_string()),
                headers: HashMap::new(),
            },
        ],
    }
}
