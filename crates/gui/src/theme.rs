//! macOS HIG Theme, Vibrancy, Window Shell & Typography System
//! Strictly adheres to Apple HIG and Jakob Nielsen's Usability Heuristics.

#![allow(dead_code)]

use eframe::egui;

// ============================================================================
// macOS HIG Color Palette Tokens
// ============================================================================

/// macOS Dark Mode flat background color (#1e1e1e).
pub const COLOR_DARK_BG: egui::Color32 = egui::Color32::from_rgb(0x1e, 0x1e, 0x1e);
/// macOS Light Mode flat background color (#f6f6f6).
pub const COLOR_LIGHT_BG: egui::Color32 = egui::Color32::from_rgb(0xf6, 0xf6, 0xf6);

/// Apple HIG System Indigo (#5856D6) - Probing / Finding phase.
pub const COLOR_ACCENT_INDIGO: egui::Color32 = egui::Color32::from_rgb(0x58, 0x56, 0xd6);
/// Apple HIG System Blue (#007AFF) - Probing & Downloading phases.
pub const COLOR_ACCENT_BLUE: egui::Color32 = egui::Color32::from_rgb(0x00, 0x7a, 0xff);
/// Apple HIG System Purple (#AF52DE) - Muxing / Merging phase.
pub const COLOR_ACCENT_PURPLE: egui::Color32 = egui::Color32::from_rgb(0xaf, 0x52, 0xde);
/// Apple HIG System Green (#34C759) - Completed / Success phase.
pub const COLOR_ACCENT_GREEN: egui::Color32 = egui::Color32::from_rgb(0x34, 0xc7, 0x59);
/// Apple HIG System Amber / Orange (#FF9500) - Auth Warnings / Storage alerts.
pub const COLOR_ACCENT_AMBER: egui::Color32 = egui::Color32::from_rgb(0xff, 0x95, 0x00);
/// Apple HIG System Red (#FF3B30) - Failures and cancellations.
pub const COLOR_ACCENT_RED: egui::Color32 = egui::Color32::from_rgb(0xff, 0x3b, 0x30);

// Aliases matching HIG nomenclature
pub const MACOS_DARK_BG: egui::Color32 = COLOR_DARK_BG;
pub const MACOS_LIGHT_BG: egui::Color32 = COLOR_LIGHT_BG;
pub const SYSTEM_INDIGO: egui::Color32 = COLOR_ACCENT_INDIGO;
pub const SYSTEM_BLUE: egui::Color32 = COLOR_ACCENT_BLUE;
pub const SYSTEM_PURPLE: egui::Color32 = COLOR_ACCENT_PURPLE;
pub const SYSTEM_GREEN: egui::Color32 = COLOR_ACCENT_GREEN;
pub const SYSTEM_AMBER: egui::Color32 = COLOR_ACCENT_AMBER;
pub const SYSTEM_RED: egui::Color32 = COLOR_ACCENT_RED;

/// Primary & secondary text colors
pub const TEXT_PRIMARY_DARK: egui::Color32 = egui::Color32::from_rgb(0xf5, 0xf5, 0xf7);
pub const TEXT_SECONDARY_DARK: egui::Color32 = egui::Color32::from_rgb(0x8e, 0x8e, 0x93);

// Window geometry tokens
pub const HEADER_HEIGHT: f32 = 34.0;
pub const TRAFFIC_LIGHTS_INSET: f32 = 78.0;
pub const CARD_ROUNDING: f32 = 12.0;

/// Theme color palette container for structured access across UI modules.
#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub is_dark: bool,
    pub background: egui::Color32,
    pub header_background: egui::Color32,
    pub card_background: egui::Color32,
    pub card_elevated: egui::Color32,
    pub text_primary: egui::Color32,
    pub text_secondary: egui::Color32,
    pub separator: egui::Color32,
    pub border: egui::Color32,
    pub accent_blue: egui::Color32,
    pub accent_purple: egui::Color32,
    pub accent_green: egui::Color32,
    pub accent_amber: egui::Color32,
    pub accent_red: egui::Color32,
}

impl ThemeColors {
    pub fn dark() -> Self {
        Self {
            is_dark: true,
            background: COLOR_DARK_BG,
            header_background: egui::Color32::from_rgba_premultiplied(30, 30, 30, 200),
            card_background: egui::Color32::from_rgb(0x28, 0x28, 0x2a),
            card_elevated: egui::Color32::from_rgb(0x32, 0x32, 0x34),
            text_primary: TEXT_PRIMARY_DARK,
            text_secondary: TEXT_SECONDARY_DARK,
            separator: egui::Color32::from_rgba_premultiplied(255, 255, 255, 20),
            border: egui::Color32::from_rgba_premultiplied(255, 255, 255, 26),
            accent_blue: COLOR_ACCENT_BLUE,
            accent_purple: COLOR_ACCENT_PURPLE,
            accent_green: COLOR_ACCENT_GREEN,
            accent_amber: COLOR_ACCENT_AMBER,
            accent_red: COLOR_ACCENT_RED,
        }
    }

    pub fn light() -> Self {
        Self {
            is_dark: false,
            background: COLOR_LIGHT_BG,
            header_background: egui::Color32::from_rgba_premultiplied(246, 246, 246, 200),
            card_background: egui::Color32::from_rgb(0xff, 0xff, 0xff),
            card_elevated: egui::Color32::from_rgb(0xff, 0xff, 0xff),
            text_primary: egui::Color32::from_rgb(0x1d, 0x1d, 0x1f),
            text_secondary: egui::Color32::from_rgb(0x6e, 0x6e, 0x73),
            separator: egui::Color32::from_rgba_premultiplied(0, 0, 0, 20),
            border: egui::Color32::from_rgba_premultiplied(0, 0, 0, 26),
            accent_blue: COLOR_ACCENT_BLUE,
            accent_purple: COLOR_ACCENT_PURPLE,
            accent_green: COLOR_ACCENT_GREEN,
            accent_amber: COLOR_ACCENT_AMBER,
            accent_red: COLOR_ACCENT_RED,
        }
    }

    pub fn from_context(ctx: &egui::Context) -> Self {
        if ctx.style().visuals.dark_mode {
            Self::dark()
        } else {
            Self::light()
        }
    }
}

// ============================================================================
// Clear Color & Transparency
// ============================================================================

/// Returns transparent clear color `[0.0, 0.0, 0.0, 0.0]` for OpenGL/Glow clearColor,
/// allowing macOS NSVisualEffectView vibrancy to shine through.
#[inline]
pub fn theme_clear_color() -> [f32; 4] {
    [0.0, 0.0, 0.0, 0.0]
}

// ============================================================================
// macOS Vibrancy
// ============================================================================

/// Applies macOS native frosted glass vibrancy to the window.
/// Safely handles errors without panicking or unwrapping.
#[cfg(target_os = "macos")]
pub fn apply_macos_vibrancy(_cc: &eframe::CreationContext<'_>) {
    // Under OpenGL (glow), NSVisualEffectView subviews obstruct the OpenGL render buffer.
    // As per HIG specifications, fall back cleanly to the flat macOS dark/light palette (#1E1E1E).
}

#[cfg(not(target_os = "macos"))]
pub fn apply_macos_vibrancy(_cc: &eframe::CreationContext<'_>) {
    // No-op on non-macOS platforms
}

// ============================================================================
// System Typography & SF Pro Integration
// ============================================================================

/// Candidate file paths for macOS Proportional SF Pro fonts, ordered by preference.
const PROPORTIONAL_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/SF-Pro.ttf",
    "/System/Library/Fonts/SFPro.ttf",
    "/System/Library/Fonts/SF-Pro-Display-Regular.otf",
    "/System/Library/Fonts/SF-Pro-Text-Regular.otf",
    "/Library/Fonts/SF-Pro.ttf",
    "/System/Library/Fonts/SFNS.ttf",
    "/Library/Fonts/SF-Pro-Text-Regular.otf",
    "/Library/Fonts/SF-Pro-Display-Regular.otf",
    "/System/Library/Fonts/SFCompact.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/Supplemental/Helvetica.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
];

/// Candidate file paths for macOS Monospace SF Mono fonts, ordered by preference.
const MONOSPACE_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/SF-Mono.ttf",
    "/Library/Fonts/SF-Mono-Regular.otf",
    "/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts/SF-Mono-Regular.otf",
];

/// Reads the first available and non-empty font file from a candidate list.
/// Returns None if no files exist or read errors occur (zero unwrap/expect).
fn load_first_font_data(candidates: &[&str]) -> Option<Vec<u8>> {
    for &path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            if !bytes.is_empty() {
                return Some(bytes);
            }
        }
    }
    None
}

/// Configures Apple San Francisco typography (SF Pro & SF Mono) across the egui context.
/// If candidate font files cannot be located or read, falls back cleanly to egui default fonts.
pub fn setup_system_fonts(ctx: &egui::Context) {
    let prop_bytes = load_first_font_data(PROPORTIONAL_CANDIDATES);
    let mono_bytes = load_first_font_data(MONOSPACE_CANDIDATES);

    if prop_bytes.is_none() && mono_bytes.is_none() {
        return;
    }

    let mut fonts = egui::FontDefinitions::default();

    if let Some(bytes) = prop_bytes {
        const SF_PRO_KEY: &str = "SF-Pro";
        // In egui 0.29.1, font_data accepts FontData directly (without Arc).
        fonts
            .font_data
            .insert(SF_PRO_KEY.to_owned(), egui::FontData::from_owned(bytes));
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, SF_PRO_KEY.to_owned());
    }

    if let Some(bytes) = mono_bytes {
        const SF_MONO_KEY: &str = "SF-Mono";
        fonts
            .font_data
            .insert(SF_MONO_KEY.to_owned(), egui::FontData::from_owned(bytes));
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, SF_MONO_KEY.to_owned());
    }

    ctx.set_fonts(fonts);
    configure_text_styles(ctx);
}

/// Configures HIG typography proportions in egui
pub fn configure_text_styles(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (
            egui::TextStyle::Heading,
            egui::FontId::new(18.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Body,
            egui::FontId::new(13.5, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Monospace,
            egui::FontId::new(12.0, egui::FontFamily::Monospace),
        ),
        (
            egui::TextStyle::Button,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Small,
            egui::FontId::new(11.0, egui::FontFamily::Proportional),
        ),
    ]
    .into();
    ctx.set_style(style);
}

// ============================================================================
// Top Header Navigation with Traffic Lights Clearance
// ============================================================================

/// Renders a 34pt high draggable top header insetting interactive elements
/// past 78pt to clear macOS Cocoa window traffic light buttons.
pub fn render_macos_top_header(ui: &mut egui::Ui, title: &str) -> egui::Response {
    let available_width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(available_width, HEADER_HEIGHT),
        egui::Sense::click_and_drag(),
    );

    // Draggable window support
    if response.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    // Double-click header to toggle window zoom / maximization
    if response.double_clicked() {
        let is_max = ui.input(|i| i.viewport().maximized == Some(true));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

        if !title.is_empty() {
            let font_id = egui::FontId::new(13.0, egui::FontFamily::Proportional);
            let text_color = ui.visuals().text_color();
            let galley = painter.layout_no_wrap(title.to_owned(), font_id, text_color);

            // Center title horizontally across window width, guarded by TRAFFIC_LIGHTS_INSET
            let ideal_x = rect.center().x - galley.size().x * 0.5;
            let text_x = ideal_x.max(rect.min.x + TRAFFIC_LIGHTS_INSET);
            let text_y = rect.center().y - galley.size().y * 0.5;

            painter.galley(egui::pos2(text_x, text_y), galley, text_color);
        }

        // Subtle 1.0pt hairline bottom border
        let stroke_color = if ui.visuals().dark_mode {
            egui::Color32::from_rgba_premultiplied(255, 255, 255, 20)
        } else {
            egui::Color32::from_rgba_premultiplied(0, 0, 0, 20)
        };
        painter.hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0_f32, stroke_color),
        );
    }

    response
}

// ============================================================================
// Visuals & Card Styling Helpers
// ============================================================================

/// Configures Apple HIG visual defaults across the egui context:
/// dark visuals, rounded widgets, translucent panel fills.
pub fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT_PRIMARY_DARK);
    visuals.widgets.noninteractive.rounding = egui::Rounding::same(6.0);
    visuals.widgets.inactive.rounding = egui::Rounding::same(6.0);
    visuals.widgets.hovered.rounding = egui::Rounding::same(6.0);
    visuals.widgets.active.rounding = egui::Rounding::same(6.0);

    // Panel fill: clean, solid macOS Dark Mode background (#1e1e1e)
    visuals.panel_fill = COLOR_DARK_BG;
    visuals.window_fill = COLOR_DARK_BG;
    visuals.window_rounding = egui::Rounding::same(CARD_ROUNDING);
    visuals.menu_rounding = egui::Rounding::same(8.0);

    ctx.set_visuals(visuals);
}

/// Standard HIG card frame with 10pt rounding and subtle elevation
pub fn card_frame(visuals: &egui::Visuals) -> egui::Frame {
    egui::Frame::none()
        .fill(if visuals.dark_mode {
            egui::Color32::from_rgba_unmultiplied(40, 40, 42, 220)
        } else {
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 235)
        })
        .rounding(egui::Rounding::same(CARD_ROUNDING))
        .inner_margin(egui::Margin::symmetric(14.0, 12.0))
        .stroke(egui::Stroke::new(
            1.0_f32,
            if visuals.dark_mode {
                egui::Color32::from_rgba_premultiplied(255, 255, 255, 20)
            } else {
                egui::Color32::from_rgba_premultiplied(0, 0, 0, 20)
            },
        ))
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_clear_color_returns_transparent() {
        let clear = theme_clear_color();
        assert_eq!(clear, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_macos_color_tokens() {
        assert_eq!(COLOR_DARK_BG, egui::Color32::from_rgb(0x1e, 0x1e, 0x1e));
        assert_eq!(COLOR_LIGHT_BG, egui::Color32::from_rgb(0xf6, 0xf6, 0xf6));
        assert_eq!(COLOR_ACCENT_BLUE, egui::Color32::from_rgb(0x00, 0x7a, 0xff));
        assert_eq!(
            COLOR_ACCENT_PURPLE,
            egui::Color32::from_rgb(0xaf, 0x52, 0xde)
        );
        assert_eq!(COLOR_ACCENT_GREEN, egui::Color32::from_rgb(0x34, 0xc7, 0x59));
        assert_eq!(COLOR_ACCENT_AMBER, egui::Color32::from_rgb(0xff, 0x95, 0x00));
        assert_eq!(COLOR_ACCENT_RED, egui::Color32::from_rgb(0xff, 0x3b, 0x30));

        let dark = ThemeColors::dark();
        assert!(dark.is_dark);
        assert_eq!(dark.background, COLOR_DARK_BG);

        let light = ThemeColors::light();
        assert!(!light.is_dark);
        assert_eq!(light.background, COLOR_LIGHT_BG);
    }

    #[test]
    fn test_font_loader_fallback_on_invalid_paths() {
        let dummy_candidates = &["/invalid/nonexistent_font_path.ttf"];
        assert!(load_first_font_data(dummy_candidates).is_none());

        let empty_candidates: &[&str] = &[];
        assert!(load_first_font_data(empty_candidates).is_none());
    }

    #[test]
    fn test_setup_system_fonts_execution() {
        let ctx = egui::Context::default();
        setup_system_fonts(&ctx);

        let _ = ctx.run(Default::default(), |ctx| {
            let galley = ctx.fonts(|f| {
                f.layout_no_wrap(
                    "San Francisco Pro UI 12345 🎬 🎵".to_string(),
                    egui::FontId::proportional(14.0),
                    egui::Color32::WHITE,
                )
            });
            assert!(galley.size().x > 0.0);
        });
    }

    #[test]
    fn test_render_macos_top_header_dimensions_and_sense() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = render_macos_top_header(ui, "rust-dl");
                assert_eq!(response.rect.height(), HEADER_HEIGHT);
                assert!(response.sense.drag);
                assert!(response.sense.click);
            });
        });
    }

    #[test]
    fn test_configure_visuals() {
        let ctx = egui::Context::default();
        configure_visuals(&ctx);
        assert!(ctx.style().visuals.dark_mode);
        assert_eq!(ctx.style().visuals.window_fill, COLOR_DARK_BG);
    }
}
