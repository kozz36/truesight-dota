use super::RgbFrame;
#[cfg(target_os = "windows")]
use super::ScreenFrameSource;
#[cfg(target_os = "windows")]
use truesight_core::ports::PortError;
#[cfg(target_os = "windows")]
use xcap::Window;

fn is_dota2_title(title: &str) -> bool {
    let title_norm = title.trim();
    title_norm.eq_ignore_ascii_case("dota 2") || title_norm.eq_ignore_ascii_case("dota2")
}

/// Validates Windows window metadata and verified PID executable identity so minimized windows,
/// unverified/elevated lookups (`None`), and third-party processes titled `"Dota 2"` are rejected.
pub fn is_dota2_windows_window(
    title: &str,
    owner_exe: Option<&str>,
    minimized: bool,
    width: u32,
    height: u32,
) -> bool {
    if minimized || width < 640 || height < 360 || !is_dota2_title(title) {
        return false;
    }
    let Some(raw_exe) = owner_exe else {
        return false;
    };
    let basename = raw_exe.trim().rsplit(['\\', '/']).next().unwrap_or("");
    basename.eq_ignore_ascii_case("dota2.exe")
}

/// Parses `tasklist /FI "PID eq <pid>" /FO CSV /NH` output and returns the verified executable
/// image name when a row matches `expected_pid`.
pub fn parse_tasklist_exe_for_pid(csv_output: &str, expected_pid: u32) -> Option<String> {
    let pid_str = expected_pid.to_string();
    for raw_line in csv_output.lines() {
        let line = raw_line.trim();
        let Some(after_open_quote) = line.strip_prefix('"') else {
            continue;
        };
        let Some((exe, rest)) = after_open_quote.split_once("\",\"") else {
            continue;
        };
        let Some((pid_col, _)) = rest.split_once('"') else {
            continue;
        };
        let exe = exe.trim();
        if !exe.is_empty() && pid_col.trim() == pid_str {
            return Some(exe.to_string());
        }
    }
    None
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

#[cfg(target_os = "windows")]
fn resolve_windows_pid_exe(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let mut cmd = std::process::Command::new("tasklist");
    crate::platform::configure_background_command(&mut cmd);
    let out = cmd
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_tasklist_exe_for_pid(&String::from_utf8_lossy(&out.stdout), pid)
}

/// Live Windows Win32/GDI top-bar frame source capturing the `"Dota 2"` window (`dota2.exe`) via
/// `xcap` (`PrintWindow` / GDI fallback, avoiding Windows Graphics Capture yellow privacy borders).
#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
pub struct WindowsTopBarFrameSource {
    cached_window: Option<(Window, u32)>,
}

#[cfg(target_os = "windows")]
impl WindowsTopBarFrameSource {
    pub fn new() -> Self {
        Self::default()
    }

    fn inspect_candidate(win: &Window) -> Option<(String, bool, u32, u32, u32)> {
        let minimized = win.is_minimized().ok()?;
        let title = win.title().ok()?;
        let width = win.width().ok()?;
        let height = win.height().ok()?;
        let pid = win.pid().ok()?;
        if minimized || width < 640 || height < 360 || !is_dota2_title(&title) {
            return None;
        }
        Some((title, minimized, width, height, pid))
    }

    fn verify_dota_window(win: &Window) -> Option<u32> {
        let (title, minimized, width, height, pid) = Self::inspect_candidate(win)?;
        let exe = resolve_windows_pid_exe(pid);
        is_dota2_windows_window(&title, exe.as_deref(), minimized, width, height).then_some(pid)
    }

    fn find_dota_window() -> Result<Option<(Window, u32)>, PortError> {
        let windows = Window::all()
            .map_err(|e| PortError(format!("Windows window enumeration failed: {e}")))?;
        for win in windows {
            if let Some(pid) = Self::verify_dota_window(&win) {
                return Ok(Some((win, pid)));
            }
        }
        Ok(None)
    }
}

#[cfg(target_os = "windows")]
impl ScreenFrameSource for WindowsTopBarFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
        if let Some((ref win, cached_pid)) = self.cached_window
            && Self::inspect_candidate(win).is_none_or(|(_, _, _, _, pid)| pid != cached_pid)
        {
            self.cached_window = None;
        }
        if self.cached_window.is_none() {
            self.cached_window = Self::find_dota_window()?;
        }
        let Some((ref win, cached_pid)) = self.cached_window else {
            return Ok(None);
        };
        let img = match win.capture_image() {
            Ok(img) => img,
            Err(err) => {
                // Closing or minimizing Dota 2 mid-poll transitions cleanly to WaitingForGame,
                // whereas failures on a still-open verified Dota 2 window surface as CaptureError.
                let still_visible =
                    Self::inspect_candidate(win).is_some_and(|(_, _, _, _, pid)| pid == cached_pid);
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
