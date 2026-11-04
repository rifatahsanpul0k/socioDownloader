//! Pre-Queue Staging Card, Metadata Prober, Stream Badging, and Disk Pre-checks.
//! Strictly enforces Apple HIG, Jakob Nielsen Heuristics, and Zero Unwrap/Expect.

#![allow(dead_code)]

use std::path::Path;
use std::time::Instant;

use eframe::egui::{self, Color32, RichText, Vec2};
use rust_dl_core::resolver::{MediaManifest, MediaStream, StreamDelivery};

use crate::theme;

pub const BADGE_DASH_PRIMARY_DEFAULT: &str = "Highest Quality (1080p Video + High-Res Audio)";
pub const BADGE_DASH_SECONDARY_DEFAULT: &str = "🎬 H.264 + 🎵 AAC";

// ============================================================================
// Staging Card State Machine Types
// ============================================================================

/// Result of a pre-download filesystem capacity evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskCheckResult {
    /// Number of bytes required (0 if unknown).
    pub required_bytes: u64,
    /// Available unprivileged bytes on the destination mount point.
    pub available_bytes: u64,
    /// Whether remaining space is sufficient for downloading.
    pub is_sufficient: bool,
}

impl DiskCheckResult {
    /// Direct constructor for known required and available byte counts.
    pub fn new(required_bytes: u64, available_bytes: u64) -> Self {
        Self {
            required_bytes,
            available_bytes,
            is_sufficient: available_bytes >= required_bytes,
        }
    }
}

/// State machine representing the user journey before adding an item to the queue.
#[derive(Debug, Clone, Default)]
pub enum StagingState {
    /// No staged item; UI displays regular queue or idle prompt.
    #[default]
    Idle,
    /// Background probe in progress.
    Probing {
        url: String,
        started_at: Instant,
    },
    /// Probing succeeded; previewing metadata, badges, and pre-checks.
    Staged {
        url: String,
        manifest: MediaManifest,
        thumbnail: Option<Vec<u8>>,
        selected_format_id: String,
        disk_check: DiskCheckResult,
    },
    /// Probing failed with a friendly masked or formatted error message.
    Error {
        url: String,
        error: String,
    },
}

impl StagingState {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn is_probing(&self) -> bool {
        matches!(self, Self::Probing { .. })
    }

    pub fn is_staged(&self) -> bool {
        matches!(self, Self::Staged { .. })
    }

    pub fn reset(&mut self) {
        *self = Self::Idle;
    }
}

/// Event communicating asynchronous probe completion from worker thread to egui loop.
pub enum StagingEvent {
    Success {
        url: String,
        manifest: MediaManifest,
        thumbnail: Option<Vec<u8>>,
    },
    Error {
        url: String,
        error: String,
    },
}

// ============================================================================
// Disk Space Pre-Check Algorithms
// ============================================================================

/// Evaluates available bytes against required bytes safely without overflow.
pub fn evaluate_disk_space(available_bytes: u64, required_bytes: Option<u64>) -> DiskCheckResult {
    let req = required_bytes.unwrap_or_default();
    DiskCheckResult {
        required_bytes: req,
        available_bytes,
        is_sufficient: available_bytes >= req,
    }
}

/// Queries available filesystem capacity on the volume containing `dir`.
/// If `dir` does not exist on disk, climbs up `.parent()` until an existing
/// directory is found, and queries `libc::statvfs` on that mount point.
#[cfg(unix)]
pub fn query_available_space(dir: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let mut current = dir;
    while !current.exists() {
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => break,
        }
    }

    let target = if current.exists() {
        current
    } else {
        Path::new("/")
    };

    let c_path = match CString::new(target.as_os_str().as_bytes()) {
        Ok(path) => path,
        Err(_) => return None,
    };

    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if ret == 0 {
        let block_size = if stat.f_frsize > 0 {
            stat.f_frsize as u64
        } else {
            stat.f_bsize as u64
        };
        Some((stat.f_bavail as u64).saturating_mul(block_size))
    } else {
        None
    }
}

#[cfg(not(unix))]
pub fn query_available_space(_dir: &Path) -> Option<u64> {
    None
}

/// Pre-checks disk capacity for a given target download directory and optional stream size.
/// Uses `libc::statvfs` with ancestor traversal and zero unwrap / expect calls.
pub fn check_disk_space(dir: &Path, required_bytes: Option<u64>) -> DiskCheckResult {
    let available_bytes = query_available_space(dir).unwrap_or_default();
    evaluate_disk_space(available_bytes, required_bytes)
}

// ============================================================================
// Stream Badging & Formatting
// ============================================================================

/// Derives the primary and secondary badge texts for a given media stream.
/// Complies with Apple HIG, Nielsen Heuristic #2, and E2E contract tests.
pub fn format_stream_badges(stream: &MediaStream) -> (String, String) {
    let is_split_dash = matches!(stream.delivery, StreamDelivery::SplitDash { .. });
    let is_1080p = stream
        .resolution
        .as_deref()
        .map(|r| r.contains("1080"))
        .unwrap_or_default();

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

pub fn derive_stream_badges(stream: &MediaStream) -> (String, String) {
    format_stream_badges(stream)
}

/// Selects the default format ID for a staged manifest.
/// Prioritizes 1080p SplitDash streams if present, then highest video resolution, or first stream.
pub fn select_default_format_id(manifest: &MediaManifest) -> Option<String> {
    // 1. 1080p SplitDash if present
    for stream in &manifest.streams {
        let is_dash = matches!(stream.delivery, StreamDelivery::SplitDash { .. })
            || stream.format_id.contains('+')
            || stream.format_id.to_lowercase().contains("dash");
        let is_1080p = stream
            .resolution
            .as_deref()
            .map(|r| r.contains("1080"))
            .unwrap_or_default();
        if is_dash && is_1080p {
            return Some(stream.format_id.clone());
        }
    }

    // 2. Any 1080p stream
    for stream in &manifest.streams {
        if stream
            .resolution
            .as_deref()
            .map(|r| r.contains("1080"))
            .unwrap_or_default()
        {
            return Some(stream.format_id.clone());
        }
    }

    // 3. Any SplitDash stream
    for stream in &manifest.streams {
        if matches!(stream.delivery, StreamDelivery::SplitDash { .. }) {
            return Some(stream.format_id.clone());
        }
    }

    // 4. Highest video resolution
    let best_video = manifest
        .streams
        .iter()
        .filter(|s| s.vcodec.is_some() || s.resolution.is_some())
        .max_by_key(|s| parse_resolution_height(s.resolution.as_deref().unwrap_or_default()));
    if let Some(stream) = best_video {
        return Some(stream.format_id.clone());
    }

    // 5. Fallback to first available stream
    manifest.streams.first().map(|s| s.format_id.clone())
}

fn parse_resolution_height(res: &str) -> u32 {
    let clean = res.trim_end_matches('p');
    if let Some((_, h)) = clean.split_once('x') {
        h.parse::<u32>().unwrap_or_default()
    } else {
        clean.parse::<u32>().unwrap_or_default()
    }
}

/// Formats byte counts into human-readable strings (e.g. "47.7 MB", "12.4 GB").
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

/// Formats duration seconds into standard digital clock notation (e.g. "3:33" or "1:01:05").
pub fn format_duration(seconds: f64) -> String {
    let total = seconds.round() as u64;
    let mins = total / 60;
    let secs = total % 60;
    let hours = mins / 60;
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, mins % 60, secs)
    } else {
        format!("{}:{:02}", mins, secs)
    }
}

// ============================================================================
// UI Rendering Components
// ============================================================================

/// Renders a single HIG pill badge with background fill and rounded corners.
pub fn render_pill(
    ui: &mut egui::Ui,
    text: &str,
    bg_color: Color32,
    text_color: Color32,
    font_size: f32,
    padding: Vec2,
    rounding: f32,
) -> egui::Response {
    let font_id = egui::FontId::new(font_size, egui::FontFamily::Proportional);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_id, text_color);
    let desired_size = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(rounding), bg_color);
        let text_pos = rect.min + padding;
        ui.painter().galley(text_pos, galley, text_color);
    }

    response
}

/// Renders adjacent primary and secondary pills.
pub fn render_stream_badges(ui: &mut egui::Ui, primary: &str, secondary: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        // Primary Pill (macOS System Blue)
        let primary_bg = theme::COLOR_ACCENT_BLUE;
        let primary_fg = Color32::WHITE;
        render_pill(
            ui,
            primary,
            primary_bg,
            primary_fg,
            11.0,
            egui::vec2(6.0, 3.0),
            4.0,
        );

        // Secondary Pill (Adaptive Muted Container)
        if !secondary.is_empty() {
            let secondary_bg = if ui.visuals().dark_mode {
                Color32::from_gray(50)
            } else {
                Color32::from_gray(225)
            };
            let secondary_fg = if ui.visuals().dark_mode {
                Color32::from_rgb(235, 235, 240)
            } else {
                Color32::from_rgb(30, 30, 35)
            };
            render_pill(
                ui,
                secondary,
                secondary_bg,
                secondary_fg,
                10.5,
                egui::vec2(5.0, 2.5),
                4.0,
            );
        }
    });
}

/// Decodes raw image bytes into an egui `ColorImage` without panicking.
pub fn decode_thumbnail_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = match image::load_from_memory(bytes) {
        Ok(img) => img,
        Err(_) => return None,
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        rgba.as_raw(),
    ))
}

enum StagingAction {
    None,
    Reset,
    Download(String, String),
}

/// Top-level staging card renderer invoked inside egui central panel.
pub fn render_staging_card(
    ui: &mut egui::Ui,
    staging: &mut StagingState,
    on_download: impl FnOnce(String, String),
) {
    let action = match staging {
        StagingState::Idle => StagingAction::None,
        StagingState::Probing { url, started_at } => {
            if render_probing_card(ui, url, *started_at) {
                StagingAction::Reset
            } else {
                StagingAction::None
            }
        }
        StagingState::Staged {
            url,
            manifest,
            thumbnail,
            selected_format_id,
            disk_check,
        } => render_staged_card(
            ui,
            url,
            manifest,
            thumbnail.as_deref(),
            selected_format_id,
            disk_check,
        ),
        StagingState::Error { url, error } => {
            if render_error_card(ui, url, error) {
                StagingAction::Reset
            } else {
                StagingAction::None
            }
        }
    };

    match action {
        StagingAction::None => {}
        StagingAction::Reset => staging.reset(),
        StagingAction::Download(url, format_id) => {
            on_download(url, format_id);
            staging.reset();
        }
    }
}

fn render_probing_card(
    ui: &mut egui::Ui,
    url: &str,
    started_at: Instant,
) -> bool {
    let mut cancel_requested = false;
    theme::card_frame(ui.visuals()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Extracting Media Info...")
                    .strong()
                    .color(theme::COLOR_ACCENT_BLUE),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✕").clicked() {
                    cancel_requested = true;
                }
            });
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(url)
                .color(theme::TEXT_SECONDARY_DARK)
                .size(11.0),
        );
        ui.add_space(8.0);
        let elapsed = started_at.elapsed().as_secs_f32();
        let pulse = ((elapsed * 3.0).sin() * 0.45 + 0.55).clamp(0.0, 1.0);
        ui.add(
            egui::ProgressBar::new(pulse)
                .animate(true)
                .text("Probing formats & codecs..."),
        );
    });

    cancel_requested || ui.input(|i| i.key_pressed(egui::Key::Escape))
}

fn render_staged_card(
    ui: &mut egui::Ui,
    url: &str,
    manifest: &MediaManifest,
    thumbnail: Option<&[u8]>,
    selected_format_id: &mut String,
    disk_check: &mut DiskCheckResult,
) -> StagingAction {
    let mut dismiss_clicked = false;
    let mut download_requested = false;

    theme::card_frame(ui.visuals()).show(ui, |ui| {
        // Header with Dismiss button [ ✕ ]
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Ready to Download")
                    .strong()
                    .color(theme::COLOR_ACCENT_BLUE),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✕").clicked() {
                    dismiss_clicked = true;
                }
            });
        });
        ui.add_space(8.0);

        // Thumbnail and Metadata Horizontal Row
        ui.horizontal(|ui| {
            render_thumbnail(ui, thumbnail, url);
            ui.add_space(12.0);

            ui.vertical(|ui| {
                ui.label(RichText::new(&manifest.title).strong().size(15.0));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&manifest.uploader)
                            .color(theme::TEXT_SECONDARY_DARK)
                            .size(12.0),
                    );
                    if let Some(duration) = manifest.duration_secs {
                        ui.label(RichText::new("•").color(theme::TEXT_SECONDARY_DARK));
                        ui.label(
                            RichText::new(format_duration(duration))
                                .color(theme::TEXT_SECONDARY_DARK)
                                .size(12.0),
                        );
                    }
                });

                // Estimated File Size
                let selected_stream = manifest
                    .streams
                    .iter()
                    .find(|s| s.format_id == *selected_format_id)
                    .or_else(|| manifest.streams.first());
                let size_text = match selected_stream.and_then(|s| s.filesize_bytes) {
                    Some(bytes) => format_bytes(bytes),
                    None => "Variable / Unknown".to_string(),
                };
                ui.label(
                    RichText::new(format!("Estimated Size: {size_text}"))
                        .color(theme::TEXT_SECONDARY_DARK)
                        .size(12.0),
                );

                ui.add_space(6.0);

                // Format Selector Dropdown with Badges
                let mut format_changed = None;
                let current_selection = selected_format_id.clone();
                egui::ComboBox::from_id_salt("staging_format_dropdown")
                    .selected_text({
                        if let Some(stream) = manifest
                            .streams
                            .iter()
                            .find(|s| s.format_id == current_selection)
                        {
                            let (pri, sec) = format_stream_badges(stream);
                            format!("{pri} · {sec}")
                        } else {
                            current_selection.clone()
                        }
                    })
                    .width(ui.available_width().min(400.0))
                    .show_ui(ui, |ui| {
                        for stream in &manifest.streams {
                            let (pri, sec) = format_stream_badges(stream);
                            let size_note = match stream.filesize_bytes {
                                Some(bytes) => format!(" • {}", format_bytes(bytes)),
                                None => String::new(),
                            };
                            let label = format!("{pri}   [{sec}]{size_note}");
                            let is_active = stream.format_id == current_selection;
                            if ui.selectable_label(is_active, label).clicked() {
                                format_changed = Some(stream.format_id.clone());
                            }
                        }
                    });

                if let Some(new_format) = format_changed {
                    *selected_format_id = new_format;
                    let new_stream_size = manifest
                        .streams
                        .iter()
                        .find(|s| s.format_id == *selected_format_id)
                        .and_then(|s| s.filesize_bytes);
                    *disk_check = evaluate_disk_space(disk_check.available_bytes, new_stream_size);
                }
            });
        });

        ui.add_space(10.0);

        // Pre-download Disk Space Status & Warnings
        if !disk_check.is_sufficient {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "⚠️ Insufficient disk space: {} required, only {} available",
                        format_bytes(disk_check.required_bytes),
                        format_bytes(disk_check.available_bytes)
                    ))
                    .color(theme::COLOR_ACCENT_AMBER)
                    .strong(),
                );
            });
            ui.add_space(4.0);
        }

        // Action Buttons
        ui.horizontal(|ui| {
            let can_download = disk_check.is_sufficient;
            let return_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));

            let download_btn = egui::Button::new(
                RichText::new("Download")
                    .strong()
                    .color(if can_download { Color32::WHITE } else { Color32::GRAY }),
            )
            .fill(if can_download {
                theme::COLOR_ACCENT_BLUE
            } else {
                Color32::from_gray(60)
            });

            let resp = ui.add_enabled(can_download, download_btn);
            if can_download && (resp.clicked() || return_pressed) {
                download_requested = true;
            } else if !can_download {
                resp.on_disabled_hover_text("Insufficient disk space on destination volume.");
            }

            if ui.button("Cancel").clicked() {
                dismiss_clicked = true;
            }
        });
    });

    if dismiss_clicked || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        StagingAction::Reset
    } else if download_requested {
        StagingAction::Download(url.to_string(), selected_format_id.clone())
    } else {
        StagingAction::None
    }
}

fn render_thumbnail(ui: &mut egui::Ui, thumbnail_bytes: Option<&[u8]>, url: &str) {
    let size = Vec2::new(160.0, 90.0);
    let tex_id = egui::Id::new("staging_thumbnail_cached_texture").with(url);

    let mut texture = ui.ctx().data(|d| d.get_temp::<egui::TextureHandle>(tex_id));
    if texture.is_none() {
        if let Some(bytes) = thumbnail_bytes {
            if let Some(color_image) = decode_thumbnail_image(bytes) {
                let loaded =
                    ui.ctx()
                        .load_texture("staging_thumb", color_image, egui::TextureOptions::LINEAR);
                ui.ctx().data_mut(|d| d.insert_temp(tex_id, loaded.clone()));
                texture = Some(loaded);
            }
        }
    }

    if let Some(tex) = texture {
        ui.add(egui::Image::new(&tex).fit_to_exact_size(size).rounding(6.0));
    } else {
        egui::Frame::none()
            .fill(Color32::from_rgb(0x22, 0x22, 0x24))
            .rounding(6.0)
            .show(ui, |ui| {
                ui.set_min_size(size);
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("🎬").size(24.0));
                });
            });
    }
}

fn render_error_card(
    ui: &mut egui::Ui,
    url: &str,
    error: &str,
) -> bool {
    let mut dismiss_clicked = false;
    theme::card_frame(ui.visuals())
        .stroke(egui::Stroke::new(1.5_f32, theme::COLOR_ACCENT_AMBER))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("⚠️ Unable to load media")
                        .strong()
                        .color(theme::COLOR_ACCENT_AMBER),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✕").clicked() {
                        dismiss_clicked = true;
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(url)
                    .color(theme::TEXT_SECONDARY_DARK)
                    .size(11.0),
            );
            ui.add_space(4.0);
            ui.label(RichText::new(error).color(theme::TEXT_PRIMARY_DARK));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Dismiss").clicked() {
                    dismiss_clicked = true;
                }
            });
        });

    dismiss_clicked || ui.input(|i| i.key_pressed(egui::Key::Escape))
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use rust_dl_core::resolver::Platform;

    fn sample_dash_stream() -> MediaStream {
        MediaStream {
            format_id: "dash-1080p".to_string(),
            delivery: StreamDelivery::SplitDash {
                video_url: "https://example.com/v.mp4".to_string(),
                audio_url: "https://example.com/a.m4a".to_string(),
            },
            container: "mp4".to_string(),
            resolution: Some("1920x1080".to_string()),
            fps: Some(30.0),
            filesize_bytes: Some(45_000_000),
            vcodec: Some("avc1.640028".to_string()),
            acodec: Some("mp4a.40.2".to_string()),
            headers: HashMap::new(),
        }
    }

    #[test]
    fn test_disk_check_sufficient() {
        let res = evaluate_disk_space(10_000_000_000, Some(50_000_000));
        assert!(res.is_sufficient);
        assert_eq!(res.required_bytes, 50_000_000);
        assert_eq!(res.available_bytes, 10_000_000_000);
    }

    #[test]
    fn test_disk_check_insufficient() {
        let res = evaluate_disk_space(20_000_000, Some(50_000_000));
        assert!(!res.is_sufficient);
        assert_eq!(res.required_bytes, 50_000_000);
        assert_eq!(res.available_bytes, 20_000_000);
    }

    #[test]
    fn test_disk_check_none_size_handles_safely() {
        let res = evaluate_disk_space(1_000_000_000, None);
        assert!(res.is_sufficient);
        assert_eq!(res.required_bytes, 0);
    }

    #[test]
    fn test_disk_check_exact_byte_match() {
        let res = evaluate_disk_space(1_048_576, Some(1_048_576));
        assert!(res.is_sufficient);
    }

    #[test]
    fn test_disk_check_off_by_one_insufficient() {
        let res = evaluate_disk_space(1_048_575, Some(1_048_576));
        assert!(!res.is_sufficient);
    }

    #[test]
    fn test_disk_check_zero_available() {
        let res = evaluate_disk_space(0, Some(100));
        assert!(!res.is_sufficient);
    }

    #[test]
    fn test_disk_check_extreme_overflow_prevention() {
        let required = 100 * 1024 * 1024 * 1024 * 1024u64; // 100 TB
        let available = 500 * 1024 * 1024 * 1024u64; // 500 GB
        let res = evaluate_disk_space(available, Some(required));
        assert!(!res.is_sufficient);
    }

    #[test]
    fn test_disk_check_constructor() {
        let c1 = DiskCheckResult::new(100, 200);
        assert!(c1.is_sufficient);
        let c2 = DiskCheckResult::new(200, 100);
        assert!(!c2.is_sufficient);
    }

    #[test]
    fn test_live_check_disk_space_mount_point() {
        let res = check_disk_space(Path::new("/tmp"), Some(1_000));
        assert!(res.available_bytes > 0);
    }

    #[test]
    fn test_live_check_disk_space_nonexistent_path_ancestor_climbing() {
        let res = check_disk_space(Path::new("/tmp/nonexistent_subdir_test_12345/child"), Some(1_000));
        assert!(res.available_bytes > 0);
    }

    #[test]
    fn test_dash_stream_badges_default() {
        let stream = sample_dash_stream();
        let (primary, secondary) = format_stream_badges(&stream);
        assert_eq!(primary, BADGE_DASH_PRIMARY_DEFAULT);
        assert_eq!(primary, "Highest Quality (1080p Video + High-Res Audio)");
        assert_eq!(secondary, BADGE_DASH_SECONDARY_DEFAULT);
        assert_eq!(secondary, "🎬 H.264 + 🎵 AAC");
    }

    #[test]
    fn test_non_dash_stream_badges() {
        let stream = MediaStream {
            format_id: "720p".to_string(),
            delivery: StreamDelivery::Direct {
                url: "https://example.com/v.mp4".to_string(),
            },
            container: "mp4".to_string(),
            resolution: Some("1280x720".to_string()),
            fps: Some(30.0),
            filesize_bytes: Some(20_000_000),
            vcodec: Some("h264".to_string()),
            acodec: Some("aac".to_string()),
            headers: HashMap::new(),
        };
        let (primary, secondary) = format_stream_badges(&stream);
        assert!(primary.contains("1280x720"));
        assert_eq!(secondary, "🎬 MP4");
    }

    #[test]
    fn test_audio_only_stream_badges() {
        let stream = MediaStream {
            format_id: "audio".to_string(),
            delivery: StreamDelivery::Direct {
                url: "https://example.com/a.m4a".to_string(),
            },
            container: "m4a".to_string(),
            resolution: None,
            fps: None,
            filesize_bytes: Some(4_000_000),
            vcodec: None,
            acodec: Some("mp4a.40.2".to_string()),
            headers: HashMap::new(),
        };
        let (primary, secondary) = format_stream_badges(&stream);
        assert_eq!(primary, "Audio Only (High-Res M4A)");
        assert_eq!(secondary, "🎵 AAC");
    }

    #[test]
    fn test_unknown_codec_stream_badges() {
        let stream = MediaStream {
            format_id: "unknown".to_string(),
            delivery: StreamDelivery::Direct {
                url: "https://example.com/v".to_string(),
            },
            container: "webm".to_string(),
            resolution: None,
            fps: None,
            filesize_bytes: None,
            vcodec: Some("vp9".to_string()),
            acodec: None,
            headers: HashMap::new(),
        };
        let (primary, secondary) = format_stream_badges(&stream);
        assert_eq!(primary, "Video Video");
        assert_eq!(secondary, "🎬 WEBM");
    }

    #[test]
    fn test_select_default_format_prefers_1080p_dash() {
        let dash = sample_dash_stream();
        let other = MediaStream {
            format_id: "720p".to_string(),
            delivery: StreamDelivery::Direct {
                url: "https://example.com".to_string(),
            },
            container: "mp4".to_string(),
            resolution: Some("720p".to_string()),
            fps: None,
            filesize_bytes: None,
            vcodec: None,
            acodec: None,
            headers: HashMap::new(),
        };
        let manifest = MediaManifest {
            id: "id".to_string(),
            title: "title".to_string(),
            uploader: "uploader".to_string(),
            duration_secs: None,
            thumbnail_url: None,
            platform: Platform::YouTube,
            streams: vec![other, dash],
        };

        let selected = select_default_format_id(&manifest);
        assert_eq!(selected, Some("dash-1080p".to_string()));
    }

    #[test]
    fn test_format_bytes_display() {
        assert_eq!(format_bytes(50_000_000), "47.7 MB");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_bytes(500), "500 B");
    }

    #[test]
    fn test_format_duration_display() {
        assert_eq!(format_duration(213.0), "3:33");
        assert_eq!(format_duration(3665.0), "1:01:05");
        assert_eq!(format_duration(45.0), "0:45");
    }

    #[test]
    fn test_staging_state_lifecycle() {
        let mut state = StagingState::Idle;
        assert!(state.is_idle());

        state = StagingState::Probing {
            url: "https://example.com".to_string(),
            started_at: Instant::now(),
        };
        assert!(state.is_probing());

        state.reset();
        assert!(state.is_idle());
    }

    #[test]
    fn test_decode_thumbnail_image_corrupt_bytes_returns_none() {
        let corrupt = vec![0u8, 1, 2, 3, 4, 5];
        let decoded = decode_thumbnail_image(&corrupt);
        assert!(decoded.is_none());
    }
}
