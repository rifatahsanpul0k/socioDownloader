//! Active QueueJob card visual presentation, HIG elevation, dynamic progress,
//! and actionable error recovery.
//!
//! Strictly enforces Apple HIG, Jakob Nielsen Usability Heuristics, and Zero Unwrap/Expect.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, RichText, Vec2};
use rust_dl_core::downloader::{DownloadPhase, DownloadProgress};
use rust_dl_core::queue::{JobId, JobStatus, QueueJob};
use rust_dl_core::Platform;

use crate::theme;

// ============================================================================
// Authoritative Color & Text Tokens (matching Apple HIG and E2E Test Suite)
// ============================================================================

pub const COLOR_PROBING_INDIGO: Color32 = Color32::from_rgb(88, 86, 214); // #5856D6
pub const COLOR_PROBING_BLUE: Color32 = COLOR_PROBING_INDIGO; // #5856D6 (aliased for compatibility)
pub const COLOR_DOWNLOADING_BLUE: Color32 = Color32::from_rgb(0, 122, 255); // #007AFF
pub const COLOR_MUXING_PURPLE: Color32 = Color32::from_rgb(175, 82, 222); // #AF52DE
pub const COLOR_COMPLETED_GREEN: Color32 = Color32::from_rgb(52, 199, 89); // #34C759
pub const COLOR_AUTH_AMBER: Color32 = Color32::from_rgb(255, 149, 0); // #FF9500

pub const TEXT_PROBING_STATUS: &str = "Finding your file...";
pub const TEXT_MUXING_STATUS: &str = "Adding high-quality sound...";
pub const TEXT_COMPLETED_STATUS: &str = "✓ Saved to Downloads";
pub const TEXT_AUTH_REQUIRED_STATUS: &str = "🔒 This video is private or age-restricted.";
pub const TEXT_NETWORK_ERROR_STATUS: &str =
    "Network connection error. Please check your internet connection.";
pub const TEXT_UNSUPPORTED_URL_STATUS: &str =
    "Unsupported or invalid URL. Please check the media link.";
pub const TEXT_PRIVATE_MEDIA_STATUS: &str = "Media is private or unavailable.";
pub const TEXT_DISK_FULL_STATUS: &str = "Insufficient disk space to save the file.";
pub const TEXT_GENERIC_ERROR_STATUS: &str =
    "Unable to complete download. Please check the link and try again.";

pub const BUTTON_CONNECT_COOKIES: &str = "Connect Browser Cookies";
pub const BUTTON_RETRY: &str = "Try Again";
pub const BUTTON_OPEN: &str = "Open";
pub const BUTTON_SHOW_IN_FINDER: &str = "Show in Finder";
pub const BUTTON_CANCEL: &str = "Cancel";
pub const BUTTON_REMOVE: &str = "Remove";

// ============================================================================
// HIG Elevation & Frame Styling
// ============================================================================

/// Constructs the HIG card frame with 10.0pt rounding and subtle elevation drop shadow.
///
/// If `is_amber` is true, tints the stroke, fill, and shadow to Apple System Amber (#FF9500).
pub fn queue_card_frame(visuals: &egui::Visuals, is_amber: bool) -> egui::Frame {
    let shadow_color = if is_amber {
        Color32::from_rgba_unmultiplied(255, 149, 0, 40)
    } else {
        Color32::from_black_alpha(25)
    };

    let mut frame = theme::card_frame(visuals)
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(egui::Margin::same(16.0))
        .stroke(egui::Stroke::new(
            1.0_f32,
            if is_amber {
                COLOR_AUTH_AMBER
            } else if visuals.dark_mode {
                Color32::from_rgba_unmultiplied(255, 255, 255, 20)
            } else {
                Color32::from_rgba_unmultiplied(0, 0, 0, 15)
            },
        ))
        .shadow(egui::epaint::Shadow {
            offset: Vec2::new(0.0, 2.0),
            blur: 8.0,
            spread: 0.0,
            color: shadow_color,
        });

    if is_amber {
        frame = frame
            .stroke(egui::Stroke::new(1.5_f32, COLOR_AUTH_AMBER))
            .fill(if visuals.dark_mode {
                Color32::from_rgba_unmultiplied(45, 36, 26, 235)
            } else {
                Color32::from_rgba_unmultiplied(255, 250, 240, 245)
            });
    }

    frame
}

// ============================================================================
// Platform Pill Badges
// ============================================================================

/// Returns badge icon, label, background fill, and foreground text color for a platform.
pub fn platform_badge_style(platform: Platform) -> (&'static str, &'static str, Color32, Color32) {
    match platform {
        Platform::YouTube => (
            "▶",
            "YouTube",
            Color32::from_rgba_unmultiplied(255, 59, 48, 30),
            Color32::from_rgb(255, 59, 48),
        ),
        Platform::Instagram => (
            "📷",
            "Instagram",
            Color32::from_rgba_unmultiplied(175, 82, 222, 30),
            Color32::from_rgb(175, 82, 222),
        ),
        Platform::Facebook => (
            "📘",
            "Facebook",
            Color32::from_rgba_unmultiplied(0, 122, 255, 30),
            Color32::from_rgb(0, 122, 255),
        ),
        Platform::Pinterest => (
            "📌",
            "Pinterest",
            Color32::from_rgba_unmultiplied(230, 0, 35, 30),
            Color32::from_rgb(230, 0, 35),
        ),
        Platform::GitHub => (
            "🐙",
            "GitHub",
            Color32::from_rgba_unmultiplied(130, 80, 223, 30),
            Color32::from_rgb(140, 95, 230),
        ),
        Platform::Generic => (
            "📦",
            "File",
            Color32::from_rgba_unmultiplied(142, 142, 147, 30),
            Color32::from_rgb(142, 142, 147),
        ),
    }
}

/// Renders a native-styled platform pill badge.
pub fn render_platform_badge(ui: &mut egui::Ui, platform: Platform) {
    let (icon, label, fill, text_color) = platform_badge_style(platform);
    egui::Frame::none()
        .fill(fill)
        .rounding(egui::Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new(icon).size(10.0).color(text_color));
                ui.label(RichText::new(label).strong().size(11.0).color(text_color));
            });
        });
}

// ============================================================================
// Pulsing Animation Math
// ============================================================================

/// Computes modulated alpha in range [0.40, 1.00] using sinusoidal oscillation.
///
/// Formula: (sin(time_seconds * speed_factor) * 0.3 + 0.7).clamp(0.0, 1.0)
#[inline]
pub fn calculate_pulse_alpha(time_seconds: f64, speed_factor: f64) -> f32 {
    let t = (time_seconds * speed_factor) as f32;
    (t.sin() * 0.3 + 0.7).clamp(0.0, 1.0)
}

/// Applies alpha modulation to an egui::Color32 without altering RGB components.
#[inline]
pub fn modulate_color_alpha(color: Color32, alpha: f32) -> Color32 {
    let unmultiplied = color.to_srgba_unmultiplied();
    let a_u8 = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(unmultiplied[0], unmultiplied[1], unmultiplied[2], a_u8)
}

// ============================================================================
// Metric Formatting Helpers (Zero Unwraps / Zero Expects)
// ============================================================================

/// Formats raw bytes into human-readable binary units (B, KB, MB, GB).
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

/// Formats transfer rate into human-readable units or "-- B/s".
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

/// Formats duration in seconds into human-readable ETA.
pub fn format_eta(secs: u64) -> String {
    if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{}h {}m left", hours, mins)
    } else if secs >= 60 {
        let mins = secs / 60;
        let remaining = secs % 60;
        format!("{}m {}s left", mins, remaining)
    } else if secs == 1 {
        "1 second left".to_string()
    } else {
        format!("{} seconds left", secs)
    }
}

/// Formats the Phase 2 metric line with non-jittering monospace numbers:
/// "Downloading — 64% • 5 seconds left"
pub fn format_download_metrics(
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    speed_bytes_per_sec: f64,
    eta_secs: Option<u64>,
) -> String {
    if let Some(total) = total_bytes.filter(|&t| t > 0) {
        let pct = (downloaded_bytes as f64 / total as f64 * 100.0).clamp(0.0, 100.0);
        let human_eta = eta_secs
            .map(format_eta)
            .unwrap_or_else(|| "-- left".to_string());
        format!("Downloading — {:.0}% • {}", pct, human_eta)
    } else if speed_bytes_per_sec > 0.0 {
        format!(
            "Downloading — {} • {}",
            format_bytes(downloaded_bytes),
            format_speed(speed_bytes_per_sec)
        )
    } else {
        format!("Downloading — {}", format_bytes(downloaded_bytes))
    }
}

// ============================================================================
// Plain-Language Error Masking & Classification (R4)
// ============================================================================

/// Determines if a raw error string indicates an authentication or rate-limiting failure.
pub fn is_auth_error(raw_error: &str) -> bool {
    let lower = raw_error.to_lowercase();
    lower.contains("login required")
        || lower.contains("authentication required")
        || lower.contains("authenticationrequired")
        || lower.contains("sign in")
        || lower.contains("bot verification")
        || lower.contains("confirm you're not a bot")
        || lower.contains("confirm you’re not a bot")
        || lower.contains("confirm your age")
        || lower.contains("rate-limit reached")
        || lower.contains("instagram api is not granting access")
        || lower.contains("members-only")
        || lower.contains("join this channel")
        || lower.contains("http error 401")
        || lower.contains("http 401")
        || lower.contains("http error 429")
        || lower.contains("http 429")
}

/// Translates raw backend errors, exit codes, and tracebacks into clean plain-language text.
pub fn mask_plain_language_error(raw_error: &str) -> String {
    let lower = raw_error.to_lowercase();

    if is_auth_error(raw_error) {
        TEXT_AUTH_REQUIRED_STATUS.to_string()
    } else if lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("network")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("socket")
        || lower.contains("wsaconnreset")
        || lower.contains("econnreset")
    {
        TEXT_NETWORK_ERROR_STATUS.to_string()
    } else if lower.contains("unsupported url")
        || lower.contains("not supported")
        || lower.contains("invalid url")
        || lower.contains("unsupported scheme")
    {
        TEXT_UNSUPPORTED_URL_STATUS.to_string()
    } else if lower.contains("private video")
        || lower.contains("video unavailable")
        || lower.contains("media unavailable")
        || lower.contains("not available")
    {
        TEXT_PRIVATE_MEDIA_STATUS.to_string()
    } else if lower.contains("disk full")
        || lower.contains("no space left")
        || lower.contains("insufficient disk space")
        || lower.contains("storage full")
        || lower.contains("out of memory")
    {
        TEXT_DISK_FULL_STATUS.to_string()
    } else {
        TEXT_GENERIC_ERROR_STATUS.to_string()
    }
}

// ============================================================================
// macOS Finder Reveal Helper
// ============================================================================

/// Reveals a file in macOS Finder using `open -R <path>`.
pub fn reveal_in_finder(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .status();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
    }
}

/// Opens a file with macOS default application using `open <path>`.
pub fn open_file(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(path)
            .status();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
    }
}

/// Derives friendly subtitle text for everyday users without technical jargon.
pub fn friendly_media_subtitle(job: &QueueJob) -> String {
    let label = match job.platform {
        Platform::GitHub => "📦 GitHub File",
        Platform::Generic => "📦 File",
        _ => {
            if let Some(ref fid) = job.selected_format_id {
                let lower = fid.to_lowercase();
                if lower.contains("audio") || lower == "m4a" || lower == "mp3" {
                    "🎵 Audio Track"
                } else if lower.contains("4k") || lower.contains("2160") {
                    "✨ 4K Video with Sound"
                } else {
                    "🎬 HD Video with Sound"
                }
            } else {
                "🎬 HD Video with Sound"
            }
        }
    };

    let size = match &job.status {
        JobStatus::Downloading { progress } => progress.total_bytes.map(format_bytes),
        _ => None,
    };

    if let Some(s) = size {
        format!("{label} • {s}")
    } else {
        label.to_string()
    }
}

// ============================================================================
// 4-Phase Dynamic Progress Indicators (R3)
// ============================================================================

/// Visual representation of the 4 progress phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualProgressPhase {
    /// Phase 1: Probing / Extracting metadata (#007AFF pulsing)
    Probing,
    /// Phase 2: Downloading data with metrics (#007AFF solid)
    Downloading,
    /// Phase 3: Muxing audio & video streams (#AF52DE pulsing)
    Muxing,
    /// Phase 4: Download completed (#34C759 solid)
    Completed,
}

/// Renders the dynamic progress bar with color, text, and animation modulation.
pub fn render_progress_indicator(
    ui: &mut egui::Ui,
    phase: VisualProgressPhase,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    speed_bytes_per_sec: f64,
    eta_secs: Option<u64>,
) -> egui::Response {
    let time = ui.input(|i| i.time);
    let pulse_alpha = calculate_pulse_alpha(time, 3.0);

    let (fraction, color, text, request_repaint) = match phase {
        VisualProgressPhase::Probing => {
            let color = modulate_color_alpha(COLOR_PROBING_BLUE, pulse_alpha);
            (0.5f32, color, TEXT_PROBING_STATUS.to_string(), true)
        }
        VisualProgressPhase::Downloading => {
            let fraction = total_bytes
                .filter(|&total| total > 0)
                .map(|total| (downloaded_bytes as f32 / total as f32).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let text = format_download_metrics(
                downloaded_bytes,
                total_bytes,
                speed_bytes_per_sec,
                eta_secs,
            );
            (fraction, COLOR_DOWNLOADING_BLUE, text, false)
        }
        VisualProgressPhase::Muxing => {
            let color = modulate_color_alpha(COLOR_MUXING_PURPLE, pulse_alpha);
            (0.95f32, color, TEXT_MUXING_STATUS.to_string(), true)
        }
        VisualProgressPhase::Completed => {
            (1.0f32, COLOR_COMPLETED_GREEN, TEXT_COMPLETED_STATUS.to_string(), false)
        }
    };

    if request_repaint {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    }

    let bar = egui::ProgressBar::new(fraction)
        .fill(color)
        .text(RichText::new(text).monospace());

    ui.add(bar)
}

/// Convenience dispatcher from `DownloadProgress`.
pub fn render_download_progress(
    ui: &mut egui::Ui,
    progress: &DownloadProgress,
) -> egui::Response {
    let phase = match &progress.phase {
        DownloadPhase::Probing => VisualProgressPhase::Probing,
        DownloadPhase::Downloading => VisualProgressPhase::Downloading,
        DownloadPhase::Muxing => VisualProgressPhase::Muxing,
        DownloadPhase::Completed { .. } => VisualProgressPhase::Completed,
        DownloadPhase::Failed { .. } | DownloadPhase::Cancelled => {
            VisualProgressPhase::Downloading
        }
    };

    render_progress_indicator(
        ui,
        phase,
        progress.downloaded_bytes,
        progress.total_bytes,
        progress.speed_bytes_per_sec,
        progress.eta_secs,
    )
}

// ============================================================================
// Actions & Modular Interface
// ============================================================================

/// User interaction actions emitted from a queue card.
pub enum QueueCardAction {
    None,
    Cancel(JobId),
    Retry(JobId),
    Remove(JobId),
    Open(PathBuf),
    ShowInFinder(PathBuf),
    OpenCookies,
}

/// Renders a queue card with standard retry/cancel/cookies callbacks.
pub fn render_queue_card(
    ui: &mut egui::Ui,
    job: &QueueJob,
    on_retry: impl FnOnce(JobId),
    on_cancel: impl FnOnce(JobId),
    on_open_cookies: impl FnOnce(),
) {
    render_queue_card_full(ui, job, on_retry, on_cancel, |_| {}, on_open_cookies);
}

/// Renders a queue card with full callback support including item removal and direct open.
pub fn render_queue_card_full(
    ui: &mut egui::Ui,
    job: &QueueJob,
    on_retry: impl FnOnce(JobId),
    on_cancel: impl FnOnce(JobId),
    on_remove: impl FnOnce(JobId),
    on_open_cookies: impl FnOnce(),
) {
    let is_amber = match &job.status {
        JobStatus::Failed { error } => is_auth_error(error),
        JobStatus::Downloading { progress } => match &progress.phase {
            DownloadPhase::Failed { error } => is_auth_error(error),
            _ => false,
        },
        _ => false,
    };

    let mut action = QueueCardAction::None;

    queue_card_frame(ui.visuals(), is_amber).show(ui, |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

        // Row 1: Platform Badge and Clean Media Title (no technical IDs)
        ui.horizontal(|ui| {
            render_platform_badge(ui, job.platform);
            let title = job.title.as_deref().unwrap_or(&job.url);
            ui.label(RichText::new(title).strong().size(14.0));
        });

        // Row 2: Friendly Subtitle & File Size (Heuristic #2: Zero technical jargon)
        ui.label(
            RichText::new(friendly_media_subtitle(job))
                .color(theme::TEXT_SECONDARY_DARK)
                .size(12.0),
        );

        // Row 3: Status / Progress Bar
        match &job.status {
            JobStatus::Queued => {
                ui.label(RichText::new("Queued").color(theme::TEXT_SECONDARY_DARK));
            }
            JobStatus::Resolving => {
                render_progress_indicator(ui, VisualProgressPhase::Probing, 0, None, 0.0, None);
            }
            JobStatus::Ready { .. } => {
                ui.label(RichText::new("Ready to download").color(COLOR_PROBING_INDIGO));
            }
            JobStatus::Downloading { progress } => match &progress.phase {
                DownloadPhase::Failed { error } => {
                    if is_auth_error(error) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("⚠️").size(14.0));
                            ui.label(
                                RichText::new(TEXT_AUTH_REQUIRED_STATUS)
                                    .strong()
                                    .color(COLOR_AUTH_AMBER)
                                    .size(13.0),
                            );
                        });
                        ui.label(
                            RichText::new(
                                "Sign in via your browser to download private or restricted content.",
                            )
                            .color(theme::TEXT_SECONDARY_DARK)
                            .size(11.5),
                        );
                    } else {
                        let masked = mask_plain_language_error(error);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("⚠️").size(14.0));
                            ui.label(RichText::new(masked).color(theme::COLOR_ACCENT_RED).size(12.5));
                        });
                    }
                }
                DownloadPhase::Cancelled => {
                    ui.label(RichText::new("Cancelled").color(theme::TEXT_SECONDARY_DARK));
                }
                _ => {
                    render_download_progress(ui, progress);
                }
            },
            JobStatus::Completed { output_path } => {
                ui.add(
                    egui::ProgressBar::new(1.0)
                        .fill(COLOR_COMPLETED_GREEN)
                        .text(RichText::new(TEXT_COMPLETED_STATUS).monospace()),
                );
                if let Some(file_name) = output_path.file_name() {
                    ui.label(
                        RichText::new(file_name.to_string_lossy())
                            .color(theme::TEXT_SECONDARY_DARK)
                            .size(11.0),
                    );
                }
            }
            JobStatus::Failed { error } => {
                if is_auth_error(error) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚠️").size(14.0));
                        ui.label(
                            RichText::new(TEXT_AUTH_REQUIRED_STATUS)
                                .strong()
                                .color(COLOR_AUTH_AMBER)
                                .size(13.0),
                        );
                    });
                    ui.label(
                        RichText::new(
                            "Sign in via your browser to download private or restricted content.",
                        )
                        .color(theme::TEXT_SECONDARY_DARK)
                        .size(11.5),
                    );
                } else {
                    let masked = mask_plain_language_error(error);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚠️").size(14.0));
                        ui.label(RichText::new(masked).color(theme::COLOR_ACCENT_RED).size(12.5));
                    });
                }
            }
            JobStatus::Cancelled => {
                ui.label(RichText::new("Cancelled").color(theme::TEXT_SECONDARY_DARK));
            }
        }

        // Row 4: Action Controls
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match &job.status {
                    JobStatus::Downloading { progress } => match &progress.phase {
                        DownloadPhase::Completed { output_path } => {
                            if ui.button(BUTTON_REMOVE).clicked() {
                                action = QueueCardAction::Remove(job.id);
                            }
                            if ui.button(BUTTON_SHOW_IN_FINDER).clicked() {
                                action = QueueCardAction::ShowInFinder(output_path.clone());
                            }
                            let open_btn = egui::Button::new(
                                RichText::new(BUTTON_OPEN).strong().color(COLOR_DOWNLOADING_BLUE),
                            );
                            if ui.add(open_btn).clicked() {
                                action = QueueCardAction::Open(output_path.clone());
                            }
                        }
                        DownloadPhase::Failed { error } => {
                            if ui.button(BUTTON_REMOVE).clicked() {
                                action = QueueCardAction::Remove(job.id);
                            }
                            if ui.button(BUTTON_RETRY).clicked() {
                                action = QueueCardAction::Retry(job.id);
                            }
                            if is_auth_error(error) {
                                let btn = egui::Button::new(
                                    RichText::new(BUTTON_CONNECT_COOKIES)
                                        .strong()
                                        .color(COLOR_AUTH_AMBER),
                                );
                                if ui.add(btn).clicked() {
                                    action = QueueCardAction::OpenCookies;
                                }
                            }
                        }
                        _ => {
                            if ui.button(BUTTON_CANCEL).clicked() {
                                action = QueueCardAction::Cancel(job.id);
                            }
                        }
                    },
                    JobStatus::Completed { output_path } => {
                        if ui.button(BUTTON_REMOVE).clicked() {
                            action = QueueCardAction::Remove(job.id);
                        }
                        if ui.button(BUTTON_SHOW_IN_FINDER).clicked() {
                            action = QueueCardAction::ShowInFinder(output_path.clone());
                        }
                        let open_btn = egui::Button::new(
                            RichText::new(BUTTON_OPEN).strong().color(COLOR_DOWNLOADING_BLUE),
                        );
                        if ui.add(open_btn).clicked() {
                            action = QueueCardAction::Open(output_path.clone());
                        }
                    }
                    JobStatus::Failed { error } => {
                        if ui.button(BUTTON_REMOVE).clicked() {
                            action = QueueCardAction::Remove(job.id);
                        }
                        if ui.button(BUTTON_RETRY).clicked() {
                            action = QueueCardAction::Retry(job.id);
                        }
                        if is_auth_error(error) {
                            let btn = egui::Button::new(
                                RichText::new(BUTTON_CONNECT_COOKIES)
                                    .strong()
                                    .color(COLOR_AUTH_AMBER),
                            );
                            if ui.add(btn).clicked() {
                                action = QueueCardAction::OpenCookies;
                            }
                        }
                    }
                    JobStatus::Cancelled => {
                        if ui.button(BUTTON_REMOVE).clicked() {
                            action = QueueCardAction::Remove(job.id);
                        }
                        if ui.button(BUTTON_RETRY).clicked() {
                            action = QueueCardAction::Retry(job.id);
                        }
                    }
                    JobStatus::Queued | JobStatus::Ready { .. } => {
                        if ui.button(BUTTON_REMOVE).clicked() {
                            action = QueueCardAction::Remove(job.id);
                        }
                    }
                    JobStatus::Resolving => {
                        if ui.button(BUTTON_CANCEL).clicked() {
                            action = QueueCardAction::Cancel(job.id);
                        }
                    }
                }
            });
        });
    });

    ui.add_space(8.0);

    match action {
        QueueCardAction::None => {}
        QueueCardAction::Cancel(id) => on_cancel(id),
        QueueCardAction::Retry(id) => on_retry(id),
        QueueCardAction::Remove(id) => on_remove(id),
        QueueCardAction::Open(path) => open_file(&path),
        QueueCardAction::ShowInFinder(path) => reveal_in_finder(&path),
        QueueCardAction::OpenCookies => on_open_cookies(),
    }
}

// ============================================================================
// Unit Tests (Strict Zero-Unwrap / Zero-Expect Guarantee)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_color_tokens() {
        assert_eq!(COLOR_PROBING_INDIGO, Color32::from_rgb(88, 86, 214));
        assert_eq!(COLOR_DOWNLOADING_BLUE, Color32::from_rgb(0, 122, 255));
        assert_eq!(COLOR_MUXING_PURPLE, Color32::from_rgb(175, 82, 222));
        assert_eq!(COLOR_COMPLETED_GREEN, Color32::from_rgb(52, 199, 89));
        assert_eq!(COLOR_AUTH_AMBER, Color32::from_rgb(255, 149, 0));
    }

    #[test]
    fn test_status_and_button_text_constants() {
        assert_eq!(TEXT_PROBING_STATUS, "Finding your file...");
        assert_eq!(TEXT_MUXING_STATUS, "Adding high-quality sound...");
        assert_eq!(TEXT_COMPLETED_STATUS, "✓ Saved to Downloads");
        assert_eq!(
            TEXT_AUTH_REQUIRED_STATUS,
            "🔒 This video is private or age-restricted."
        );
        assert_eq!(BUTTON_CONNECT_COOKIES, "Connect Browser Cookies");
        assert_eq!(BUTTON_RETRY, "Try Again");
        assert_eq!(BUTTON_OPEN, "Open");
        assert_eq!(BUTTON_SHOW_IN_FINDER, "Show in Finder");
    }

    #[test]
    fn test_pulse_alpha_sinusoidal_bounds() {
        let alpha_zero = calculate_pulse_alpha(0.0, 1.0);
        assert!((alpha_zero - 0.70).abs() < 1e-4);

        let alpha_peak = calculate_pulse_alpha(std::f64::consts::PI / 2.0, 1.0);
        assert!((alpha_peak - 1.00).abs() < 1e-4);

        let alpha_trough = calculate_pulse_alpha(3.0 * std::f64::consts::PI / 2.0, 1.0);
        assert!((alpha_trough - 0.40).abs() < 1e-4);

        for step in 0..100 {
            let t = step as f64 * 0.1;
            let a = calculate_pulse_alpha(t, 3.0);
            assert!((0.40..=1.00).contains(&a));
        }
    }

    #[test]
    fn test_modulate_color_alpha_preserves_rgb() {
        let base = Color32::from_rgb(10, 20, 30);
        let modulated = modulate_color_alpha(base, 0.5);
        let unmultiplied = modulated.to_srgba_unmultiplied();
        assert!((unmultiplied[0] as i32 - 10).abs() <= 1);
        assert!((unmultiplied[1] as i32 - 20).abs() <= 1);
        assert!((unmultiplied[2] as i32 - 30).abs() <= 1);
        assert_eq!(unmultiplied[3], 128);
    }

    #[test]
    fn test_format_bytes_scales() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(52_428_800), "50.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
        assert_eq!(format_bytes(2_684_354_560), "2.5 GB");
    }

    #[test]
    fn test_format_speed_scales() {
        assert_eq!(format_speed(0.0), "-- B/s");
        assert_eq!(format_speed(100.0), "100 B/s");
        assert_eq!(format_speed(1024.0), "1.0 KB/s");
        assert_eq!(format_speed(512_000.0), "500.0 KB/s");
        assert_eq!(format_speed(1_048_576.0), "1.0 MB/s");
        assert_eq!(format_speed(10_485_760.0), "10.0 MB/s");
    }

    #[test]
    fn test_format_eta_scales() {
        assert_eq!(format_eta(1), "1 second left");
        assert_eq!(format_eta(15), "15 seconds left");
        assert_eq!(format_eta(59), "59 seconds left");
        assert_eq!(format_eta(60), "1m 0s left");
        assert_eq!(format_eta(125), "2m 5s left");
        assert_eq!(format_eta(3600), "1h 0m left");
        assert_eq!(format_eta(3665), "1h 1m left");
        assert_eq!(format_eta(1_800_000), "500h 0m left");
    }

    #[test]
    fn test_format_download_metrics_complete_line() {
        let m = format_download_metrics(50_000_000, Some(100_000_000), 5_242_880.0, Some(10));
        assert_eq!(m, "Downloading — 50% • 10 seconds left");

        let zero = format_download_metrics(0, Some(0), 0.0, None);
        assert_eq!(zero, "Downloading — 0 B");

        let unknown_total = format_download_metrics(12_345_678, None, 1_000_000.0, None);
        assert_eq!(unknown_total, "Downloading — 11.8 MB • 976.6 KB/s");
    }

    #[test]
    fn test_auth_error_detection() {
        assert!(is_auth_error("Login required for YouTube"));
        assert!(is_auth_error("Authentication required for Instagram"));
        assert!(is_auth_error("Sign in to confirm your age"));
        assert!(is_auth_error("HTTP Error 401: Unauthorized"));
        assert!(is_auth_error("HTTP Error 429: Too Many Requests"));
        assert!(is_auth_error("Bot verification required"));
        assert!(!is_auth_error("Connection refused"));
        assert!(!is_auth_error("File not found"));
    }

    #[test]
    fn test_mask_plain_language_error_translations() {
        assert_eq!(
            mask_plain_language_error("Authentication required for Instagram: sessionid=xyz123"),
            TEXT_AUTH_REQUIRED_STATUS
        );
        assert_eq!(
            mask_plain_language_error("Connection reset by peer (os error 54)"),
            TEXT_NETWORK_ERROR_STATUS
        );
        assert_eq!(
            mask_plain_language_error("ERROR: Unsupported URL: https://example.com/stream"),
            TEXT_UNSUPPORTED_URL_STATUS
        );
        assert_eq!(
            mask_plain_language_error("Private video. Sign in if you've been granted access."),
            TEXT_AUTH_REQUIRED_STATUS
        );
        assert_eq!(
            mask_plain_language_error("No space left on device: writing /tmp/file"),
            TEXT_DISK_FULL_STATUS
        );
        assert_eq!(
            mask_plain_language_error("Something unexpected happened"),
            TEXT_GENERIC_ERROR_STATUS
        );
    }

    #[test]
    fn test_mask_plain_language_error_suppresses_technical_details() {
        let raw = "Process yt-dlp exited with status code 1: internal error\nstderr: Traceback (most recent call last):\n  File \"extractor.py\", line 42";
        let masked = mask_plain_language_error(raw);
        assert!(!masked.contains("exit code 1"));
        assert!(!masked.contains("status code"));
        assert!(!masked.contains("yt-dlp"));
        assert!(!masked.contains("stderr:"));
        assert!(!masked.contains("Traceback"));
        assert!(!masked.contains('\n'));
    }

    #[test]
    fn test_platform_badge_styles() {
        for platform in [
            Platform::YouTube,
            Platform::Instagram,
            Platform::Facebook,
            Platform::Pinterest,
            Platform::GitHub,
            Platform::Generic,
        ] {
            let (icon, label, fill, text_color) = platform_badge_style(platform);
            assert!(!icon.is_empty());
            assert!(!label.is_empty());
            assert!(fill.a() > 0);
            assert!(text_color.a() > 0);
        }
    }

    #[test]
    fn test_queue_card_frame_styling() {
        let visuals = egui::Visuals::dark();
        let normal_frame = queue_card_frame(&visuals, false);
        assert_eq!(normal_frame.rounding.nw, 12.0);
        assert_eq!(normal_frame.shadow.offset, Vec2::new(0.0, 2.0));
        assert_eq!(normal_frame.shadow.blur, 8.0);
        assert_eq!(normal_frame.shadow.color.a(), 25);

        let amber_frame = queue_card_frame(&visuals, true);
        assert_eq!(amber_frame.rounding.nw, 12.0);
        assert_eq!(amber_frame.stroke.width, 1.5);
        assert_eq!(amber_frame.stroke.color, COLOR_AUTH_AMBER);
    }

    #[test]
    fn test_headless_render_queue_card_lifecycle() {
        let ctx = egui::Context::default();
        let job = QueueJob {
            id: 42,
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_string(),
            platform: Platform::YouTube,
            title: Some("Never Gonna Give You Up".to_string()),
            selected_format_id: Some("137+140".to_string()),
            output_dir: PathBuf::from("/tmp"),
            status: JobStatus::Resolving,
        };

        let mut rendered = false;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_queue_card(ui, &job, |_| {}, |_| {}, || {});
                rendered = true;
            });
        });
        assert!(rendered);
    }

    #[test]
    fn test_headless_render_queue_card_downloading_phases() {
        let ctx = egui::Context::default();
        for phase in [
            DownloadPhase::Probing,
            DownloadPhase::Downloading,
            DownloadPhase::Muxing,
            DownloadPhase::Completed {
                output_path: PathBuf::from("/tmp/video.mp4"),
            },
        ] {
            let progress = DownloadProgress {
                phase,
                downloaded_bytes: 50_000_000,
                total_bytes: Some(100_000_000),
                speed_bytes_per_sec: 5_242_880.0,
                eta_secs: Some(10),
            };
            let job = QueueJob {
                id: 101,
                url: "https://instagram.com/reel/123".to_string(),
                platform: Platform::Instagram,
                title: Some("Reel".to_string()),
                selected_format_id: None,
                output_dir: PathBuf::from("/tmp"),
                status: JobStatus::Downloading { progress },
            };

            let mut rendered = false;
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render_queue_card_full(ui, &job, |_| {}, |_| {}, |_| {}, || {});
                    rendered = true;
                });
            });
            assert!(rendered);
        }
    }

    #[test]
    fn test_headless_render_queue_card_auth_and_network_errors() {
        let ctx = egui::Context::default();
        let auth_job = QueueJob {
            id: 202,
            url: "https://instagram.com/reel/auth_required".to_string(),
            platform: Platform::Instagram,
            title: Some("Private Reel".to_string()),
            selected_format_id: None,
            output_dir: PathBuf::from("/tmp"),
            status: JobStatus::Failed {
                error: "Authentication required for Instagram: login required".to_string(),
            },
        };

        let mut rendered_auth = false;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_queue_card_full(ui, &auth_job, |_| {}, |_| {}, |_| {}, || {});
                rendered_auth = true;
            });
        });
        assert!(rendered_auth);

        let net_job = QueueJob {
            id: 203,
            url: "https://example.com/stream".to_string(),
            platform: Platform::Generic,
            title: None,
            selected_format_id: None,
            output_dir: PathBuf::from("/tmp"),
            status: JobStatus::Failed {
                error: "Connection refused (os error 61)".to_string(),
            },
        };

        let mut rendered_net = false;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_queue_card_full(ui, &net_job, |_| {}, |_| {}, |_| {}, || {});
                rendered_net = true;
            });
        });
        assert!(rendered_net);
    }
}
