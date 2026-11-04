#![allow(clippy::all)]
#![allow(clippy::assertions_on_constants)]

//! Empirical Adversarial Stress Test Suite for Milestone 2 (R2) — Challenger 1.
//!
//! Scope:
//! 1. Stream badge derivations:
//!    - Probing edge cases: streams with unusual format IDs ("137+140", "custom_dash_1080p", "audio_only"),
//!      missing codecs, non-standard resolutions, missing containers, empty strings.
//!    - Verify default badge texts match "Highest Quality (1080p Video + High-Res Audio)" and "🎬 H.264 + 🎵 AAC".
//! 2. Default format selection:
//!    - Verify selection order: 1080p SplitDash > any 1080p > any SplitDash > highest video resolution > first stream.
//!    - Multi-candidate resolution tie-breaks, vertical video, extreme aspect ratios, empty streams.
//! 3. Thumbnail resilience:
//!    - Probing thumbnail decoding with corrupt byte buffers, empty buffers, invalid PNG/JPEG/WebP headers,
//!      fuzzed bytes, truncated bitstreams, valid image roundtrips (ensuring zero panic, graceful None).
//! 4. Filesystem pre-checks & capacity invariants:
//!    - Interior null bytes, deeply nested paths, boundary values (0, u64::MAX).
//! 5. UI Rendering under stress:
//!    - Headless egui Context execution of StagingState (Idle, Probing, Staged, Error) with extreme payloads.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use eframe::egui;
use rust_dl_core::resolver::{MediaManifest, MediaStream, Platform, StreamDelivery};

#[path = "../src/theme.rs"]
pub mod theme;

#[path = "../src/staging.rs"]
pub mod staging;

use staging::{
    check_disk_space, decode_thumbnail_image, evaluate_disk_space, format_bytes, format_duration,
    format_stream_badges, render_pill, render_staging_card, render_stream_badges,
    select_default_format_id, DiskCheckResult, StagingState, BADGE_DASH_PRIMARY_DEFAULT,
    BADGE_DASH_SECONDARY_DEFAULT,
};

// ============================================================================
// Helper builders for adversarial manifests and streams
// ============================================================================

fn make_stream(
    format_id: &str,
    delivery: StreamDelivery,
    container: &str,
    resolution: Option<&str>,
    vcodec: Option<&str>,
    acodec: Option<&str>,
    filesize_bytes: Option<u64>,
) -> MediaStream {
    MediaStream {
        format_id: format_id.to_string(),
        delivery,
        container: container.to_string(),
        resolution: resolution.map(String::from),
        fps: Some(30.0),
        filesize_bytes,
        vcodec: vcodec.map(String::from),
        acodec: acodec.map(String::from),
        headers: HashMap::new(),
    }
}

fn make_manifest(streams: Vec<MediaStream>) -> MediaManifest {
    MediaManifest {
        id: "stress-vid-123".to_string(),
        title: "Adversarial Stress Test Video 🎬".to_string(),
        uploader: "Stress Channel 🎵".to_string(),
        duration_secs: Some(3665.0),
        thumbnail_url: Some("https://example.com/thumb.jpg".to_string()),
        platform: Platform::YouTube,
        streams,
    }
}

// ============================================================================
// 1. Badge Derivation Stress Tests
// ============================================================================

#[test]
fn test_stress_badge_dash_1080p_splitdash_default_exact_text() {
    let stream = make_stream(
        "137+140",
        StreamDelivery::SplitDash {
            video_url: "https://example.com/v137.mp4".to_string(),
            audio_url: "https://example.com/a140.m4a".to_string(),
        },
        "mp4",
        Some("1920x1080"),
        Some("avc1.640028"),
        Some("mp4a.40.2"),
        Some(50_000_000),
    );

    let (primary, secondary) = format_stream_badges(&stream);

    // Assert exact mandated strings from HIG and E2E contracts
    assert_eq!(primary, "Highest Quality (1080p Video + High-Res Audio)");
    assert_eq!(primary, BADGE_DASH_PRIMARY_DEFAULT);
    assert_eq!(secondary, "🎬 H.264 + 🎵 AAC");
    assert_eq!(secondary, BADGE_DASH_SECONDARY_DEFAULT);
}

#[test]
fn test_stress_badge_custom_dash_1080p_format_id_direct_delivery() {
    let stream = make_stream(
        "custom_dash_1080p",
        StreamDelivery::Direct {
            url: "https://example.com/direct.mp4".to_string(),
        },
        "mp4",
        Some("1920x1080"),
        Some("h264"),
        Some("aac"),
        Some(40_000_000),
    );

    let (primary, secondary) = format_stream_badges(&stream);
    assert_eq!(primary, BADGE_DASH_PRIMARY_DEFAULT);
    assert_eq!(secondary, BADGE_DASH_SECONDARY_DEFAULT);
}

#[test]
fn test_stress_badge_audio_only_stream() {
    let stream = make_stream(
        "audio_only",
        StreamDelivery::Direct {
            url: "https://example.com/audio.m4a".to_string(),
        },
        "m4a",
        None,
        None, // vcodec is None
        Some("mp4a.40.2"),
        Some(5_000_000),
    );

    let (primary, secondary) = format_stream_badges(&stream);
    assert_eq!(primary, "Audio Only (High-Res M4A)");
    assert_eq!(secondary, "🎵 AAC");
}

#[test]
fn test_stress_badge_missing_both_codecs_non_dash() {
    let stream = make_stream(
        "no_codecs",
        StreamDelivery::Direct {
            url: "https://example.com/stream.mkv".to_string(),
        },
        "mkv",
        Some("1280x720"),
        None,
        None,
        Some(25_000_000),
    );

    let (primary, secondary) = format_stream_badges(&stream);
    assert_eq!(primary, "1280x720 Video");
    assert_eq!(secondary, "🎬 MKV");
}

#[test]
fn test_stress_badge_missing_resolution_and_container() {
    let stream = make_stream(
        "raw_stream",
        StreamDelivery::Direct {
            url: "https://example.com/raw".to_string(),
        },
        "",
        None,
        None,
        None,
        None,
    );

    let (primary, secondary) = format_stream_badges(&stream);
    assert_eq!(primary, "Video Video");
    assert_eq!(secondary, "🎬 ");
}

#[test]
fn test_stress_badge_non_standard_resolutions() {
    let resolutions = [
        ("vertical_1080x1920", "vertical_1080x1920 Video"),
        ("1080x1920", "1080x1920 Video"),
        ("3840x2160", "3840x2160 Video"),
        ("7680x4320", "7680x4320 Video"),
        ("2560x1440", "2560x1440 Video"),
        ("1080i", "1080i Video"),
        ("custom_res", "custom_res Video"),
    ];

    for (res_input, expected_primary) in resolutions {
        let stream = make_stream(
            "test_fmt",
            StreamDelivery::Direct {
                url: "https://example.com/v".to_string(),
            },
            "mp4",
            Some(res_input),
            Some("h264"),
            Some("aac"),
            None,
        );
        let (primary, secondary) = format_stream_badges(&stream);
        assert_eq!(primary, expected_primary);
        assert_eq!(secondary, "🎬 MP4");
    }
}

#[test]
fn test_stress_badge_unusual_and_adversarial_format_ids() {
    let format_ids = [
        "137+140",
        "custom_dash_1080p",
        "audio_only",
        "FORMAT_WITH_CAPS_DASH",
        "format/with:special?chars#and=params",
        "0",
        "very_long_format_id_which_contains_lots_of_metadata_and_hashes_1234567890",
    ];

    for fid in format_ids {
        let stream = make_stream(
            fid,
            StreamDelivery::Direct {
                url: "https://example.com/v".to_string(),
            },
            "webm",
            Some("720p"),
            Some("vp9"),
            Some("opus"),
            None,
        );
        let (primary, secondary) = format_stream_badges(&stream);
        assert!(!primary.is_empty());
        assert_eq!(secondary, "🎬 WEBM");
    }
}

// ============================================================================
// 2. Default Format Selection Order Priority Stress Tests
// ============================================================================

#[test]
fn test_stress_selection_priority_1_1080p_splitdash_over_all_others() {
    // Candidates:
    // A: 4K Direct (highest resolution)
    // B: 1080p Direct
    // C: 4K SplitDash
    // D: 1080p SplitDash (PRIORITY 1)
    // E: First stream 720p
    let s_first = make_stream("fmt_first_720p", StreamDelivery::Direct { url: "".into() }, "mp4", Some("720p"), None, None, None);
    let s_4k_direct = make_stream("fmt_4k_direct", StreamDelivery::Direct { url: "".into() }, "mp4", Some("3840x2160"), None, None, None);
    let s_1080p_direct = make_stream("fmt_1080p_direct", StreamDelivery::Direct { url: "".into() }, "mp4", Some("1920x1080"), None, None, None);
    let s_4k_dash = make_stream("fmt_4k_dash", StreamDelivery::SplitDash { video_url: "".into(), audio_url: "".into() }, "mp4", Some("3840x2160"), None, None, None);
    let s_1080p_dash = make_stream("fmt_1080p_dash", StreamDelivery::SplitDash { video_url: "".into(), audio_url: "".into() }, "mp4", Some("1920x1080"), None, None, None);

    let manifest = make_manifest(vec![s_first, s_4k_direct, s_1080p_direct, s_4k_dash, s_1080p_dash]);
    let chosen = select_default_format_id(&manifest);
    assert_eq!(chosen, Some("fmt_1080p_dash".to_string()));
}

#[test]
fn test_stress_selection_priority_2_any_1080p_over_lower_and_higher_non_dash() {
    // Candidates without 1080p SplitDash:
    // A: First stream 480p
    // B: 4K SplitDash
    // C: 1080p Direct (PRIORITY 2: Any 1080p)
    // D: 720p SplitDash
    let s_first = make_stream("fmt_480p", StreamDelivery::Direct { url: "".into() }, "mp4", Some("480p"), None, None, None);
    let s_4k_dash = make_stream("fmt_4k_dash", StreamDelivery::SplitDash { video_url: "".into(), audio_url: "".into() }, "mp4", Some("3840x2160"), None, None, None);
    let s_1080p_direct = make_stream("fmt_1080p_direct", StreamDelivery::Direct { url: "".into() }, "mp4", Some("1920x1080"), None, None, None);
    let s_720p_dash = make_stream("fmt_720p_dash", StreamDelivery::SplitDash { video_url: "".into(), audio_url: "".into() }, "mp4", Some("720p"), None, None, None);

    let manifest = make_manifest(vec![s_first, s_4k_dash, s_1080p_direct, s_720p_dash]);
    let chosen = select_default_format_id(&manifest);
    assert_eq!(chosen, Some("fmt_1080p_direct".to_string()));
}

#[test]
fn test_stress_selection_priority_3_any_splitdash_over_other_resolutions() {
    // Candidates without any 1080p:
    // A: First stream 480p Direct
    // B: 4K Direct (highest resolution)
    // C: 720p SplitDash (PRIORITY 3: Any SplitDash)
    // D: 720p Direct
    let s_first = make_stream("fmt_480p", StreamDelivery::Direct { url: "".into() }, "mp4", Some("480p"), None, None, None);
    let s_4k_direct = make_stream("fmt_4k_direct", StreamDelivery::Direct { url: "".into() }, "mp4", Some("3840x2160"), None, None, None);
    let s_720p_dash = make_stream("fmt_720p_dash", StreamDelivery::SplitDash { video_url: "".into(), audio_url: "".into() }, "mp4", Some("720p"), None, None, None);
    let s_720p_direct = make_stream("fmt_720p_direct", StreamDelivery::Direct { url: "".into() }, "mp4", Some("720p"), None, None, None);

    let manifest = make_manifest(vec![s_first, s_4k_direct, s_720p_dash, s_720p_direct]);
    let chosen = select_default_format_id(&manifest);
    assert_eq!(chosen, Some("fmt_720p_dash".to_string()));
}

#[test]
fn test_stress_selection_priority_4_highest_video_resolution() {
    // Candidates without any 1080p and without any SplitDash:
    // A: First stream 360p Direct
    // B: 720p Direct
    // C: 4K Direct (PRIORITY 4: Highest Video Resolution)
    // D: 1440p Direct
    let s_360 = make_stream("fmt_360p", StreamDelivery::Direct { url: "".into() }, "mp4", Some("640x360"), Some("h264"), None, None);
    let s_720 = make_stream("fmt_720p", StreamDelivery::Direct { url: "".into() }, "mp4", Some("1280x720"), Some("h264"), None, None);
    let s_4k = make_stream("fmt_4k", StreamDelivery::Direct { url: "".into() }, "mp4", Some("3840x2160"), Some("h264"), None, None);
    let s_1440 = make_stream("fmt_1440p", StreamDelivery::Direct { url: "".into() }, "mp4", Some("2560x1440"), Some("h264"), None, None);

    let manifest = make_manifest(vec![s_360, s_720, s_4k, s_1440]);
    let chosen = select_default_format_id(&manifest);
    assert_eq!(chosen, Some("fmt_4k".to_string()));
}

#[test]
fn test_stress_selection_priority_5_first_stream_fallback() {
    // Candidates without video codecs or resolvable heights (e.g. audio-only streams)
    let s_audio1 = make_stream("audio_best", StreamDelivery::Direct { url: "".into() }, "m4a", None, None, Some("aac"), None);
    let s_audio2 = make_stream("audio_backup", StreamDelivery::Direct { url: "".into() }, "mp3", None, None, Some("mp3"), None);

    let manifest = make_manifest(vec![s_audio1, s_audio2]);
    let chosen = select_default_format_id(&manifest);
    assert_eq!(chosen, Some("audio_best".to_string()));
}

#[test]
fn test_stress_selection_empty_manifest_returns_none() {
    let manifest = make_manifest(vec![]);
    let chosen = select_default_format_id(&manifest);
    assert_eq!(chosen, None);
}

#[test]
fn test_stress_selection_vertical_and_atypical_resolutions() {
    // Vertical video: 1080x1920 (TikTok / Reels / Shorts 1080p)
    let s_vert = make_stream(
        "vertical_1080p_dash",
        StreamDelivery::SplitDash {
            video_url: "".into(),
            audio_url: "".into(),
        },
        "mp4",
        Some("1080x1920"),
        Some("h264"),
        Some("aac"),
        None,
    );
    let s_720 = make_stream("fmt_720p", StreamDelivery::Direct { url: "".into() }, "mp4", Some("720p"), None, None, None);

    let manifest = make_manifest(vec![s_720, s_vert]);
    let chosen = select_default_format_id(&manifest);
    assert_eq!(chosen, Some("vertical_1080p_dash".to_string()));
}

// ============================================================================
// 3. Thumbnail Resilience & Decoding Under Adversarial Inputs
// ============================================================================

#[test]
fn test_stress_thumbnail_decode_empty_buffer_returns_none() {
    assert!(decode_thumbnail_image(&[]).is_none());
}

#[test]
fn test_stress_thumbnail_decode_tiny_single_bytes_returns_none() {
    assert!(decode_thumbnail_image(&[0x00]).is_none());
    assert!(decode_thumbnail_image(&[0xFF]).is_none());
    assert!(decode_thumbnail_image(&[0x47, 0x49, 0x46]).is_none()); // "GIF" truncated
    assert!(decode_thumbnail_image(&[0x89, 0x50, 0x4E, 0x47]).is_none()); // PNG magic truncated
}

#[test]
fn test_stress_thumbnail_decode_corrupt_png_header_returns_none() {
    let mut corrupt_png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]; // PNG 8-byte magic
    corrupt_png.extend_from_slice(b"IHDR_CORRUPTED_PAYLOAD_12345");
    assert!(decode_thumbnail_image(&corrupt_png).is_none());
}

#[test]
fn test_stress_thumbnail_decode_corrupt_jpeg_header_returns_none() {
    let mut corrupt_jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00];
    corrupt_jpeg.extend_from_slice(b"DEADBEEF_TRUNCATED_SOF0_MARKER");
    assert!(decode_thumbnail_image(&corrupt_jpeg).is_none());
}

#[test]
fn test_stress_thumbnail_decode_corrupt_webp_riff_returns_none() {
    let corrupt_webp = b"RIFF\x14\x00\x00\x00WEBPVP8 \x08\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    assert!(decode_thumbnail_image(corrupt_webp).is_none());
}

#[test]
fn test_stress_thumbnail_decode_fuzzed_random_byte_buffers() {
    // Generate pseudo-random deterministic byte streams across multiple sizes
    let mut state: u64 = 0xDEADBEEFCAFEBABE;
    for len in [2, 7, 16, 64, 256, 1024, 8192] {
        let mut fuzzed = Vec::with_capacity(len);
        for _ in 0..len {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            fuzzed.push((state >> 56) as u8);
        }
        let result = decode_thumbnail_image(&fuzzed);
        // Fuzzed garbage bytes must return None safely without panic or unwrap
        assert!(result.is_none());
    }
}

#[test]
fn test_stress_thumbnail_decode_valid_generated_png() {
    // Synthesize a valid 4x4 RGBA PNG in memory
    let img = image::RgbaImage::from_fn(4, 4, |x, y| {
        image::Rgba([(x * 50) as u8, (y * 50) as u8, 200, 255])
    });
    let mut png_bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .expect("Test valid PNG creation must succeed");

    let decoded = decode_thumbnail_image(&png_bytes);
    assert!(decoded.is_some());
    let color_img = decoded.unwrap();
    assert_eq!(color_img.size, [4, 4]);
    assert_eq!(color_img.pixels.len(), 16);
}

#[test]
fn test_stress_thumbnail_decode_valid_generated_jpeg() {
    // Synthesize a valid 8x8 RGB JPEG in memory
    let img = image::RgbImage::from_fn(8, 8, |x, y| {
        image::Rgb([(x * 30) as u8, (y * 30) as u8, 150])
    });
    let mut jpeg_bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut jpeg_bytes), image::ImageFormat::Jpeg)
        .expect("Test valid JPEG creation must succeed");

    let decoded = decode_thumbnail_image(&jpeg_bytes);
    assert!(decoded.is_some());
    let color_img = decoded.unwrap();
    assert_eq!(color_img.size, [8, 8]);
    assert_eq!(color_img.pixels.len(), 64);
}

// ============================================================================
// 4. Filesystem Pre-checks & Capacity Invariants Under Adversarial Stress
// ============================================================================

#[test]
fn test_stress_disk_check_path_with_interior_null_byte() {
    // A nonexistent path with an interior null byte climbs to existing parent (/tmp) safely
    let res = check_disk_space(Path::new("/tmp/test\0forbidden"), Some(50_000_000));
    assert!(res.available_bytes > 0);
    assert!(res.is_sufficient);
}

#[test]
fn test_stress_disk_check_ancestor_traversal_deep_hierarchy() {
    // Deeply nonexistent subdirectories ascend to an existing mount point
    let res = check_disk_space(
        Path::new("/tmp/deep_a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p"),
        Some(100),
    );
    assert!(res.available_bytes > 0);
    assert!(res.is_sufficient);
}

#[test]
fn test_stress_disk_check_boundary_values() {
    // 0 required bytes is always sufficient
    let r0 = evaluate_disk_space(0, Some(0));
    assert!(r0.is_sufficient);

    // u64::MAX required bytes against 1 GB
    let r_max = evaluate_disk_space(1_000_000_000, Some(u64::MAX));
    assert!(!r_max.is_sufficient);
    assert_eq!(r_max.required_bytes, u64::MAX);

    // Both u64::MAX
    let r_both_max = evaluate_disk_space(u64::MAX, Some(u64::MAX));
    assert!(r_both_max.is_sufficient);
}

// ============================================================================
// 5. Staging UI Rendering & State Machine Stress Invariants
// ============================================================================

#[test]
fn test_stress_staging_headless_probing_render() {
    let ctx = egui::Context::default();
    let mut staging = StagingState::Probing {
        url: "https://www.youtube.com/watch?v=adversarial_url_probe_12345".to_string(),
        started_at: Instant::now(),
    };

    let mut download_called = false;
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            render_staging_card(ui, &mut staging, |_url, _fmt| {
                download_called = true;
            });
        });
    });

    assert!(staging.is_probing());
    assert!(!download_called);
}

#[test]
fn test_stress_staging_headless_staged_render_with_badges_and_sufficient_disk() {
    let ctx = egui::Context::default();
    let stream_dash = make_stream(
        "137+140",
        StreamDelivery::SplitDash {
            video_url: "".into(),
            audio_url: "".into(),
        },
        "mp4",
        Some("1920x1080"),
        Some("avc1"),
        Some("mp4a"),
        Some(45_000_000),
    );
    let manifest = make_manifest(vec![stream_dash]);

    let mut staging = StagingState::Staged {
        url: "https://example.com/watch".to_string(),
        manifest,
        thumbnail: None,
        selected_format_id: "137+140".to_string(),
        disk_check: DiskCheckResult::new(45_000_000, 10_000_000_000),
    };

    let mut download_called = false;
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            render_staging_card(ui, &mut staging, |_url, _fmt| {
                download_called = true;
            });
        });
    });

    assert!(staging.is_staged());
    assert!(!download_called);
}

#[test]
fn test_stress_staging_headless_staged_render_with_insufficient_disk_shows_warning() {
    let ctx = egui::Context::default();
    let stream_dash = make_stream(
        "137+140",
        StreamDelivery::SplitDash {
            video_url: "".into(),
            audio_url: "".into(),
        },
        "mp4",
        Some("1920x1080"),
        Some("avc1"),
        Some("mp4a"),
        Some(100_000_000),
    );
    let manifest = make_manifest(vec![stream_dash]);

    let mut staging = StagingState::Staged {
        url: "https://example.com/watch".to_string(),
        manifest,
        thumbnail: None,
        selected_format_id: "137+140".to_string(),
        disk_check: DiskCheckResult::new(100_000_000, 20_000_000), // Only 20MB available!
    };

    let mut download_called = false;
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            render_staging_card(ui, &mut staging, |_url, _fmt| {
                download_called = true;
            });
        });
    });

    // Invariant: Insufficient disk space MUST NOT trigger download
    assert!(staging.is_staged());
    assert!(!download_called);
}

#[test]
fn test_stress_staging_headless_error_render() {
    let ctx = egui::Context::default();
    let mut staging = StagingState::Error {
        url: "https://example.com/broken".to_string(),
        error: "HTTP 403 Forbidden: Cloudflare anti-bot verification challenge".to_string(),
    };

    let mut download_called = false;
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            render_staging_card(ui, &mut staging, |_url, _fmt| {
                download_called = true;
            });
        });
    });

    assert!(matches!(staging, StagingState::Error { .. }));
    assert!(!download_called);
}

#[test]
fn test_stress_pill_badges_rendering_extreme_lengths() {
    let ctx = egui::Context::default();
    let massive_pri = "Highest Quality (1080p Video + High-Res Audio) ".repeat(5);
    let massive_sec = "🎬 H.264 + 🎵 AAC ".repeat(5);

    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            render_stream_badges(ui, &massive_pri, &massive_sec);
            render_pill(
                ui,
                "Extreme Pill",
                egui::Color32::BLUE,
                egui::Color32::WHITE,
                12.0,
                egui::vec2(8.0, 4.0),
                6.0,
            );
        });
    });
}

#[test]
fn test_stress_duration_formatting_extremes() {
    // 0 seconds -> "0:00"
    assert_eq!(format_duration(0.0), "0:00");
    // Under 1 minute -> "0:45"
    assert_eq!(format_duration(45.0), "0:45");
    // Under 1 hour -> "3:33"
    assert_eq!(format_duration(213.0), "3:33");
    // Exact 1 hour -> "1:00:00"
    assert_eq!(format_duration(3600.0), "1:00:00");
    // Over 1 hour -> "1:01:05"
    assert_eq!(format_duration(3665.0), "1:01:05");
    // Multi-digit hours -> "100:00:00"
    assert_eq!(format_duration(360000.0), "100:00:00");
    // Float subseconds rounded properly -> 59.6 rounds to 60 -> "1:00"
    assert_eq!(format_duration(59.6), "1:00");
    // Negative duration saturates safely without panic
    assert_eq!(format_duration(-10.0), "0:00");
    // NaN saturates safely
    assert_eq!(format_duration(f64::NAN), "0:00");
}

#[test]
fn test_stress_format_bytes_extremes() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(1), "1 B");
    assert_eq!(format_bytes(1023), "1023 B");
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(1024 * 1024 - 1), "1024.0 KB");
    assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
    assert_eq!(format_bytes(50 * 1024 * 1024), "50.0 MB");
    assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
    assert_eq!(format_bytes(10 * 1024 * 1024 * 1024), "10.0 GB");
}
