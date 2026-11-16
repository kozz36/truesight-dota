use super::RgbFrame;
#[cfg(target_os = "windows")]
use super::ScreenFrameSource;
#[cfg(target_os = "windows")]
use truesight_core::ports::PortError;
#[cfg(target_os = "windows")]
use xcap::Window;

/// Validates Windows window metadata so minimized/helper windows and browsers mentioning
/// `"Dota 2"` in a tab title are excluded.
pub fn is_dota2_windows_window(
    title: &str,
    app_name: &str,
    minimized: bool,
    width: u32,
    height: u32,
) -> bool {
    if minimized || width < 640 || height < 360 {
        return false;
    }
    let title_norm = title.trim();
    let title_matches =
        title_norm.eq_ignore_ascii_case("dota 2") || title_norm.eq_ignore_ascii_case("dota2");
    if !title_matches {
        return false;
    }
    let app_lower = app_name.trim().to_ascii_lowercase();
    let app_base = app_lower.strip_suffix(".exe").unwrap_or(&app_lower);
    // Empty `app_name` (e.g. restricted version-info read) falls back to exact title matching,
    // while non-empty executable/product names must belong to Dota 2.
    app_base.is_empty() || app_base == "dota2" || app_base == "dota 2"
}

/// Extracts the top 8% Dota 2 hero top-bar strip from a full-window RGBA buffer (`width x height`),
/// matching X11 capture geometry while avoiding full-frame RGB copies.
pub fn rgba_to_topbar_rgb_frame(rgba: &[u8], width: usize, height: usize) -> Option<RgbFrame> {
    if width < 640 || height < 360 {
        return None;
    }
    let full_bytes = width.checked_mul(height)?.checked_mul(4)?;
    if rgba.len() < full_bytes {
        return None;
    }
    let strip_h = ((height * 8).div_ceil(100)).max(60).min(height);
    if strip_h < 40 {
        return None;
    }
    let strip_bytes = width.checked_mul(strip_h)?.checked_mul(4)?;
    let mut rgb = Vec::with_capacity(width * strip_h * 3);
    for px in rgba[..strip_bytes].chunks_exact(4) {
        rgb.push(px[0]);
        rgb.push(px[1]);
        rgb.push(px[2]);
    }
    Some(RgbFrame::with_source_height(width, strip_h, height, rgb))
}

/// Live Windows Win32/GDI top-bar frame source capturing the `"Dota 2"` window via `xcap`
/// (`PrintWindow` / GDI fallback, avoiding Windows Graphics Capture yellow privacy borders).
#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
pub struct WindowsTopBarFrameSource {
    cached_window: Option<Window>,
}

#[cfg(target_os = "windows")]
impl WindowsTopBarFrameSource {
    pub fn new() -> Self {
        Self::default()
    }

    fn matches_dota_window(win: &Window) -> bool {
        let Ok(minimized) = win.is_minimized() else {
            return false;
        };
        let Ok(title) = win.title() else {
            return false;
        };
        let app_name = win.app_name().unwrap_or_default();
        let Ok(width) = win.width() else {
            return false;
        };
        let Ok(height) = win.height() else {
            return false;
        };
        is_dota2_windows_window(&title, &app_name, minimized, width, height)
    }

    fn find_dota_window() -> Result<Option<Window>, PortError> {
        let windows = Window::all()
            .map_err(|e| PortError(format!("Windows window enumeration failed: {e}")))?;
        Ok(windows.into_iter().find(Self::matches_dota_window))
    }
}

#[cfg(target_os = "windows")]
impl ScreenFrameSource for WindowsTopBarFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
        if let Some(ref win) = self.cached_window
            && !Self::matches_dota_window(win)
        {
            self.cached_window = None;
        }
        if self.cached_window.is_none() {
            self.cached_window = Self::find_dota_window()?;
        }
        let Some(ref win) = self.cached_window else {
            return Ok(None);
        };
        let img = match win.capture_image() {
            Ok(img) => img,
            Err(err) => {
                // Closing or minimizing Dota 2 mid-poll transitions to WaitingForGame (`Ok(None)`),
                // whereas capture failures on an open visible window surface as `CaptureError`.
                let still_visible = Self::matches_dota_window(win);
                self.cached_window = None;
                if !still_visible {
                    return Ok(None);
                }
                return Err(PortError(format!("Windows capture failed: {err}")));
            }
        };
        let width = img.width() as usize;
        let height = img.height() as usize;
        if width < 640 || height < 360 {
            self.cached_window = None;
            return Ok(None);
        }
        rgba_to_topbar_rgb_frame(img.as_raw(), width, height)
            .map(Some)
            .ok_or_else(|| {
                PortError("Windows capture returned an invalid RGBA frame buffer.".into())
            })
    }
}
