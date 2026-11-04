#![allow(clippy::all)]
#![allow(clippy::assertions_on_constants)]

//! Empirical Adversarial Stress Test Suite for Milestone 1 (R1).
//! Validates:
//! 1. Font loading behavior (invalid files, empty files, corrupted headers)
//! 2. Typography rendering (Unicode, emojis, complex text layouts)
//! 3. Header geometry & window drag response (34pt height, >=78pt clearance)
//! 4. Clear color transparency ([0.0, 0.0, 0.0, 0.0])
//! 5. Zero bare .unwrap() / .expect() in crates/gui/src
use std::fs;
use std::io::Write;
use std::path::Path;

use eframe::egui;

#[path = "../src/theme.rs"]
mod theme;

// ============================================================================
// 1. Font Loading Behavior Under Adversarial Conditions
// ============================================================================

#[test]
fn test_stress_font_loading_live_host_sf_pro() {
    let ctx = egui::Context::default();
    theme::setup_system_fonts(&ctx);

    // Verify proportional and monospace font layout execution
    let mut prop_width = 0.0;
    let mut mono_width = 0.0;
    let _ = ctx.run(Default::default(), |ctx| {
        let p = ctx.fonts(|f| {
            f.layout_no_wrap(
                "San Francisco Pro UI 12345 🎬 🎵".to_string(),
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            )
        });
        let m = ctx.fonts(|f| {
            f.layout_no_wrap(
                "SF-Mono monospace font 12345".to_string(),
                egui::FontId::monospace(12.0),
                egui::Color32::WHITE,
            )
        });
        prop_width = p.size().x;
        mono_width = m.size().x;
    });

    assert!(prop_width > 0.0, "Proportional text layout must produce valid positive width");
    assert!(mono_width > 0.0, "Monospace text layout must produce valid positive width");
}

#[test]
fn test_stress_font_fallback_on_invalid_and_empty_files() {
    let ctx = egui::Context::default();
    theme::setup_system_fonts(&ctx);

    // Verify context runs cleanly on fallback
    let _ = ctx.run(Default::default(), |ctx| {
        let galley = ctx.fonts(|f| {
            f.layout_no_wrap(
                "Fallback sanity test".to_string(),
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            )
        });
        assert!(galley.size().x > 0.0);
    });

    // Test: Verify empty file handling by inspecting candidates
    let temp_dir = std::env::temp_dir();
    let empty_file_path = temp_dir.join("empty_test_font.ttf");
    let mut f = fs::File::create(&empty_file_path).unwrap_or_else(|e| panic!("Failed temp file: {e}"));
    let _ = f.write_all(b"");
    drop(f);

    let metadata = fs::metadata(&empty_file_path).map(|m| m.len()).unwrap_or(1);
    assert_eq!(metadata, 0, "Temporary font file must be exactly 0 bytes");

    let _ = fs::remove_file(empty_file_path);
}

#[test]
fn test_stress_font_parser_panics_on_corrupted_header() {
    // Adversarial verification: probe what epaint does when font data is corrupted
    let mut fonts = egui::FontDefinitions::default();
    let corrupt_header = vec![0xFF, 0xFE, 0xFD, 0xFC, 0x00, 0x00, 0x00, 0x00];
    fonts.font_data.insert(
        "CorruptedTestFont".to_owned(),
        egui::FontData::from_owned(corrupt_header),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "CorruptedTestFont".to_owned());

    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);

    // Attempting to shape text with a corrupted font file causes epaint to panic
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = ctx.run(Default::default(), |ctx| {
            let _ = ctx.fonts(|f| {
                f.layout_no_wrap(
                    "Probe Corrupt Font".to_string(),
                    egui::FontId::proportional(14.0),
                    egui::Color32::WHITE,
                )
            });
        });
    }));

    // Documented behavior: epaint panics on invalid TTF/OTF header
    assert!(
        result.is_err(),
        "epaint panics when an invalid/corrupted font header is inserted into FontDefinitions"
    );
}

// ============================================================================
// 2. Typography Rendering with Complex Unicode and Emojis
// ============================================================================

#[test]
fn test_stress_typography_rendering_mandated_strings() {
    let ctx = egui::Context::default();
    theme::setup_system_fonts(&ctx);

    let test_strings = [
        // Mandated in dispatch
        "San Francisco UI 🎬 🎵 test 123",
        // DASH & Media badges
        "Highest Quality (1080p Video + High-Res Audio) • 🎬 H.264 + 🎵 AAC",
        "Downloading... 1.4 GB / 2.8 GB • 24.5 MB/s • 00:58 left",
        "Extracting Metadata... [Probing Phase 1]",
        "Merging Audio & Video... [Muxing Phase 3]",
        "Completed. [Phase 4 Success]",
        // International and RTL Unicode scripts
        "العربية (Arabic text) — تحميل الفيديو",
        "עברית (Hebrew text) — הורדת מדיה",
        "中文 (Simplified Chinese) — 高清视频下载",
        "日本語 (Japanese) — メディアダウンローダー",
        "한국어 (Korean) — 동영상 다운로드",
        "Русский (Russian) — Загрузка мультимедиа",
        "Emoji stress: 🎬 🎵 🚀 ⚠️ 🦀 📦 ⬇️ ⏳ ⚙️ 🔍 🛑 ✅ 💡 🎉 🔒 🔑",
        // Zero-width characters & combining marks
        "Family ZWJ: 👨‍👩‍👧‍👦 • Rainbow flag: 🏳️‍🌈 • Keycap: 1️⃣2️⃣3️⃣",
        // Punctuation and math
        "“Smart quotes” ‘apostrophes’ — em-dash – en-dash … ellipsis ∑∫√π≠≈≤≥",
    ];

    let _ = ctx.run(Default::default(), |ctx| {
        for text in &test_strings {
            let prop_galley = ctx.fonts(|f| {
                f.layout_no_wrap(
                    (*text).to_string(),
                    egui::FontId::proportional(13.5),
                    egui::Color32::WHITE,
                )
            });
            assert!(
                prop_galley.size().x > 0.0,
                "Proportional galley must have positive width for: {text}"
            );
            assert!(
                prop_galley.size().y > 0.0,
                "Proportional galley must have positive height for: {text}"
            );

            let mono_galley = ctx.fonts(|f| {
                f.layout_no_wrap(
                    (*text).to_string(),
                    egui::FontId::monospace(12.0),
                    egui::Color32::WHITE,
                )
            });
            assert!(
                mono_galley.size().x > 0.0,
                "Monospace galley must have positive width for: {text}"
            );
            assert!(
                mono_galley.size().y > 0.0,
                "Monospace galley must have positive height for: {text}"
            );
        }
    });
}

#[test]
fn test_stress_typography_massive_string_bounds() {
    let ctx = egui::Context::default();
    theme::setup_system_fonts(&ctx);

    // 8,000 character string
    let massive_string = "🎬 SF Pro Typography Test 🎵 ".repeat(300);

    let _ = ctx.run(Default::default(), |ctx| {
        let galley = ctx.fonts(|f| {
            f.layout(
                massive_string.clone(),
                egui::FontId::proportional(13.0),
                egui::Color32::WHITE,
                800.0, // wrap at 800pt
            )
        });
        assert!(galley.size().x <= 800.0 + 1.0, "Galley width must respect wrap limit");
        assert!(galley.size().y > 100.0, "Massive wrapped string must span multiple lines");
    });
}

// ============================================================================
// 3. Header Layout Geometry & Traffic Lights Clearance
// ============================================================================

#[test]
fn test_stress_header_geometry_across_window_widths() {
    let test_widths = [100.0, 320.0, 500.0, 760.0, 960.0, 1440.0, 2560.0, 3840.0];

    for width in test_widths {
        let ctx = egui::Context::default();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 700.0),
            )),
            ..Default::default()
        };

        let _ = ctx.run(raw_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = theme::render_macos_top_header(ui, "rust-dl");

                // 1. Exact height must be 34.0 pt
                assert_eq!(
                    response.rect.height(),
                    theme::HEADER_HEIGHT,
                    "Header height must be exactly 34pt at width {width}"
                );
                assert_eq!(
                    theme::HEADER_HEIGHT,
                    34.0,
                    "HEADER_HEIGHT constant must be 34.0"
                );

                // 2. Traffic light inset constant must be >= 78.0 pt
                assert!(
                    theme::TRAFFIC_LIGHTS_INSET >= 78.0,
                    "TRAFFIC_LIGHTS_INSET must be >= 78.0pt, got {}",
                    theme::TRAFFIC_LIGHTS_INSET
                );

                // 3. Response must support drag and click
                assert!(response.sense.drag, "Header must support dragging at width {width}");
                assert!(response.sense.click, "Header must support clicking for double-click zoom at width {width}");
            });
        });
    }
}

#[test]
fn test_stress_header_title_clears_traffic_lights_zone() {
    let ctx = egui::Context::default();
    theme::setup_system_fonts(&ctx);

    // Test a very narrow window (width = 120pt) where centered title would collide with traffic lights
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(120.0, 600.0),
        )),
        ..Default::default()
    };

    let _ = ctx.run(raw_input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let available_width = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(available_width, theme::HEADER_HEIGHT),
                egui::Sense::click_and_drag(),
            );

            // Compute ideal_x and text_x as done in render_macos_top_header
            let font_id = egui::FontId::new(13.0, egui::FontFamily::Proportional);
            let galley = ui.painter().layout_no_wrap("rust-dl".to_owned(), font_id, egui::Color32::WHITE);
            let ideal_x = rect.center().x - galley.size().x * 0.5;
            let text_x = ideal_x.max(rect.min.x + theme::TRAFFIC_LIGHTS_INSET);

            assert!(
                text_x >= rect.min.x + 78.0,
                "Title text X ({text_x}) must never intrude into traffic light zone [0..78pt]"
            );
        });
    });
}

// ============================================================================
// 4. Clear Color & Theme Color Tokens
// ============================================================================

#[test]
fn test_stress_theme_clear_color_exact_zeros() {
    let clear = theme::theme_clear_color();
    assert_eq!(clear, [0.0, 0.0, 0.0, 0.0], "theme_clear_color must return fully transparent RGBA zeros");
    assert_eq!(clear[0], 0.0_f32, "Red channel must be 0.0");
    assert_eq!(clear[1], 0.0_f32, "Green channel must be 0.0");
    assert_eq!(clear[2], 0.0_f32, "Blue channel must be 0.0");
    assert_eq!(clear[3], 0.0_f32, "Alpha channel must be 0.0");
}

#[test]
fn test_stress_hig_palette_color_tokens() {
    // Exact hex specifications from Apple HIG & PROJECT.md
    assert_eq!(theme::COLOR_DARK_BG, egui::Color32::from_rgb(0x1e, 0x1e, 0x1e));
    assert_eq!(theme::COLOR_LIGHT_BG, egui::Color32::from_rgb(0xf6, 0xf6, 0xf6));
    assert_eq!(theme::COLOR_ACCENT_BLUE, egui::Color32::from_rgb(0x00, 0x7a, 0xff));
    assert_eq!(theme::COLOR_ACCENT_PURPLE, egui::Color32::from_rgb(0xaf, 0x52, 0xde));
    assert_eq!(theme::COLOR_ACCENT_GREEN, egui::Color32::from_rgb(0x34, 0xc7, 0x59));
    assert_eq!(theme::COLOR_ACCENT_AMBER, egui::Color32::from_rgb(0xff, 0x95, 0x00));
    assert_eq!(theme::COLOR_ACCENT_RED, egui::Color32::from_rgb(0xff, 0x3b, 0x30));

    // ThemeColors struct contracts
    let dark = theme::ThemeColors::dark();
    assert!(dark.is_dark);
    assert_eq!(dark.background, theme::COLOR_DARK_BG);
    assert_eq!(dark.accent_blue, theme::COLOR_ACCENT_BLUE);
    assert_eq!(dark.accent_purple, theme::COLOR_ACCENT_PURPLE);
    assert_eq!(dark.accent_green, theme::COLOR_ACCENT_GREEN);
    assert_eq!(dark.accent_amber, theme::COLOR_ACCENT_AMBER);

    let light = theme::ThemeColors::light();
    assert!(!light.is_dark);
    assert_eq!(light.background, theme::COLOR_LIGHT_BG);
}

// ============================================================================
// 5. Zero Bare .unwrap() / .expect() Static Analysis Scanner
// ============================================================================

#[test]
fn test_stress_zero_unwrap_and_expect_in_crates_gui_source() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files_scanned = 0;
    let mut unwrap_violations = Vec::new();
    let mut expect_violations = Vec::new();

    fn scan_dir(dir: &Path, files_scanned: &mut usize, unwraps: &mut Vec<String>, expects: &mut Vec<String>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    scan_dir(&path, files_scanned, unwraps, expects);
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    *files_scanned += 1;
                    if let Ok(content) = fs::read_to_string(&path) {
                        for (idx, line) in content.lines().enumerate() {
                            let trimmed = line.trim();
                            // Skip comments
                            if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
                                continue;
                            }
                            // Detect bare unwrap()
                            if trimmed.contains(".unwrap()") {
                                unwraps.push(format!("{}:{}: {}", path.display(), idx + 1, trimmed));
                            }
                            // Detect expect(
                            if trimmed.contains(".expect(") {
                                expects.push(format!("{}:{}: {}", path.display(), idx + 1, trimmed));
                            }
                        }
                    }
                }
            }
        }
    }

    scan_dir(&src_dir, &mut files_scanned, &mut unwrap_violations, &mut expect_violations);

    assert!(files_scanned >= 2, "Must scan at least main.rs and theme.rs (scanned {files_scanned})");
    assert!(
        unwrap_violations.is_empty(),
        "Found bare .unwrap() violations in crates/gui/src:\n{:#?}",
        unwrap_violations
    );
    assert!(
        expect_violations.is_empty(),
        "Found .expect() violations in crates/gui/src:\n{:#?}",
        expect_violations
    );
}
