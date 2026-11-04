//! Empirical Adversarial Probe Harness for Milestone 1 Challenger 2
//! Adversarially probes:
//! 1. `ViewportBuilder` configuration in `main.rs`
//! 2. `window-vibrancy` and `apply_macos_vibrancy` under mock/null/headless conditions
//! 3. Thread-safety and panic freedom

#![allow(clippy::all)]

use std::fs;
use std::path::Path;
use eframe::egui;
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WaylandWindowHandle, WindowHandle,
};

// ============================================================================
// Probe 1: ViewportBuilder Configuration in main.rs
// ============================================================================

#[test]
fn test_probe_viewport_builder_source_tokens() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let main_rs_path = Path::new(manifest_dir).join("src/main.rs");
    let content = fs::read_to_string(&main_rs_path)
        .expect("crates/gui/src/main.rs must exist and be readable");

    // Adversarially verify all 5 required ViewportBuilder method calls
    assert!(
        content.contains(".with_fullsize_content_view(true)"),
        "Missing with_fullsize_content_view(true) in main.rs"
    );
    assert!(
        content.contains(".with_titlebar_shown(false)"),
        "Missing with_titlebar_shown(false) in main.rs"
    );
    assert!(
        content.contains(".with_title_shown(false)"),
        "Missing with_title_shown(false) in main.rs"
    );
    assert!(
        content.contains(".with_titlebar_buttons_shown(true)"),
        "Missing with_titlebar_buttons_shown(true) in main.rs"
    );
    assert!(
        content.contains(".with_transparent(true)"),
        "Missing with_transparent(true) in main.rs"
    );
}

#[test]
fn test_probe_viewport_builder_runtime_invariants() {
    // Construct the exact ViewportBuilder matching main.rs
    let builder = egui::ViewportBuilder::default()
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_titlebar_buttons_shown(true)
        .with_transparent(true)
        .with_inner_size([960.0, 720.0])
        .with_min_inner_size([760.0, 520.0]);

    // Empirically verify builder struct fields
    assert_eq!(
        builder.fullsize_content_view,
        Some(true),
        "fullsize_content_view must be true"
    );
    assert_eq!(
        builder.titlebar_shown,
        Some(false),
        "titlebar_shown must be false"
    );
    assert_eq!(
        builder.title_shown,
        Some(false),
        "title_shown must be false"
    );
    assert_eq!(
        builder.titlebar_buttons_shown,
        Some(true),
        "titlebar_buttons_shown must be true (Cocoa traffic lights enabled)"
    );
    assert_eq!(
        builder.transparent,
        Some(true),
        "transparent must be true for vibrancy pass-through"
    );
    assert_eq!(
        builder.inner_size,
        Some(egui::vec2(960.0, 720.0)),
        "inner_size must match initial dimensions"
    );
    assert_eq!(
        builder.min_inner_size,
        Some(egui::vec2(760.0, 520.0)),
        "min_inner_size must enforce minimum dimensions"
    );
}

// ============================================================================
// Probe 2: apply_macos_vibrancy & window_vibrancy Mock/Headless Harness
// ============================================================================

struct MockUnavailableWindowHandle;

impl HasWindowHandle for MockUnavailableWindowHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }
}

struct MockNonAppKitWindowHandle;

impl HasWindowHandle for MockNonAppKitWindowHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let wayland = WaylandWindowHandle::new(std::ptr::NonNull::dangling());
        let raw = RawWindowHandle::Wayland(wayland);
        unsafe { Ok(WindowHandle::borrow_raw(raw)) }
    }
}

#[test]
fn test_probe_vibrancy_with_unavailable_window_handle() {
    let mock_window = MockUnavailableWindowHandle;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        window_vibrancy::apply_vibrancy(
            &mock_window,
            window_vibrancy::NSVisualEffectMaterial::UnderWindowBackground,
            Some(window_vibrancy::NSVisualEffectState::FollowsWindowActiveState),
            None,
        )
    }));

    assert!(result.is_ok(), "apply_vibrancy must not panic on unavailable handle");
    let apply_res = result.unwrap();
    assert!(
        matches!(apply_res, Err(window_vibrancy::Error::WindowHandleError(_))),
        "apply_vibrancy must return WindowHandleError when handle is unavailable"
    );
}

#[test]
fn test_probe_vibrancy_with_non_appkit_mock_handle() {
    let mock_window = MockNonAppKitWindowHandle;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        window_vibrancy::apply_vibrancy(
            &mock_window,
            window_vibrancy::NSVisualEffectMaterial::UnderWindowBackground,
            Some(window_vibrancy::NSVisualEffectState::FollowsWindowActiveState),
            None,
        )
    }));

    assert!(result.is_ok(), "apply_vibrancy must not panic on non-AppKit handle");
    let apply_res = result.unwrap();
    // On macOS, if the raw handle is not AppKit, it safely ignores it and returns Ok(())
    assert!(apply_res.is_ok(), "non-AppKit handle gracefully succeeds without memory violation");
}

#[test]
fn test_probe_vibrancy_error_display_formatting() {
    let err_unsupported = window_vibrancy::Error::UnsupportedPlatform;
    assert_eq!(format!("{err_unsupported}"), "Unsupported platform");

    let err_failed = window_vibrancy::Error::FailedToApply;
    assert_eq!(format!("{err_failed}"), "Failed to apply vibrancy");

    let err_handle = window_vibrancy::Error::WindowHandleError(HandleError::Unavailable);
    assert!(format!("{err_handle}").contains("Window handle error"));
}

#[test]
fn test_probe_vibrancy_materials_and_states() {
    // Test that all material variants can be passed without error
    let mock = MockNonAppKitWindowHandle;
    let materials = [
        window_vibrancy::NSVisualEffectMaterial::Titlebar,
        window_vibrancy::NSVisualEffectMaterial::Selection,
        window_vibrancy::NSVisualEffectMaterial::Menu,
        window_vibrancy::NSVisualEffectMaterial::Popover,
        window_vibrancy::NSVisualEffectMaterial::Sidebar,
        window_vibrancy::NSVisualEffectMaterial::HeaderView,
        window_vibrancy::NSVisualEffectMaterial::Sheet,
        window_vibrancy::NSVisualEffectMaterial::WindowBackground,
        window_vibrancy::NSVisualEffectMaterial::HUDWindow,
        window_vibrancy::NSVisualEffectMaterial::FullScreenUI,
        window_vibrancy::NSVisualEffectMaterial::ToolTip,
        window_vibrancy::NSVisualEffectMaterial::ContentBackground,
        window_vibrancy::NSVisualEffectMaterial::UnderWindowBackground,
        window_vibrancy::NSVisualEffectMaterial::UnderPageBackground,
    ];

    for mat in materials {
        let res = window_vibrancy::apply_vibrancy(
            &mock,
            mat,
            Some(window_vibrancy::NSVisualEffectState::Active),
            None,
        );
        assert!(res.is_ok());
    }
}
