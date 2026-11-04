#![allow(deprecated)]

use raw_window_handle::HasWindowHandle;

#[cfg(target_os = "macos")]
pub use objc2_app_kit::{NSVisualEffectMaterial, NSVisualEffectState};

#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NSVisualEffectMaterial {
    Titlebar,
    Selection,
    Menu,
    Popover,
    Sidebar,
    HeaderView,
    Sheet,
    WindowBackground,
    HUDWindow,
    FullScreenUI,
    ToolTip,
    ContentBackground,
    UnderWindowBackground,
    UnderPageBackground,
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NSVisualEffectState {
    FollowsWindowActiveState,
    Active,
    Inactive,
}

#[derive(Debug)]
pub enum Error {
    UnsupportedPlatform,
    WindowHandleError(raw_window_handle::HandleError),
    FailedToApply,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "Unsupported platform"),
            Self::WindowHandleError(e) => write!(f, "Window handle error: {e}"),
            Self::FailedToApply => write!(f, "Failed to apply vibrancy"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(target_os = "macos")]
pub fn apply_vibrancy(
    window: impl HasWindowHandle,
    material: NSVisualEffectMaterial,
    state: Option<NSVisualEffectState>,
    _radius: Option<f64>,
) -> Result<(), Error> {
    use objc2_app_kit::{NSVisualEffectBlendingMode, NSVisualEffectView};
    use objc2_foundation::MainThreadMarker;

    let handle = window.window_handle().map_err(Error::WindowHandleError)?;
    let raw = handle.as_raw();

    if let raw_window_handle::RawWindowHandle::AppKit(appkit_handle) = raw {
        let ns_view_ptr = appkit_handle.ns_view.as_ptr();
        unsafe {
            let mtm = MainThreadMarker::new_unchecked();
            let ns_view: &objc2_app_kit::NSView = &*(ns_view_ptr as *const objc2_app_kit::NSView);
            let effect_view = NSVisualEffectView::new(mtm);
            effect_view.setMaterial(material);
            if let Some(state) = state {
                effect_view.setState(state);
            }
            effect_view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            effect_view.setFrame(ns_view.bounds());
            ns_view.addSubview(&effect_view);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn apply_vibrancy(
    _window: impl HasWindowHandle,
    _material: NSVisualEffectMaterial,
    _state: Option<NSVisualEffectState>,
    _radius: Option<f64>,
) -> Result<(), Error> {
    Err(Error::UnsupportedPlatform)
}
