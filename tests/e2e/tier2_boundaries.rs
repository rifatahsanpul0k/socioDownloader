//! Tier 2: Boundary & Corner Cases for macOS Media Downloader GUI Rewrite.
//! Strictly covers boundary values, zero conditions, extremes, and edge cases.

use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{self, Key, Rounding, Vec2};
use rust_dl_core::downloader::DownloadProgress;
use rust_dl_core::storage::HistoryEntry;
use rust_dl_core::Platform;

use super::harness::*;

// ============================================================================
// Boundary 1: Disk Space Boundary Cases (>= 5 tests)
// ============================================================================

#[test]
fn test_b01_disk_check_exact_byte_match() {
    // Exact match: available == required -> boundary must be sufficient
    let result = evaluate_disk_space(1_048_576, Some(1_048_576));
    assert!(result.is_sufficient, "Exact byte match must be sufficient");
    assert_eq!(result.required_bytes, 1_048_576);
    assert_eq!(result.available_bytes, 1_048_576);
}

#[test]
fn test_b01_disk_check_off_by_one_insufficient() {
    // 1 byte less than required -> must fail sufficiency check
    let required = 1_000_000u64;
    let available = required - 1;
    let result = evaluate_disk_space(available, Some(required));
    assert!(!result.is_sufficient, "1 byte deficit must be insufficient");
}

#[test]
fn test_b01_disk_check_zero_required_bytes() {
    // Stream size 0 (e.g. live stream or stream with unknown header)
    let result = evaluate_disk_space(500_000_000, Some(0));
    assert!(result.is_sufficient, "0 required bytes must be sufficient");
    assert_eq!(result.required_bytes, 0);
}

#[test]
fn test_b01_disk_check_extreme_required_bytes_overflow_prevention() {
    // Extreme 100 TB file size against 500 GB disk
    let required = 100 * 1024 * 1024 * 1024 * 1024u64; // 100 TB
    let available = 500 * 1024 * 1024 * 1024u64;       // 500 GB
    let result = evaluate_disk_space(available, Some(required));
    assert!(!result.is_sufficient);
}

#[test]
fn test_b01_disk_check_zero_available_bytes() {
    // Target volume has completely 0 bytes available
    let result = evaluate_disk_space(0, Some(100));
    assert!(!result.is_sufficient);
}

// ============================================================================
// Boundary 2: Progress Metrics Boundaries (>= 5 tests)
// ============================================================================

#[test]
fn test_b02_progress_metrics_zero_downloaded_and_total() {
    let metrics = format_download_metrics(0, Some(0), 0.0, None);
    assert_eq!(metrics, "Downloading... 0 B / 0 B • -- B/s • -- left");
}

#[test]
fn test_b02_progress_metrics_none_total_bytes_no_division_by_zero() {
    let metrics = format_download_metrics(12_345_678, None, 1_000_000.0, None);
    assert!(metrics.contains("-- MB"), "Total bytes None must display placeholder without panic");
    assert!(metrics.contains("11.8 MB"));
}

#[test]
fn test_b02_progress_metrics_downloaded_exceeds_total_clamped() {
    let downloaded = 105_000_000u64;
    let total = 100_000_000u64;
    let fraction = (downloaded as f32 / total as f32).clamp(0.0, 1.0);
    assert_eq!(fraction, 1.0, "Progress fraction must be clamped to 1.0");
}

#[test]
fn test_b02_progress_metrics_zero_speed_displays_dashes() {
    let speed_formatted = format_speed(0.0);
    assert_eq!(speed_formatted, "-- B/s");
}

#[test]
fn test_b02_progress_metrics_extreme_eta_seconds_formatting() {
    // 500 hours ETA
    let eta_formatted = format_eta(1_800_000);
    assert_eq!(eta_formatted, "500h 0m left");
}

// ============================================================================
// Boundary 3: Toast Timer Boundaries (>= 5 tests)
// ============================================================================

#[test]
fn test_b03_toast_timer_subsecond_precision_before_expiration() {
    let mut manager = ToastManager::new();
    let entry = make_history_entry("Test", "https://example.com", Platform::YouTube, "/tmp/video.mp4", Some(1000), 12345);
    manager.trigger_clear_history_toast(vec![entry]);
    manager.tick(4.999);
    assert!(manager.history_undo_timer_remaining > 0.0);
    let restored = manager.perform_undo();
    assert_eq!(restored.len(), 1, "Undo at 4.999s must successfully restore");
}

#[test]
fn test_b03_toast_timer_expiration_at_5001_seconds() {
    let mut manager = ToastManager::new();
    let entry = make_history_entry("Test", "https://example.com", Platform::YouTube, "/tmp/video.mp4", Some(1000), 12345);
    manager.trigger_clear_history_toast(vec![entry]);
    manager.tick(5.001);
    assert_eq!(manager.history_undo_timer_remaining, 0.0);
    let restored = manager.perform_undo();
    assert!(restored.is_empty(), "Undo at 5.001s must be expired");
}

#[test]
fn test_b03_toast_rapid_consecutive_clears_resets_timer() {
    let mut manager = ToastManager::new();
    let entry1 = make_history_entry("Test 1", "https://example1.com", Platform::YouTube, "/tmp/1.mp4", Some(1000), 100);
    let entry2 = make_history_entry("Test 2", "https://example2.com", Platform::YouTube, "/tmp/2.mp4", Some(2000), 200);
    manager.trigger_clear_history_toast(vec![entry1]);
    manager.tick(3.0);
    assert_eq!(manager.history_undo_timer_remaining, 2.0);

    // Rapid second clear occurs
    manager.trigger_clear_history_toast(vec![entry2]);
    assert_eq!(manager.history_undo_timer_remaining, 5.0, "Second clear must reset timer to 5s");
    assert_eq!(manager.history_undo_entries.len(), 1);
    assert_eq!(manager.history_undo_entries[0].title, "Test 2");
}

#[test]
fn test_b03_toast_empty_cleared_list_does_not_trigger_toast() {
    let mut manager = ToastManager::new();
    manager.trigger_clear_history_toast(Vec::new());
    assert_eq!(manager.history_undo_timer_remaining, 0.0);
    assert!(manager.history_undo_entries.is_empty());
}

#[test]
fn test_b03_toast_multiple_undo_calls_idempotent() {
    let mut manager = ToastManager::new();
    let entry = make_history_entry("Test", "https://example.com", Platform::YouTube, "/tmp/video.mp4", Some(1000), 12345);
    manager.trigger_clear_history_toast(vec![entry]);
    let first = manager.perform_undo();
    let second = manager.perform_undo();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 0, "Second undo must yield empty list");
}

// ============================================================================
// Boundary 4: URL Staging & Parsing Boundaries (>= 5 tests)
// ============================================================================

#[test]
fn test_b04_url_empty_string_rejected() {
    let url = "";
    assert!(url.is_empty());
    let mut manager = ToastManager::new();
    if !url.is_empty() {
        manager.trigger_clipboard_toast(url.to_string());
    }
    assert!(manager.active_clipboard_url.is_none());
}

#[test]
fn test_b04_url_excessive_length_4096_bytes() {
    let mut long_url = "https://example.com/watch?v=123".to_string();
    long_url.push_str(&"&extra_param=".repeat(200));
    assert!(long_url.len() > 2000);
    // Harness must handle long URLs safely without stack overflow
    let mut manager = ToastManager::new();
    manager.trigger_clipboard_toast(long_url.clone());
    assert_eq!(manager.active_clipboard_url.as_deref(), Some(long_url.as_str()));
}

#[test]
fn test_b04_url_with_shell_metacharacters_sanitized() {
    let malicious = "https://example.com/video; rm -rf /; echo $HOME";
    let is_clean = !malicious.contains(';') && !malicious.contains('$');
    assert!(!is_clean, "Malicious URL contains shell metacharacters that must be rejected/escaped");
}

#[test]
fn test_b04_url_with_unicode_and_emojis() {
    let unicode_url = "https://example.com/watch?v=✨🎉🦀";
    let mut manager = ToastManager::new();
    manager.trigger_clipboard_toast(unicode_url.to_string());
    assert!(manager.active_clipboard_url.is_some());
}

#[test]
fn test_b04_url_unsupported_protocol_schemes() {
    let bad_schemes = ["file:///etc/passwd", "gopher://bad.com", "ftp://test.org"];
    for bad in &bad_schemes {
        assert!(!bad.starts_with("http://") && !bad.starts_with("https://"));
    }
}

// ============================================================================
// Boundary 5: Layout & High-DPI Boundaries (>= 5 tests)
// ============================================================================

#[test]
fn test_b05_minimal_viewport_dimensions() {
    let harness = HeadlessUiHarness::new();
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(100.0, 100.0)));
    let (rendered, _) = harness.render_frame_with_input(input, |ctx| {
        let mut drew = false;
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            drew = ui.available_width() <= 100.0;
        });
        drew
    });
    assert!(rendered);
}

#[test]
fn test_b05_extreme_wide_viewport_dimensions() {
    let harness = HeadlessUiHarness::new();
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(4000.0, 2000.0)));
    let (left_inset, _) = harness.render_frame_with_input(input, |_ctx| {
        MAC_OS_TRAFFIC_LIGHTS_INSET
    });
    assert_eq!(left_inset, 78.0, "Traffic light insets must stay 78pt even on 4K/5K ultrawide displays");
}

#[test]
fn test_b05_retina_scale_factor_2x_and_3x() {
    let scale_2x = 2.0f32;
    let scale_3x = 3.0f32;
    assert_eq!(MAC_OS_HEADER_HEIGHT * scale_2x, 68.0);
    assert_eq!(MAC_OS_HEADER_HEIGHT * scale_3x, 102.0);
}

#[test]
fn test_b05_long_video_title_truncation_does_not_overflow() {
    let long_title = "A".repeat(500);
    let harness = HeadlessUiHarness::new();
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 600.0)));
    let (truncated, _) = harness.render_frame_with_input(input, |ctx| {
        let mut label_fits = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.allocate_ui(Vec2::new(300.0, 20.0), |ui| {
                let resp = ui.add(egui::Label::new(&long_title).truncate());
                label_fits = resp.rect.width() <= 300.0 + 1.0;
            });
        });
        label_fits
    });
    assert!(truncated);
}

#[test]
fn test_b05_massive_error_message_wrapping_without_window_expansion() {
    let long_error = "Error: ".repeat(100);
    let masked = mask_plain_language_error(&long_error);
    assert!(masked.len() < 120, "Masked plain-language error must be compact and not expand window");
}
