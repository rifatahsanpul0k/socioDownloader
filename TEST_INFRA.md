# TEST_INFRA.md — E2E Test Suite & Infrastructure Specification

## 1. Overview & Test Architecture

The E2E test suite for the `rust-dl` macOS media downloader GUI rewrite is an opaque-box, headless integration test suite designed to verify strict adherence to Apple's Human Interface Guidelines (HIG), Jakob Nielsen's 10 Usability Heuristics, and zero `.unwrap()` / `.expect()` safety guarantees.

The test infrastructure is located under:
```
tests/e2e/
├── mod.rs                  # Test suite root & module declarations
├── harness.rs              # Headless egui runner, design tokens, mock contracts & fixtures
├── tier1_features.rs       # Tier 1: Feature Coverage (>=5 tests per feature across all 18 features)
├── tier2_boundaries.rs     # Tier 2: Boundary & Corner Cases (>=5 tests per feature with boundaries)
├── tier3_combinations.rs   # Tier 3: Cross-Feature Combinations (Pairwise component interactions)
└── tier4_scenarios.rs      # Tier 4: Real-World Application Scenarios (Realistic user workflows)
crates/gui/tests/
└── e2e.rs                  # Cargo integration test hook (cargo test --test e2e)
```

## 2. Test Runner Execution

To execute the complete E2E test suite:
```bash
cargo test --test e2e
```

To run individual tiers:
```bash
# Tier 1: Feature Coverage (90 tests)
cargo test --test e2e tier1_features

# Tier 2: Boundary & Corner Cases (25 tests)
cargo test --test e2e tier2_boundaries

# Tier 3: Cross-Feature Combinations (10 tests)
cargo test --test e2e tier3_combinations

# Tier 4: Real-World Application Scenarios (5 tests)
cargo test --test e2e tier4_scenarios
```

To run tests with full backtrace:
```bash
RUST_BACKTRACE=1 cargo test --test e2e -- --nocapture
```

## 3. Headless Verification Methodology

The test suite exercises the GUI without requiring a physical monitor, macOS WindowServer, or Cocoa display server:
- **Headless Egui Simulation**: Uses `egui::Context::run(raw_input, ...)` inside `HeadlessUiHarness` to evaluate layout geometry, shapes, paint primitives, text styling, and colors in pure offscreen memory.
- **Event Dispatch Simulation**: Simulates user interactions including keyboard events (`Key::Enter`, Return key) and mouse clicks on buttons (`Connect Browser Cookies`, `Retry`, `Show in Finder`, `Undo`).
- **State Machine Verification**: Verifies transitions across `StagingState` (`Idle` -> `Probing` -> `Staged` -> `Error`), `DownloadPhase` (`Probing` -> `Downloading` -> `Muxing` -> `Completed`), and `ToastManager` (auto-detection, deduplication, 5s countdown, undo restoration).
- **Static Code Analysis**: Audits `crates/gui/src/` to verify zero occurrences of `.unwrap()` and `.expect()` calls in non-comment code.

## 4. Authoritative Sources of Expected Outputs

Every test derives its expected outputs directly from the project requirements:
- **ORIGINAL_REQUEST.md**:
  - R1: Transparent titlebar, frosted glass vibrancy, SF Pro system font loading.
  - R2: Staging Card, DASH output badges (`Highest Quality (1080p Video + High-Res Audio)` & `🎬 H.264 + 🎵 AAC`), pre-download disk space check.
  - R3: 10pt rounded cards with drop shadows, 4-phase progress bars (`#007AFF` blue probing, `#007AFF` blue downloading metrics, `#AF52DE` purple muxing, `#34C759` green completed).
  - R4: Plain-language error masking, amber auth recovery card (`#FF9500`) with `[ Connect Browser Cookies ]` and `[ Retry ]`.
  - R5: Bottom-right toasts, clipboard link auto-detect ("Clipboard link detected. Press Return to add."), Return key shortcut, 5s history undo.
  - R6: Zero `.unwrap()` / `.expect()` in `crates/gui`, clean compilation.
- **PROJECT.md**:
  - Feature Inventory (18 features)
  - Milestone decomposition (M1 through M5)
  - Interface Contracts (`theme.rs`, `staging.rs`, `queue_view.rs`, `toast.rs`)

## 5. Test Tier Breakdown & Metric Inventory

| Tier | Scope | Minimum Required | Actual Tests Built | Status |
|------|-------|------------------|-------------------|--------|
| Tier 1 | Feature Coverage across all 18 features in PROJECT.md | >= 90 (5/feature) | 90 tests | PASS |
| Tier 2 | Boundary & Corner Cases (Disk space, Metrics, Toasts, URLs, Viewports) | >= 25 | 25 tests | PASS |
| Tier 3 | Cross-Feature Combinations (Pairwise interactions) | >= 10 | 10 tests | PASS |
| Tier 4 | Real-World Application Scenarios (End-to-end workflows) | >= 5 | 5 tests | PASS |
| **Total** | **Comprehensive E2E Test Suite** | **>= 130** | **130 tests** | **100% PASS** |

### Detailed Test Inventory

#### Tier 1: Feature Coverage (90 tests)
- **Feature 1: macOS Transparent Titlebar & Shell (5 tests)**
  - `test_f01_native_options_window_transparent_titlebar`: Verifies transparent clear color and window transparency.
  - `test_f01_native_options_fullsize_content_view`: Verifies fullsize content view panel height within 34pt.
  - `test_f01_traffic_light_clearance_header_dimensions`: Verifies 34pt height and 78pt left insets.
  - `test_f01_traffic_light_clearance_header_drag_sense`: Verifies draggable header response for window motion.
  - `test_f01_top_header_title_layout_and_font`: Verifies title typography and layout clearance.
- **Feature 2: Frosted Glass Vibrancy & Native Palette (5 tests)**
  - `test_f02_theme_clear_color_is_fully_transparent`: Verifies clear color is `[0.0, 0.0, 0.0, 0.0]`.
  - `test_f02_macos_dark_palette_color_token`: Verifies macOS native dark `#1E1E1E` (RGB 30, 30, 30).
  - `test_f02_macos_light_palette_color_token`: Verifies macOS native light `#F6F6F6` (RGB 246, 246, 246).
  - `test_f02_vibrancy_fallback_color_contrast`: Verifies dark/light contrast ratio > 0.8.
  - `test_f02_visual_style_transparency_consistency`: Verifies egui visual style consistency.
- **Feature 3: SF Pro System Font Integration (5 tests)**
  - `test_f03_system_font_paths_checked_in_priority_order`: Verifies SF Pro paths under `/System/Library/Fonts/`.
  - `test_f03_font_definitions_registers_proportional_and_monospace`: Verifies font definitions family insertion.
  - `test_f03_defensive_fallback_when_system_fonts_missing`: Verifies zero panic fallback to default egui fonts.
  - `test_f03_font_loading_never_panics_on_corrupt_file`: Verifies font loading resilience on corrupt input.
  - `test_f03_sf_pro_family_precedence_in_font_definitions`: Verifies SF Pro has highest family priority.
- **Feature 4: Pre-Queue Staging Card (5 tests)**
  - `test_f04_staging_state_initializes_to_idle`: Verifies initial `StagingState::Idle`.
  - `test_f04_staging_state_transition_idle_to_probing`: Verifies transition from `Idle` to `Probing`.
  - `test_f04_staging_state_transition_probing_to_staged`: Verifies transition from `Probing` to `Staged`.
  - `test_f04_staging_renders_thumbnail_and_metadata`: Verifies metadata fields in manifest.
  - `test_f04_staging_cancel_resets_to_idle`: Verifies cancellation resets state to `Idle`.
- **Feature 5: Merged DASH Output Badges (5 tests)**
  - `test_f05_dash_badges_default_highest_quality_text`: Verifies default `"Highest Quality (1080p Video + High-Res Audio)"`.
  - `test_f05_dash_badges_secondary_codec_badge_text`: Verifies secondary badge `"🎬 H.264 + 🎵 AAC"`.
  - `test_f05_badges_non_dash_stream_formatting`: Verifies single-stream non-DASH badge format.
  - `test_f05_badges_audio_only_stream_formatting`: Verifies audio-only stream badge format.
  - `test_f05_badges_empty_or_unknown_codec_fallback`: Verifies graceful fallback for unknown codecs.
- **Feature 6: Pre-Download Disk Space Check (5 tests)**
  - `test_f06_disk_check_sufficient_space_enables_download`: Verifies button enabled when space is sufficient.
  - `test_f06_disk_check_insufficient_space_disables_download`: Verifies button disabled when space is insufficient.
  - `test_f06_disk_check_warning_text_displayed_when_insufficient`: Verifies clear warning text format.
  - `test_f06_disk_check_handles_unknown_filesize_safely`: Verifies safe default when filesize is None.
  - `test_f06_disk_check_result_struct_invariants`: Verifies `DiskCheckResult` boundary invariants.
- **Feature 7: Rounded Job Card Styling (5 tests)**
  - `test_f07_job_card_corner_rounding_is_10pt`: Verifies 10.0pt rounding on all four corners.
  - `test_f07_job_card_drop_shadow_styling_applied`: Verifies drop shadow elevation styling.
  - `test_f07_job_card_layout_spacing_and_padding`: Verifies spacing tokens between job cards.
  - `test_f07_job_card_renders_title_and_status`: Verifies card title and status layout.
  - `test_f07_job_card_elevation_visual_hierarchy`: Verifies shadow alpha and elevation.
- **Feature 8: Phase 1: Probing Progress Bar (5 tests)**
  - `test_f08_probing_bar_color_is_blue_007aff`: Verifies `#007AFF` probing bar color.
  - `test_f08_probing_bar_displays_extracting_metadata`: Verifies `"Extracting Metadata..."` string.
  - `test_f08_probing_bar_pulsing_alpha_modulates_over_time`: Verifies sinusoidal alpha modulation.
  - `test_f08_probing_bar_ignores_speed_eta_metrics`: Verifies speed/ETA suppression during probing.
  - `test_f08_probing_bar_renders_indeterminate_indicator`: Verifies indeterminate progress bar widget.
- **Feature 9: Phase 2: Downloading Metrics Bar (5 tests)**
  - `test_f09_downloading_bar_color_is_solid_blue_007aff`: Verifies `#007AFF` solid downloading bar color.
  - `test_f09_downloading_bar_formats_metrics_line`: Verifies `"Downloading... <dl> / <tot> • <spd> • <eta> left"`.
  - `test_f09_downloading_bar_calculates_fractional_progress`: Verifies fractional progress calculation.
  - `test_f09_downloading_bar_formats_human_speed`: Verifies speed formatting (`MB/s`, `KB/s`, `B/s`).
  - `test_f09_downloading_bar_formats_human_eta`: Verifies ETA formatting (`h`, `m`, `s`).
- **Feature 10: Phase 3: Muxing Progress Bar (5 tests)**
  - `test_f10_muxing_bar_color_is_purple_af52de`: Verifies `#AF52DE` purple muxing bar color.
  - `test_f10_muxing_bar_displays_merging_audio_and_video`: Verifies `"Merging Audio & Video..."` string.
  - `test_f10_muxing_bar_pulsing_animation_cycle`: Verifies pulsing muxing bar animation widget.
  - `test_f10_muxing_bar_progress_indicates_finalizing`: Verifies finalizing state representation.
  - `test_f10_muxing_bar_suppresses_download_speeds`: Verifies speed metric suppression during muxing.
- **Feature 11: Phase 4: Completed State Indicator (5 tests)**
  - `test_f11_completed_bar_color_is_green_34c759`: Verifies `#34C759` solid green completed bar color.
  - `test_f11_completed_bar_displays_completed_text`: Verifies `"Completed."` string.
  - `test_f11_completed_bar_progress_is_100_percent`: Verifies 100% completion progress value.
  - `test_f11_completed_card_renders_finder_reveal_button`: Verifies `"Show in Finder"` button label.
  - `test_f11_completed_card_finder_button_triggers_callback`: Verifies Finder reveal callback invocation.
- **Feature 12: Plain-Language Error Masking (5 tests)**
  - `test_f12_error_masking_suppresses_raw_exit_codes`: Verifies raw exit codes are stripped.
  - `test_f12_error_masking_suppresses_python_tracebacks`: Verifies Python tracebacks are hidden.
  - `test_f12_error_masking_maps_network_error_to_plain_language`: Verifies friendly network message.
  - `test_f12_error_masking_maps_unsupported_url_to_plain_language`: Verifies friendly unsupported URL message.
  - `test_f12_error_masking_sanitizes_multiline_stderr`: Verifies single-line clean error output.
- **Feature 13: Auth Required Amber Recovery Card (5 tests)**
  - `test_f13_auth_error_renders_amber_card_ff9500`: Verifies `#FF9500` amber theme color.
  - `test_f13_auth_error_displays_login_required_message`: Verifies `"Login required for this platform"`.
  - `test_f13_auth_error_hides_raw_auth_headers_and_cookies`: Verifies cookies/headers are sanitized.
  - `test_f13_auth_error_card_presents_recovery_actions`: Verifies recovery action button presence.
  - `test_f13_auth_error_card_renders_retry_button`: Verifies retry button renders.
- **Feature 14: Auth Settings Direct Shortcut (5 tests)**
  - `test_f14_auth_shortcut_renders_connect_browser_cookies_button`: Verifies `"Connect Browser Cookies"` text.
  - `test_f14_auth_shortcut_triggers_on_open_cookies_callback`: Verifies direct settings navigation callback.
  - `test_f14_auth_shortcut_button_styling_matches_hig`: Verifies button visual styling.
  - `test_f14_auth_shortcut_supports_all_target_browsers`: Verifies Chrome, Safari, Firefox, Arc, Edge support.
  - `test_f14_auth_shortcut_does_not_mutate_job_state_on_nav`: Verifies non-destructive navigation.
- **Feature 15: Clipboard Auto-Detect Toast (5 tests)**
  - `test_f15_clipboard_toast_triggers_on_supported_url`: Verifies toast triggered when URL copied.
  - `test_f15_clipboard_toast_renders_exact_required_text`: Verifies `"Clipboard link detected. Press Return to add."`.
  - `test_f15_clipboard_toast_positioned_at_bottom_right`: Verifies `Align2::RIGHT_BOTTOM` positioning.
  - `test_f15_clipboard_toast_deduplicates_repeated_clipboard_events`: Verifies duplicate URL toast suppression.
  - `test_f15_clipboard_toast_ignores_unsupported_urls_and_text`: Verifies non-URL text does not trigger toast.
- **Feature 16: Keyboard Return Shortcut for Toast (5 tests)**
  - `test_f16_keyboard_return_stages_toast_url`: Verifies Return key stages detected URL.
  - `test_f16_keyboard_return_ignored_if_text_input_focused`: Verifies Return key ignored if typing in text input.
  - `test_f16_keyboard_return_clears_active_clipboard_toast`: Verifies staging clears clipboard toast.
  - `test_f16_keyboard_return_supports_both_return_and_keypad_enter`: Verifies Return/Enter key support.
  - `test_f16_keyboard_other_keys_do_not_stage_url`: Verifies non-Return keys do not stage.
- **Feature 17: History "Clear Completed" 5s Undo (5 tests)**
  - `test_f17_clear_completed_shows_undo_toast`: Verifies 5-second undo toast appears on clear.
  - `test_f17_undo_toast_persists_for_5_seconds`: Verifies countdown timer remains active at 2.0s.
  - `test_f17_clicking_undo_restores_all_cleared_entries`: Verifies all cleared entries restored on undo.
  - `test_f17_undo_toast_auto_expires_after_5_seconds`: Verifies expiration after 5.0 seconds.
  - `test_f17_expired_undo_toast_cannot_restore_entries`: Verifies expired undo yields empty restoration.
- **Feature 18: Zero Unwrap/Expect & Clean Build (5 tests)**
  - `test_f18_gui_source_has_zero_unwrap_calls`: Scans `crates/gui/src/` for `.unwrap()`.
  - `test_f18_gui_source_has_zero_expect_calls`: Scans `crates/gui/src/` for `.expect()`.
  - `test_f18_gui_error_handling_uses_pattern_matching_or_question_mark`: Verifies defensive error patterns.
  - `test_f18_gui_code_contains_no_todo_or_unimplemented_macros`: Verifies no unfinished macros in source.
  - `test_f18_gui_build_and_clippy_readiness_audit`: Verifies clean compilation audit readiness.

#### Tier 2: Boundary & Corner Cases (25 tests)
- **Boundary 1: Disk Space Boundary Cases (5 tests)**
  - `test_b01_disk_check_exact_byte_match`: Available == Required -> Sufficient.
  - `test_b01_disk_check_off_by_one_insufficient`: Available == Required - 1 -> Insufficient.
  - `test_b01_disk_check_zero_required_bytes`: 0 required bytes -> Sufficient.
  - `test_b01_disk_check_extreme_required_bytes_overflow_prevention`: 100 TB file size handled without overflow.
  - `test_b01_disk_check_zero_available_bytes`: 0 available bytes -> Insufficient.
- **Boundary 2: Progress Metrics Boundaries (5 tests)**
  - `test_b02_progress_metrics_zero_downloaded_and_total`: 0 B / 0 B formatting.
  - `test_b02_progress_metrics_none_total_bytes_no_division_by_zero`: Unknown total bytes format.
  - `test_b02_progress_metrics_downloaded_exceeds_total_clamped`: Clamping progress to 100%.
  - `test_b02_progress_metrics_zero_speed_displays_dashes`: 0.0 B/s speed displays `-- B/s`.
  - `test_b02_progress_metrics_extreme_eta_seconds_formatting`: 500h ETA formatting.
- **Boundary 3: Toast Timer Boundaries (5 tests)**
  - `test_b03_toast_timer_subsecond_precision_before_expiration`: 4.999s elapsed -> Undo still valid.
  - `test_b03_toast_timer_expiration_at_5001_seconds`: 5.001s elapsed -> Undo expired.
  - `test_b03_toast_rapid_consecutive_clears_resets_timer`: Consecutive clear triggers reset timer to 5s.
  - `test_b03_toast_empty_cleared_list_does_not_trigger_toast`: Empty history clear no-ops.
  - `test_b03_toast_multiple_undo_calls_idempotent`: Repeated undo calls idempotent.
- **Boundary 4: URL Staging & Parsing Boundaries (5 tests)**
  - `test_b04_url_empty_string_rejected`: Empty URL string rejected safely.
  - `test_b04_url_excessive_length_4096_bytes`: 4096-char long URL processed without crash.
  - `test_b04_url_with_shell_metacharacters_sanitized`: Shell metacharacters detection.
  - `test_b04_url_with_unicode_and_emojis`: Unicode emojis handled gracefully.
  - `test_b04_url_unsupported_protocol_schemes`: File, gopher, ftp protocols rejected.
- **Boundary 5: Layout & High-DPI Boundaries (5 tests)**
  - `test_b05_minimal_viewport_dimensions`: 100x100 tiny viewport layout sanity.
  - `test_b05_extreme_wide_viewport_dimensions`: 4000x2000 ultrawide viewport traffic light clearance.
  - `test_b05_retina_scale_factor_2x_and_3x`: Retina scaling factors calculated accurately.
  - `test_b05_long_video_title_truncation_does_not_overflow`: 500-char title truncation inside fixed width.
  - `test_b05_massive_error_message_wrapping_without_window_expansion`: Massive error sanitization.

#### Tier 3: Cross-Feature Combinations (10 tests)
- `test_t3_clipboard_to_staging_via_return_key`: Clipboard toast detected -> Return pressed -> URL staged in Probing state.
- `test_t3_staging_to_insufficient_disk_blocks_queue`: Staging card detects low disk space -> Download button disabled.
- `test_t3_staging_to_sufficient_disk_enqueues_job`: Staging card validates disk space -> Download button clicked -> Queues job with DASH format.
- `test_t3_job_lifecycle_probing_to_downloading_to_muxing_to_completed`: Full progress lifecycle across all 4 visual states and colors.
- `test_t3_auth_failure_to_browser_cookie_settings_navigation`: Auth failure turns card amber -> `[ Connect Browser Cookies ]` opens auth settings.
- `test_t3_auth_failure_retry_restarts_probing`: `[ Retry ]` button re-enqueues job into Probing state.
- `test_t3_batch_downloads_to_clear_history_to_undo`: Multiple completed jobs cleared from history -> Restored via 5s undo.
- `test_t3_batch_downloads_to_clear_history_to_timeout`: History cleared -> 5.1s elapsed -> Undo buffer permanently purged.
- `test_t3_system_font_fallback_with_vibrancy_palette`: Font definition fallback coupled with dark/light mode palette rendering.
- `test_t3_error_masking_during_network_interruption`: Low-level socket reset masked to clean plain-language error.

#### Tier 4: Real-World Application Scenarios (5 tests)
- `test_t4_scenario_youtube_1080p_dash_happy_path`: End-to-end YouTube DASH video workflow from clipboard to completion.
- `test_t4_scenario_low_disk_space_guardrail`: Low disk space protection prevents disk exhaustion on 4K stream; format selector recovers.
- `test_t4_scenario_instagram_private_reel_auth_recovery`: Private Instagram reel auth failure with amber recovery card and cookie workflow.
- `test_t4_scenario_rapid_clipboard_keyboard_batch_ingestion`: Rapid keyboard-driven batch URL ingestion with typing focus safety.
- `test_t4_scenario_accidental_history_clear_and_undo_recovery`: Accidental history clear with 100% data recovery via Nielsen Heuristic #3 Undo control.
