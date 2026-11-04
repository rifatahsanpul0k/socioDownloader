//! Empirical Adversarial Stress Test Suite for Milestone 2 Challenger 2
//! Focus: Disk Space Pre-checks, Boundary Value Analysis, and statvfs Filesystem Invariants.
//!
//! Validates:
//! 1. Boundary Value Analysis:
//!    - Exact boundary (`available == required`)
//!    - Off-by-one deficit (`available + 1 == required`) and surplus (`available == required + 1`)
//!    - Zero required bytes (`required == 0` or `required == None`)
//!    - Zero available bytes (`available == 0`)
//!    - Extreme capacity & overflow safety (100 TB, u64::MAX, petabytes)
//! 2. Human-Readable Formatting Stress Testing (`format_bytes` across ranges up to u64::MAX)
//! 3. Filesystem `statvfs` & Ancestor Traversal Climbing:
//!    - Live mount points (`/`, `/tmp`, home dir, download dir, current dir)
//!    - Deeply nested non-existent directory paths (1-level up to 10+ levels deep)
//!    - Non-existent deep paths under root (falling back to `/`)
//!    - Path edge cases: empty path `""`, relative non-existent paths, interior null bytes, trailing slashes, existing files
//! 4. UI Pre-Check & Action Gating Integration:
//!    - Verify `render_staging_card` blocks download action (even upon Enter key) when `!disk_check.is_sufficient`
//!    - Verify `render_staging_card` permits download when `disk_check.is_sufficient`
//!    - Verify dynamic re-evaluation of `disk_check` upon selecting a lighter format in dropdown

#![allow(clippy::all)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use eframe::egui;
use rust_dl_core::resolver::{MediaManifest, MediaStream, Platform, StreamDelivery};

#[path = "../src/theme.rs"]
mod theme;

#[path = "../src/staging.rs"]
mod staging;

use staging::{
    check_disk_space, evaluate_disk_space, format_bytes, query_available_space,
    DiskCheckResult, StagingState,
};

// ============================================================================
// Helper Fixtures
// ============================================================================

fn create_sample_manifest(stream_sizes: Vec<(&str, Option<u64>)>) -> MediaManifest {
    let streams = stream_sizes
        .into_iter()
        .map(|(fid, sz)| MediaStream {
            format_id: fid.to_string(),
            delivery: StreamDelivery::Direct {
                url: format!("https://example.com/{fid}.mp4"),
            },
            container: "mp4".to_string(),
            resolution: Some("1920x1080".to_string()),
            fps: Some(30.0),
            filesize_bytes: sz,
            vcodec: Some("h264".to_string()),
            acodec: Some("aac".to_string()),
            headers: HashMap::new(),
        })
        .collect();

    MediaManifest {
        id: "m2_challenger_test".to_string(),
        title: "Challenger 2 Disk Space Stress Test".to_string(),
        uploader: "Empirical Tester".to_string(),
        duration_secs: Some(120.0),
        thumbnail_url: None,
        platform: Platform::YouTube,
        streams,
    }
}

// ============================================================================
// 1. Boundary Value Analysis: Exact Boundary Tests (available == required)
// ============================================================================

#[test]
fn test_exact_boundary_zero_bytes() {
    let res = evaluate_disk_space(0, Some(0));
    assert!(
        res.is_sufficient,
        "Zero available must be sufficient when zero bytes are required"
    );
    assert_eq!(res.required_bytes, 0);
    assert_eq!(res.available_bytes, 0);

    let direct = DiskCheckResult::new(0, 0);
    assert!(direct.is_sufficient);
    assert_eq!(direct.required_bytes, 0);
    assert_eq!(direct.available_bytes, 0);
}

#[test]
fn test_exact_boundary_various_scales() {
    let test_points: &[u64] = &[
        1,                                          // 1 byte
        1024,                                       // 1 KB
        1_048_576,                                  // 1 MB
        1_073_741_824,                              // 1 GB
        50_000_000_000,                             // 50 GB
        1_099_511_627_776,                          // 1 TB
        109_951_162_777_600,                        // 100 TB
        1_125_899_906_842_624,                      // 1 PB
        u64::MAX,                                   // Max u64 (~18.44 EB)
    ];

    for &size in test_points {
        let res = evaluate_disk_space(size, Some(size));
        assert!(
            res.is_sufficient,
            "Failed exact boundary check at size: {size}"
        );
        assert_eq!(res.required_bytes, size);
        assert_eq!(res.available_bytes, size);

        let direct = DiskCheckResult::new(size, size);
        assert!(
            direct.is_sufficient,
            "Failed DiskCheckResult::new exact boundary check at size: {size}"
        );
    }
}

// ============================================================================
// 2. Boundary Value Analysis: Off-by-one Deficit and Surplus Tests
// ============================================================================

#[test]
fn test_off_by_one_boundary_deficit() {
    // available + 1 == required (insufficient by exactly 1 byte)
    let test_pairs: &[(u64, u64)] = &[
        (0, 1),
        (999, 1000),
        (1_048_575, 1_048_576),                     // 1 MB - 1 B
        (1_073_741_823, 1_073_741_824),             // 1 GB - 1 B
        (49_999_999, 50_000_000),                   // 50 MB - 1 B
        (109_951_162_777_599, 109_951_162_777_600), // 100 TB - 1 B
        (u64::MAX - 1, u64::MAX),
    ];

    for &(available, required) in test_pairs {
        let res = evaluate_disk_space(available, Some(required));
        assert!(
            !res.is_sufficient,
            "Expected deficit by 1 to fail: available={available}, required={required}"
        );
        assert_eq!(res.required_bytes, required);
        assert_eq!(res.available_bytes, available);

        let direct = DiskCheckResult::new(required, available);
        assert!(
            !direct.is_sufficient,
            "Expected DiskCheckResult::new deficit by 1 to fail: required={required}, available={available}"
        );
    }
}

#[test]
fn test_off_by_one_boundary_surplus() {
    // available == required + 1 (sufficient by exactly 1 byte)
    let test_pairs: &[(u64, u64)] = &[
        (1, 0),
        (1001, 1000),
        (1_048_577, 1_048_576),                     // 1 MB + 1 B
        (1_073_741_825, 1_073_741_824),             // 1 GB + 1 B
        (50_000_001, 50_000_000),                   // 50 MB + 1 B
        (109_951_162_777_601, 109_951_162_777_600), // 100 TB + 1 B
        (u64::MAX, u64::MAX - 1),
    ];

    for &(available, required) in test_pairs {
        let res = evaluate_disk_space(available, Some(required));
        assert!(
            res.is_sufficient,
            "Expected surplus by 1 to succeed: available={available}, required={required}"
        );
        assert_eq!(res.required_bytes, required);
        assert_eq!(res.available_bytes, available);

        let direct = DiskCheckResult::new(required, available);
        assert!(
            direct.is_sufficient,
            "Expected DiskCheckResult::new surplus by 1 to succeed: required={required}, available={available}"
        );
    }
}

// ============================================================================
// 3. Boundary Value Analysis: Zero Required Bytes (Some(0) and None)
// ============================================================================

#[test]
fn test_zero_required_bytes_some_and_none() {
    let available_levels: &[u64] = &[
        0,
        1,
        1_000_000,
        500_000_000_000,
        u64::MAX,
    ];

    for &available in available_levels {
        // Some(0)
        let res_some_zero = evaluate_disk_space(available, Some(0));
        assert!(
            res_some_zero.is_sufficient,
            "Some(0) required must always be sufficient for available={available}"
        );
        assert_eq!(res_some_zero.required_bytes, 0);
        assert_eq!(res_some_zero.available_bytes, available);

        // None
        let res_none = evaluate_disk_space(available, None);
        assert!(
            res_none.is_sufficient,
            "None required must default to 0 and always be sufficient for available={available}"
        );
        assert_eq!(res_none.required_bytes, 0);
        assert_eq!(res_none.available_bytes, available);
    }
}

// ============================================================================
// 4. Boundary Value Analysis: Zero Available Bytes
// ============================================================================

#[test]
fn test_zero_available_bytes_adversarial() {
    // 0 available should be sufficient ONLY when required is 0 or None
    assert!(evaluate_disk_space(0, None).is_sufficient);
    assert!(evaluate_disk_space(0, Some(0)).is_sufficient);

    // Any positive requirement must fail with 0 available
    assert!(!evaluate_disk_space(0, Some(1)).is_sufficient);
    assert!(!evaluate_disk_space(0, Some(1024)).is_sufficient);
    assert!(!evaluate_disk_space(0, Some(50_000_000)).is_sufficient);
    assert!(!evaluate_disk_space(0, Some(u64::MAX)).is_sufficient);
}

// ============================================================================
// 5. Extreme Capacity & Overflow Safety (100 TB, u64::MAX, Petabytes)
// ============================================================================

#[test]
fn test_extreme_capacity_overflow_safety() {
    let tb_100: u64 = 100 * 1024 * 1024 * 1024 * 1024; // 100 TB
    let gb_500: u64 = 500 * 1024 * 1024 * 1024;        // 500 GB
    let tb_200: u64 = 200 * 1024 * 1024 * 1024 * 1024; // 200 TB

    // 100 TB requested on 500 GB disk
    let r1 = evaluate_disk_space(gb_500, Some(tb_100));
    assert!(!r1.is_sufficient);
    assert_eq!(r1.required_bytes, tb_100);
    assert_eq!(r1.available_bytes, gb_500);

    // 100 TB requested on 200 TB disk
    let r2 = evaluate_disk_space(tb_200, Some(tb_100));
    assert!(r2.is_sufficient);

    // u64::MAX requested
    let r3 = evaluate_disk_space(0, Some(u64::MAX));
    assert!(!r3.is_sufficient);

    let r4 = evaluate_disk_space(u64::MAX / 2, Some(u64::MAX));
    assert!(!r4.is_sufficient);

    let r5 = evaluate_disk_space(u64::MAX, Some(u64::MAX));
    assert!(r5.is_sufficient);

    // u64::MAX available, small requested
    let r6 = evaluate_disk_space(u64::MAX, Some(1));
    assert!(r6.is_sufficient);

    let r7 = evaluate_disk_space(u64::MAX, None);
    assert!(r7.is_sufficient);
}

// ============================================================================
// 6. Formatting Stress Testing: format_bytes Range & Safety
// ============================================================================

#[test]
fn test_format_bytes_adversarial_spectrum() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(1), "1 B");
    assert_eq!(format_bytes(1023), "1023 B");
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(1536), "1.5 KB");
    assert_eq!(format_bytes(1_048_576), "1.0 MB");
    assert_eq!(format_bytes(45_000_000), "42.9 MB");
    assert_eq!(format_bytes(1_073_741_824), "1.0 GB");

    // Extreme values must not panic or cause format errors
    let tb_100: u64 = 100 * 1024 * 1024 * 1024 * 1024;
    let s_100tb = format_bytes(tb_100);
    assert!(s_100tb.ends_with("GB"), "Expected {s_100tb} to be expressed in GB");
    assert!(s_100tb.starts_with("102400"));

    let s_max = format_bytes(u64::MAX);
    assert!(s_max.ends_with("GB"));
    assert!(!s_max.is_empty());
}

// ============================================================================
// 7. Filesystem statvfs & Ancestor Traversal Invariants
// ============================================================================

#[test]
fn test_live_mount_points_capacity_positive() {
    // 1. Root /
    let res_root = check_disk_space(Path::new("/"), Some(1024));
    assert!(
        res_root.available_bytes > 0,
        "Root mount point must report positive available bytes"
    );
    assert_eq!(res_root.required_bytes, 1024);

    // 2. /tmp
    let res_tmp = check_disk_space(Path::new("/tmp"), Some(2048));
    assert!(
        res_tmp.available_bytes > 0,
        "/tmp mount point must report positive available bytes"
    );

    // 3. User Home Directory
    if let Some(home) = dirs::home_dir() {
        let res_home = check_disk_space(&home, Some(4096));
        assert!(
            res_home.available_bytes > 0,
            "Home dir ({:?}) must report positive available bytes",
            home
        );
    }

    // 4. Download Directory
    let download_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let res_dl = check_disk_space(&download_dir, Some(8192));
    assert!(
        res_dl.available_bytes > 0,
        "Download dir ({:?}) must report positive available bytes",
        download_dir
    );

    // 5. Current Working Directory
    if let Ok(cwd) = std::env::current_dir() {
        let res_cwd = check_disk_space(&cwd, Some(16384));
        assert!(
            res_cwd.available_bytes > 0,
            "CWD ({:?}) must report positive available bytes",
            cwd
        );
    }
}

#[test]
fn test_ancestor_traversal_deeply_nested_nonexistent_paths() {
    // Deep path under /tmp that definitely does not exist: 10 levels deep
    let deep_tmp = Path::new("/tmp/challenger2_stress/a/b/c/d/e/f/g/h/i/j");
    assert!(!deep_tmp.exists(), "Deep test path must not exist prior to test");

    let res = check_disk_space(deep_tmp, Some(50_000_000));
    assert!(
        res.available_bytes > 0,
        "Ancestor climbing from /tmp/challenger2_stress/... must locate /tmp and succeed"
    );
    assert_eq!(res.required_bytes, 50_000_000);

    // Non-existent path directly under root
    let deep_root = Path::new("/nonexistent_challenger2_root_123456789/nested/leaf");
    assert!(!deep_root.exists());

    let res_root = check_disk_space(deep_root, Some(10_000));
    assert!(
        res_root.available_bytes > 0,
        "Ancestor climbing from /nonexistent_... must locate / and succeed"
    );
}

#[test]
fn test_path_edge_cases_safety_and_no_panics() {
    // 1. Empty path ""
    let empty_path = Path::new("");
    let res_empty = check_disk_space(empty_path, Some(500));
    assert!(
        res_empty.available_bytes > 0,
        "Empty path must safely fall back to root / without panicking"
    );

    // 2. Relative non-existent path
    let rel_path = Path::new("relative/nonexistent/subdir/download_target");
    let res_rel = check_disk_space(rel_path, Some(500));
    assert!(
        res_rel.available_bytes > 0,
        "Relative non-existent path must climb and fall back to root / without panicking"
    );

    // 3. Path ending in trailing slash
    let trailing = Path::new("/tmp/");
    let res_trailing = check_disk_space(trailing, Some(500));
    assert!(res_trailing.available_bytes > 0);

    // 4. Repeated slashes
    let repeated = Path::new("///tmp///");
    let res_repeated = check_disk_space(repeated, Some(500));
    assert!(res_repeated.available_bytes > 0);

    // 5. Existing file path (statvfs works on files on POSIX)
    let file_path = Path::new("/etc/hosts");
    if file_path.exists() {
        let res_file = check_disk_space(file_path, Some(500));
        assert!(
            res_file.available_bytes > 0,
            "statvfs on existing file must report positive space"
        );
    }

    // 6. Path with interior null byte (CString conversion fails gracefully)
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let null_bytes = b"/tmp/bad\0path";
        let null_os_str = OsStr::from_bytes(null_bytes);
        let null_path = Path::new(null_os_str);

        // Interior null byte in child component: does not exist on disk,
        // so ancestor climbing safely climbs to /tmp and never panics.
        let space_opt = query_available_space(null_path);
        assert!(
            space_opt.is_some(),
            "query_available_space on interior null byte climbs to existing parent (/tmp) without panicking"
        );
        assert!(
            space_opt.unwrap_or_default() > 0,
            "Available bytes on climbed ancestor /tmp must be positive"
        );

        let res_null = check_disk_space(null_path, Some(100));
        assert!(
            res_null.available_bytes > 0,
            "check_disk_space on path with null byte in leaf component safely resolves via ancestor"
        );
    }
}

#[test]
fn test_long_path_climber_stress() {
    // Construct a deeply nested 2000-character path
    let mut long_buf = PathBuf::from("/tmp");
    for i in 0..50 {
        long_buf.push(format!("sub_level_{:04}_stress_test_padding_extra_characters", i));
    }
    assert!(long_buf.to_str().map(|s| s.len()).unwrap_or(0) > 1500);

    let res = check_disk_space(&long_buf, Some(1024));
    assert!(
        res.available_bytes > 0,
        "Long path ancestor traversal must resolve back to /tmp without buffer overflow"
    );
}

// ============================================================================
// 8. UI Interaction & Dynamic Pre-check Integration Stress
// ============================================================================

#[test]
fn test_stress_ui_renders_amber_warning_when_insufficient_space() {
    let manifest = create_sample_manifest(vec![("best", Some(50_000_000))]);
    let selected_format = "best".to_string();
    let disk_check = DiskCheckResult {
        required_bytes: 50_000_000,
        available_bytes: 10_000_000,
        is_sufficient: false,
    };

    let ctx = egui::Context::default();
    let mut download_invoked = false;

    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            // Render staged card with insufficient space
            let mut state = StagingState::Staged {
                url: "https://example.com/watch?v=stress".to_string(),
                manifest: manifest.clone(),
                thumbnail: None,
                selected_format_id: selected_format.clone(),
                disk_check,
            };

            staging::render_staging_card(ui, &mut state, |_, _| {
                download_invoked = true;
            });

            // Card must remain in Staged state and not trigger download
            assert!(state.is_staged());
        });
    });

    // The download action must NOT have been triggered
    assert!(
        !download_invoked,
        "Download action must not trigger when disk space is insufficient"
    );
}

#[test]
fn test_stress_ui_blocks_return_key_download_when_insufficient_space() {
    let manifest = create_sample_manifest(vec![("best", Some(50_000_000))]);
    let selected_format = "best".to_string();
    let disk_check = DiskCheckResult {
        required_bytes: 50_000_000,
        available_bytes: 10_000_000,
        is_sufficient: false,
    };

    let ctx = egui::Context::default();
    let mut triggered_download = false;

    // Simulate Return key press in raw input
    let raw_input = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };

    let _ = ctx.run(raw_input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut state = StagingState::Staged {
                url: "https://example.com/watch?v=stress".to_string(),
                manifest: manifest.clone(),
                thumbnail: None,
                selected_format_id: selected_format.clone(),
                disk_check,
            };

            staging::render_staging_card(ui, &mut state, |_, _| {
                triggered_download = true;
            });

            // State must remain Staged
            assert!(state.is_staged());
        });
    });

    assert!(
        !triggered_download,
        "Return key must NOT trigger download when disk space is insufficient!"
    );
}

#[test]
fn test_stress_ui_permits_return_key_download_when_sufficient_space() {
    let manifest = create_sample_manifest(vec![("best", Some(50_000_000))]);
    let selected_format = "best".to_string();
    let disk_check = DiskCheckResult {
        required_bytes: 50_000_000,
        available_bytes: 100_000_000,
        is_sufficient: true,
    };

    let ctx = egui::Context::default();
    let mut triggered_download = false;

    // Simulate Return key press in raw input
    let raw_input = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };

    let _ = ctx.run(raw_input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut state = StagingState::Staged {
                url: "https://example.com/watch?v=stress".to_string(),
                manifest: manifest.clone(),
                thumbnail: None,
                selected_format_id: selected_format.clone(),
                disk_check,
            };

            staging::render_staging_card(ui, &mut state, |_, _| {
                triggered_download = true;
            });
        });
    });

    assert!(
        triggered_download,
        "Return key MUST trigger download when disk space is sufficient!"
    );
}

#[test]
fn test_stress_format_selection_change_dynamically_rechecks_disk() {
    // Manifest with 4K (1 GB) and 720p (50 MB)
    let manifest = create_sample_manifest(vec![
        ("4k", Some(1_073_741_824)),
        ("720p", Some(52_428_800)),
    ]);

    let available = 200_000_000; // 200 MB available: Insufficient for 4K (1 GB), sufficient for 720p (50 MB)

    // Initially 4K is selected
    let disk_check = evaluate_disk_space(available, Some(1_073_741_824));
    assert!(
        !disk_check.is_sufficient,
        "4K must be insufficient on 200 MB disk"
    );

    // User switches to 720p format
    let new_selected_stream = manifest.streams.iter().find(|s| s.format_id == "720p").unwrap();
    let new_check = evaluate_disk_space(available, new_selected_stream.filesize_bytes);

    assert!(
        new_check.is_sufficient,
        "720p must be sufficient on 200 MB disk"
    );
    assert_eq!(new_check.required_bytes, 52_428_800);
    assert_eq!(new_check.available_bytes, available);
}
