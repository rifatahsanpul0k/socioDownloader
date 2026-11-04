# Project: macOS Media Downloader GUI Rewrite (Heuristic UI Overhaul)

## Architecture
The application is a native macOS media downloader desktop GUI built with `eframe` and `egui` (v0.29.1) interfacing with `rust-dl-core` via asynchronous tokio runtime and channels (`std::sync::mpsc`).
To strictly adhere to Apple HIG and Jakob Nielsen's 10 Usability Heuristics while ensuring zero `.unwrap()` / `.expect()` calls, `crates/gui` is decomposed into modular components:
- `theme.rs`: Native window shell options (transparent titlebar, Cocoa traffic lights clearance), frosted glass vibrancy (via `window-vibrancy 0.5.2` and OpenGL clear color transparency), and SF Pro system font loading from `/System/Library/Fonts/` with defensive fallback.
- `staging.rs`: Pre-queue Staging Card state machine (Idle -> Probing -> Staged -> Disk Checked), async metadata probing, thumbnail texture fetching, format selector with badges (`Highest Quality (1080p Video + High-Res Audio)` & `🎬 H.264 + 🎵 AAC`), and pre-download disk space checks via `libc::statvfs` / `fs2`.
- `queue_view.rs`: Job cards with 10pt rounded corners and drop shadows, 4-phase dynamic progress bars (Probing pulsing blue, Downloading solid blue with metrics, Muxing pulsing purple, Completed solid green), plain-language error masking, and amber recovery cards for `CoreError::AuthenticationRequired` with `[ Connect Browser Cookies ]` and `[ Retry ]` actions.
- `toast.rs`: Custom bottom-right sliding notification overlay (`egui::Area` at `Align2::RIGHT_BOTTOM`) handling clipboard link auto-detection toasts with Return/Enter key trigger, and history "Clear Completed" 5-second countdown toasts with functional `[ Undo ]` restoration.
- `main.rs`: Application entry point, runtime bundle initialization, event dispatch, and UI panel composition.

## Feature Inventory
| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | macOS Transparent Titlebar & Shell | Native window frame with transparent titlebar, fullsize content view, hidden system title, traffic light clearance header | M1 | Survey / R1 |
| 2 | Frosted Glass Vibrancy & Native Palette | Window background vibrancy via `window-vibrancy 0.5.2` and clear_color [0,0,0,0], fallback to macOS dark (#1e1e1e) / light (#f6f6f6) | M1 | Survey / R1 |
| 3 | SF Pro System Font Integration | Load San Francisco Pro from `/System/Library/Fonts/` (`SFNS.ttf` / `SF-Pro.ttf`) into `FontDefinitions`, fallback to defaults | M1 | Survey / R1 |
| 4 | Pre-Queue Staging Card | Rich preview card (thumbnail, title, uploader, duration, total size) before queueing | M2 | Survey / R2 |
| 5 | Merged DASH Output Badges | Primary badge `Highest Quality (1080p Video + High-Res Audio)` & secondary badge `🎬 H.264 + 🎵 AAC` for merged streams | M2 | Survey / R2 |
| 6 | Pre-Download Disk Space Check | Query filesystem available bytes on target partition against stream filesize; disable button + amber warning if insufficient | M2 | Survey / R2 |
| 7 | Rounded Job Card Styling | Rounded corners (10pt radius) and subtle drop shadow elevation | M3 | Survey / R3 |
| 8 | Phase 1: Probing Progress Bar | Pulsing blue bar (`#007AFF`) displaying "Extracting Metadata..." | M3 | Survey / R3 |
| 9 | Phase 2: Downloading Metrics Bar | Solid blue bar displaying `"Downloading... <downloaded> / <total> • <speed> • <ETA> left"` | M3 | Survey / R3 |
| 10 | Phase 3: Muxing Progress Bar | Pulsing purple bar (`#AF52DE`) displaying `"Merging Audio & Video..."` | M3 | Survey / R3 |
| 11 | Phase 4: Completed State Indicator | Solid green bar (`#34C759`) displaying `"Completed."` + Finder reveal button | M3 | Survey / R3 |
| 12 | Plain-Language Error Masking | Suppress raw exit codes, stderr, and stack traces from UI | M3 | Survey / R4 |
| 13 | Auth Required Amber Recovery Card | Amber-themed card for `CoreError::AuthenticationRequired` with "Login required for this platform" and recovery actions | M3 | Survey / R4 |
| 14 | Auth Settings Direct Shortcut | `[ Connect Browser Cookies ]` button navigating directly to cookie source settings | M3 | Survey / R4 |
| 15 | Clipboard Auto-Detect Toast | Bottom-right toast alert: "Clipboard link detected. Press Return to add." | M4 | Survey / R5 |
| 16 | Keyboard Return Shortcut for Toast | Pressing Return/Enter when clipboard toast is active stages the URL | M4 | Survey / R5 |
| 17 | History "Clear Completed" 5s Undo | Bottom-right toast with 5s countdown and working `[ Undo ]` button restoring cleared entries | M4 | Survey / R5 |
| 18 | Zero Unwrap/Expect & Clean Build | Zero `.unwrap()` / `.expect()` in `crates/gui`, clean clippy, clean release build | M5 | Survey / R6 |

## Milestones
| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| M1 | Window Shell, Vibrancy & Typography | R1: `theme.rs`, Cargo dependencies (`window-vibrancy`), native titlebar, vibrancy, SF Pro font loading | none | DONE (theme.rs, main.rs, 158 tests pass, 0 warnings) |
| M2 | Staging Card & Error Prevention | R2: `staging.rs`, async metadata probe, thumbnail rendering, DASH badges, disk space pre-check | M1 | DONE (staging.rs, main.rs, 275 tests pass, 0 clippy warnings) |
| M3 | Visual Progress & Error Recovery | R3, R4: `queue_view.rs`, 4-phase progress bars, card styling, plain errors, amber auth card | M1 | IN_PROGRESS |
| M4 | Toasts & Undo Controls | R5: `toast.rs`, custom bottom-right toast panel, clipboard toast + Return shortcut, 5s history undo | M1 | PLANNED |
| M5 | Final Milestone: E2E Test Pass & Adversarial Hardening | R6: Full E2E test suite pass (Tiers 1-4) + adversarial coverage hardening (Tier 5), 0 unwraps, clean clippy & release build | M1, M2, M3, M4, TEST_READY | PLANNED |

## Parallel Track: E2E Testing Track
| # | Name | Scope | Status |
|---|------|-------|--------|
| E2E | Opaque-Box E2E Test Suite | Test runner, Tier 1-4 test suites covering all 18 inventory items, publishes `TEST_INFRA.md` & `TEST_READY.md` | DONE (TEST_READY.md published, 130 tests passing) |

## Interface Contracts

### `theme.rs` ↔ `main.rs`
- `setup_system_fonts(ctx: &egui::Context)`: Loads SF Pro/SFNS into `FontDefinitions`, falls back to defaults.
- `apply_macos_vibrancy(cc: &eframe::CreationContext)`: Applies Cocoa vibrancy if on macOS, no-op fallback.
- `render_macos_top_header(ui: &mut egui::Ui, title: &str) -> egui::Response`: Renders 34pt draggable top header with 78pt left insets to clear traffic lights.
- `theme_clear_color() -> [f32; 4]`: Returns `[0.0, 0.0, 0.0, 0.0]` for transparent clear color.

### `staging.rs` ↔ `main.rs`
- `pub enum StagingState`: `Idle`, `Probing { url, started_at }`, `Staged { url, manifest, thumbnail, selected_format_id, disk_check }`, `Error { url, error }`.
- `pub struct DiskCheckResult`: `required_bytes: u64`, `available_bytes: u64`, `is_sufficient: bool`.
- `pub fn check_disk_space(dir: &Path, required_bytes: Option<u64>) -> DiskCheckResult`.
- `pub fn format_stream_badges(stream: &MediaStream) -> (String, String)`: Returns primary and secondary badge texts.
- `pub fn render_staging_card(ui: &mut egui::Ui, staging: &mut StagingState, on_download: impl FnOnce(String, String))` -> Renders preview, format dropdown, disk check, and download/cancel buttons.

### `queue_view.rs` ↔ `main.rs`
- `pub fn render_queue_card(ui: &mut egui::Ui, job: &QueueJob, on_retry: impl FnOnce(JobId), on_cancel: impl FnOnce(JobId), on_open_cookies: impl FnOnce())`.
- Handles `JobStatus::Resolving` / `Downloading { progress }` / `Completed` / `Failed { error }`.
- Dynamic color & text by `progress.phase`: Probing (pulsing blue), Downloading (solid blue + metrics), Muxing (pulsing purple), Completed (solid green).
- Detects `AuthenticationRequired` -> amber card with `[ Connect Browser Cookies ]` and `[ Retry ]`.

### `toast.rs` ↔ `main.rs`
- `pub struct ToastManager`: `show(ctx, on_undo, on_add_clipboard)`.
- `pub fn trigger_clipboard_toast(&mut self, url: String)`.
- `pub fn trigger_clear_history_toast(&mut self, cleared_items: Vec<HistoryEntry>)`.
- Intercepts Return/Enter key if clipboard toast is active and text input does not have focus.

## Code Layout
```
crates/gui/
├── Cargo.toml          # eframe, egui, window-vibrancy, reqwest, image, libc
└── src/
    ├── main.rs         # Entry point, runtime bundle, event loop, view integration
    ├── theme.rs        # macOS window options, vibrancy, SF Pro font loading, styling tokens
    ├── staging.rs      # Staging Card state machine, thumbnail decode, badges, disk space checks
    ├── queue_view.rs   # QueueJob card styling, 4-phase progress bars, amber auth recovery
    └── toast.rs        # Bottom-right toast panel, clipboard toast, 5s history undo
```
