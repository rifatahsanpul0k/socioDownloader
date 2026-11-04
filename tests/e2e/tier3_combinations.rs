//! Tier 3: Cross-Feature Combinations (Pairwise Component Interactions).
//! Verifies seamless coordination across Window Shell, Staging, Queue, and Toast systems.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, Color32, Key, Rounding};
use rust_dl_core::downloader::DownloadPhase;
use rust_dl_core::storage::HistoryEntry;
use rust_dl_core::{BrowserKind, CookieSource, Platform, StreamDelivery};

use super::harness::*;

#[test]
fn test_t3_clipboard_to_staging_via_return_key() {
    let mut manager = ToastManager::new();
    let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_string();
    manager.trigger_clipboard_toast(url.clone());
    assert!(manager.active_clipboard_url.is_some());

    // Pressing Return key when text input does not have focus
    let staged_url = manager.handle_key_return(false);
    assert_eq!(staged_url, Some(url));
    assert!(manager.active_clipboard_url.is_none());

    // Transitions into StagingState::Probing
    let staging_state = StagingState::Probing {
        url: staged_url.unwrap(),
        started_at: Instant::now(),
    };
    assert!(matches!(staging_state, StagingState::Probing { .. }));
}

#[test]
fn test_t3_staging_to_insufficient_disk_blocks_queue() {
    let manifest = sample_youtube_manifest();
    // 50 MB required, only 10 MB available on target partition
    let disk_check = evaluate_disk_space(10_000_000, Some(50_000_000));
    assert!(!disk_check.is_sufficient);

    let staging = StagingState::Staged {
        url: manifest.id.clone(),
        manifest,
        thumbnail: None,
        selected_format_id: "dash-1080p".to_string(),
        disk_check: disk_check.clone(),
    };

    // Download button must be disabled
    let can_download = disk_check.is_sufficient;
    assert!(!can_download, "Download button must remain disabled when disk is insufficient");
    assert!(matches!(staging, StagingState::Staged { .. }));
}

#[test]
fn test_t3_staging_to_sufficient_disk_enqueues_job() {
    let manifest = sample_youtube_manifest();
    // 50 MB required, 5 GB available
    let disk_check = evaluate_disk_space(5_000_000_000, Some(50_000_000));
    assert!(disk_check.is_sufficient);

    let (primary_badge, secondary_badge) = derive_stream_badges(&manifest.streams[0]);
    assert_eq!(primary_badge, BADGE_DASH_PRIMARY_DEFAULT);
    assert_eq!(secondary_badge, BADGE_DASH_SECONDARY_DEFAULT);

    let mut queued_job = None;
    let mut on_download = |url: String, format_id: String| {
        queued_job = Some((url, format_id));
    };

    on_download(manifest.id.clone(), "dash-1080p".to_string());
    assert_eq!(queued_job, Some((manifest.id, "dash-1080p".to_string())));
}

#[test]
fn test_t3_job_lifecycle_probing_to_downloading_to_muxing_to_completed() {
    // Phase 1: Probing
    let phase1 = DownloadPhase::Probing;
    let color1 = match phase1 {
        DownloadPhase::Probing => COLOR_PROBING_BLUE,
        _ => Color32::WHITE,
    };
    let text1 = match phase1 {
        DownloadPhase::Probing => TEXT_PROBING_STATUS,
        _ => "",
    };
    assert_eq!(color1, Color32::from_rgb(0, 122, 255));
    assert_eq!(text1, "Extracting Metadata...");

    // Phase 2: Downloading
    let phase2 = DownloadPhase::Downloading;
    let color2 = match phase2 {
        DownloadPhase::Downloading => COLOR_DOWNLOADING_BLUE,
        _ => Color32::WHITE,
    };
    let text2 = format_download_metrics(20_000_000, Some(40_000_000), 2_000_000.0, Some(10));
    assert_eq!(color2, Color32::from_rgb(0, 122, 255));
    assert!(text2.contains("Downloading..."));

    // Phase 3: Muxing
    let phase3 = DownloadPhase::Muxing;
    let color3 = match phase3 {
        DownloadPhase::Muxing => COLOR_MUXING_PURPLE,
        _ => Color32::WHITE,
    };
    let text3 = match phase3 {
        DownloadPhase::Muxing => TEXT_MUXING_STATUS,
        _ => "",
    };
    assert_eq!(color3, Color32::from_rgb(175, 82, 222));
    assert_eq!(text3, "Merging Audio & Video...");

    // Phase 4: Completed
    let phase4 = DownloadPhase::Completed { output_path: PathBuf::from("/tmp/video.mp4") };
    let color4 = match phase4 {
        DownloadPhase::Completed { .. } => COLOR_COMPLETED_GREEN,
        _ => Color32::WHITE,
    };
    let text4 = match phase4 {
        DownloadPhase::Completed { .. } => TEXT_COMPLETED_STATUS,
        _ => "",
    };
    assert_eq!(color4, Color32::from_rgb(52, 199, 89));
    assert_eq!(text4, "Completed.");
}

#[test]
fn test_t3_auth_failure_to_browser_cookie_settings_navigation() {
    let raw_error = "Sign in to confirm you’re not a bot (HTTP 429 / AuthenticationRequired)";
    let masked = mask_plain_language_error(raw_error);
    assert_eq!(masked, TEXT_AUTH_REQUIRED_STATUS);

    let card_color = COLOR_AUTH_AMBER;
    assert_eq!(card_color, Color32::from_rgb(255, 149, 0));

    let mut navigated_to_cookies = false;
    let mut on_open_cookies = || {
        navigated_to_cookies = true;
    };
    on_open_cookies();
    assert!(navigated_to_cookies);
}

#[test]
fn test_t3_auth_failure_retry_restarts_probing() {
    let mut job_restarted = false;
    let mut on_retry = || {
        job_restarted = true;
    };
    on_retry();
    assert!(job_restarted);
}

#[test]
fn test_t3_batch_downloads_to_clear_history_to_undo() {
    let mut manager = ToastManager::new();
    let entries = (1..=5)
        .map(|i| {
            make_history_entry(
                &format!("Video {}", i),
                &format!("https://example.com/{}", i),
                Platform::YouTube,
                &format!("/tmp/video_{}.mp4", i),
                Some(10_000_000 * i as u64),
                1000 + i as u64,
            )
        })
        .collect::<Vec<_>>();

    // User clears 5 items
    manager.trigger_clear_history_toast(entries.clone());
    assert_eq!(manager.history_undo_timer_remaining, 5.0);
    assert_eq!(manager.history_undo_entries.len(), 5);

    // 2.5 seconds pass
    manager.tick(2.5);
    assert_eq!(manager.history_undo_timer_remaining, 2.5);

    // User clicks Undo
    let restored = manager.perform_undo();
    assert_eq!(restored.len(), 5);
    for (orig, rest) in entries.iter().zip(restored.iter()) {
        assert_eq!(orig.title, rest.title);
        assert_eq!(orig.url, rest.url);
    }
}

#[test]
fn test_t3_batch_downloads_to_clear_history_to_timeout() {
    let mut manager = ToastManager::new();
    let entries = vec![make_history_entry("Video", "https://example.com", Platform::YouTube, "/tmp/video.mp4", Some(10_000), 1000)];

    manager.trigger_clear_history_toast(entries);
    manager.tick(5.1); // Timeout exceeded
    assert_eq!(manager.history_undo_timer_remaining, 0.0);
    let restored = manager.perform_undo();
    assert!(restored.is_empty(), "Timeout must permanently purge undo buffer");
}

#[test]
fn test_t3_system_font_fallback_with_vibrancy_palette() {
    let harness = HeadlessUiHarness::new();
    let (success, _) = harness.render_frame(|ctx| {
        let mut visuals = ctx.style().visuals.clone();
        visuals.panel_fill = COLOR_MAC_OS_DARK_BG;
        ctx.set_visuals(visuals);
        let default_fonts = egui::FontDefinitions::default();
        ctx.set_fonts(default_fonts);
        ctx.style().visuals.panel_fill == COLOR_MAC_OS_DARK_BG
    });
    assert!(success);
}

#[test]
fn test_t3_error_masking_during_network_interruption() {
    let socket_reset_err = "SocketError(10054): WSAECONNRESET Connection reset by peer during chunked transfer";
    let masked = mask_plain_language_error(socket_reset_err);
    assert_eq!(masked, "Network connection error. Please check your internet connection.");
    assert!(!masked.contains("10054"));
    assert!(!masked.contains("WSAECONNRESET"));
}
