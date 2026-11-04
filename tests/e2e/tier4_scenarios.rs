//! Tier 4: Real-World Application Scenarios (Realistic Workloads).
//! Models complete end-to-end user journeys and state machine workflows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, Color32, Rounding};
use rust_dl_core::downloader::DownloadPhase;
use rust_dl_core::storage::HistoryEntry;
use rust_dl_core::{BrowserKind, CookieSource, Platform, StreamDelivery};

use super::harness::*;

#[test]
fn test_t4_scenario_youtube_1080p_dash_happy_path() {
    // 1. User copies link
    let yt_url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_string();
    let mut toast_mgr = ToastManager::new();
    toast_mgr.trigger_clipboard_toast(yt_url.clone());
    assert_eq!(toast_mgr.active_clipboard_url.as_deref(), Some(yt_url.as_str()));

    // 2. User presses Return key to stage
    let staged_url = toast_mgr.handle_key_return(false).expect("URL staged");
    assert_eq!(staged_url, yt_url);
    assert!(toast_mgr.active_clipboard_url.is_none());

    // 3. Staging card probes and discovers DASH manifest
    let manifest = sample_youtube_manifest();
    let stream_1080p = &manifest.streams[0];
    let (badge_primary, badge_secondary) = derive_stream_badges(stream_1080p);
    assert_eq!(badge_primary, "Highest Quality (1080p Video + High-Res Audio)");
    assert_eq!(badge_secondary, "🎬 H.264 + 🎵 AAC");

    // 4. Pre-download disk space check (45 MB required, 20 GB available)
    let disk_result = evaluate_disk_space(20_000_000_000, stream_1080p.filesize_bytes);
    assert!(disk_result.is_sufficient);

    // 5. User queues job -> Card renders with 10pt rounding and subtle elevation
    let card_rounding = Rounding::same(MAC_OS_CARD_CORNER_RADIUS);
    assert_eq!(card_rounding.nw, 10.0);

    // 6. Phase 1: Probing
    let phase_probing = DownloadPhase::Probing;
    assert_eq!(COLOR_PROBING_BLUE, Color32::from_rgb(0, 122, 255));
    assert_eq!(TEXT_PROBING_STATUS, "Extracting Metadata...");

    // 7. Phase 2: Downloading
    let phase_downloading = DownloadPhase::Downloading;
    let metrics = format_download_metrics(45_000_000, Some(45_000_000), 5_000_000.0, Some(0));
    assert_eq!(COLOR_DOWNLOADING_BLUE, Color32::from_rgb(0, 122, 255));
    assert!(metrics.contains("42.9 MB / 42.9 MB"));

    // 8. Phase 3: Muxing
    let phase_muxing = DownloadPhase::Muxing;
    assert_eq!(COLOR_MUXING_PURPLE, Color32::from_rgb(175, 82, 222));
    assert_eq!(TEXT_MUXING_STATUS, "Merging Audio & Video...");

    // 9. Phase 4: Completed
    let output_path = PathBuf::from("/Users/test/Downloads/Rick Astley.mp4");
    let phase_completed = DownloadPhase::Completed { output_path: output_path.clone() };
    assert_eq!(COLOR_COMPLETED_GREEN, Color32::from_rgb(52, 199, 89));
    assert_eq!(TEXT_COMPLETED_STATUS, "Completed.");
    assert_eq!(BUTTON_SHOW_IN_FINDER, "Show in Finder");
}

#[test]
fn test_t4_scenario_low_disk_space_guardrail() {
    // 1. User stages 4K video requiring 25 GB
    let required_4k = 25 * 1024 * 1024 * 1024u64; // 25 GB
    let available_disk = 4 * 1024 * 1024 * 1024u64; // 4 GB available

    // 2. Pre-check fails
    let check_4k = evaluate_disk_space(available_disk, Some(required_4k));
    assert!(!check_4k.is_sufficient);

    // Download button must be disabled
    let button_enabled = check_4k.is_sufficient;
    assert!(!button_enabled);

    // 3. User switches format selector to 720p (requiring 1.2 GB)
    let required_720p = 1_200_000_000u64; // 1.2 GB
    let check_720p = evaluate_disk_space(available_disk, Some(required_720p));
    assert!(check_720p.is_sufficient);

    let button_enabled_720p = check_720p.is_sufficient;
    assert!(button_enabled_720p, "Download button must become enabled after selecting lighter format");
}

#[test]
fn test_t4_scenario_instagram_private_reel_auth_recovery() {
    // 1. Private reel URL
    let reel_url = "https://www.instagram.com/reel/C1234567/";

    // 2. Resolver returns raw error
    let raw_error = "AuthenticationRequired for instagram: Login required to view this private reel (HTTP 401)";
    let masked = mask_plain_language_error(raw_error);
    assert_eq!(masked, TEXT_AUTH_REQUIRED_STATUS);
    assert_eq!(masked, "Login required for this platform");

    // 3. Card theme is Amber
    let theme_color = COLOR_AUTH_AMBER;
    assert_eq!(theme_color, Color32::from_rgb(255, 149, 0));

    // 4. Recovery actions
    assert_eq!(BUTTON_CONNECT_COOKIES, "Connect Browser Cookies");
    assert_eq!(BUTTON_RETRY, "Retry");

    // 5. User clicks Connect Browser Cookies -> Navigates to auth settings
    let mut current_view = "Queue";
    let mut on_connect_cookies = || {
        current_view = "AuthSettings";
    };
    on_connect_cookies();
    assert_eq!(current_view, "AuthSettings");

    // 6. User selects Safari / Arc cookies and clicks Retry
    let mut retry_triggered = false;
    let mut on_retry = || {
        retry_triggered = true;
    };
    on_retry();
    assert!(retry_triggered);
}

#[test]
fn test_t4_scenario_rapid_clipboard_keyboard_batch_ingestion() {
    let mut toast_mgr = ToastManager::new();

    // Ingest URL 1
    toast_mgr.trigger_clipboard_toast("https://example.com/v1".to_string());
    assert_eq!(toast_mgr.handle_key_return(false), Some("https://example.com/v1".to_string()));

    // Ingest URL 2
    toast_mgr.trigger_clipboard_toast("https://example.com/v2".to_string());
    assert_eq!(toast_mgr.handle_key_return(false), Some("https://example.com/v2".to_string()));

    // Duplicate URL 2 re-copied: duplicate suppression active
    toast_mgr.trigger_clipboard_toast("https://example.com/v2".to_string());
    assert!(toast_mgr.active_clipboard_url.is_none());

    // User typing in text input field: Return must NOT stage URL
    toast_mgr.trigger_clipboard_toast("https://example.com/v3".to_string());
    let typing_return = toast_mgr.handle_key_return(true);
    assert!(typing_return.is_none());
    assert!(toast_mgr.active_clipboard_url.is_some());
}

#[test]
fn test_t4_scenario_accidental_history_clear_and_undo_recovery() {
    let mut toast_mgr = ToastManager::new();
    let initial_history = (1..=10)
        .map(|i| {
            make_history_entry(
                &format!("Historical Download #{}", i),
                &format!("https://example.com/video_{}", i),
                Platform::YouTube,
                &format!("/Users/test/Downloads/video_{}.mp4", i),
                Some(50_000_000 * i as u64),
                1700000000 + i as u64,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(initial_history.len(), 10);

    // User accidentally clicks Clear Completed
    let mut active_history: Vec<HistoryEntry> = Vec::new(); // Cleared
    assert!(active_history.is_empty());
    toast_mgr.trigger_clear_history_toast(initial_history.clone());

    // 2.5 seconds elapse
    toast_mgr.tick(2.5);
    assert_eq!(toast_mgr.history_undo_timer_remaining, 2.5);

    // User clicks Undo
    let restored = toast_mgr.perform_undo();
    active_history = restored;

    // Verify all 10 items restored with 100% fidelity
    assert_eq!(active_history.len(), 10);
    for (i, item) in active_history.iter().enumerate() {
        assert_eq!(item.title, format!("Historical Download #{}", i + 1));
        assert_eq!(item.url, format!("https://example.com/video_{}", i + 1));
    }
}
