use super::RgbFrame;
#[cfg(target_os = "windows")]
use super::ScreenFrameSource;
use std::path::PathBuf;
use truesight_core::ports::PortError;
#[cfg(target_os = "windows")]
use xcap::Window;

fn is_dota2_title(title: &str) -> bool {
    let title_norm = title.trim();
    title_norm.eq_ignore_ascii_case("dota 2") || title_norm.eq_ignore_ascii_case("dota2")
}

fn is_windows_abs_root(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let drive_rooted = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    drive_rooted || raw.starts_with(r"\\")
}

/// Resolves an absolute path inside Windows `System32` (`%SystemRoot%` -> `%WINDIR%` ->
/// `C:\Windows`) so `Command::new` never resolves helper executables from CWD or the app dir.
pub fn resolve_windows_system32_exe(
    system_root: Option<&str>,
    windir: Option<&str>,
    exe_name: &str,
) -> PathBuf {
    let root = [system_root, windir]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|cand| !cand.is_empty() && is_windows_abs_root(cand))
        .unwrap_or(r"C:\Windows");
    let mut path = PathBuf::from(root).join("System32");
    for segment in exe_name.split(['\\', '/']).filter(|s| !s.is_empty()) {
        path.push(segment);
    }
    path
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

/// Distinguishes a `tasklist.exe` process failure (`Err`) from a clean query where the PID
/// already exited (`Ok(None)`), preventing failed PID lookups from silently showing `WAIT`.
pub fn evaluate_pid_lookup_output(
    success: bool,
    status_code: Option<i32>,
    stdout: &str,
    expected_pid: u32,
) -> Result<Option<String>, PortError> {
    if !success {
        return Err(PortError(format!(
            "Windows process lookup failed (tasklist status {status_code:?})"
        )));
    }
    Ok(parse_tasklist_exe_for_pid(stdout, expected_pid))
}

/// Evaluates one Windows candidate window using an injected PID executable resolver, propagating
/// lookup failures while skipping minimized, non-matching, or exited targets cleanly.
pub fn verify_dota_candidate_with<F>(
    title: &str,
    pid: u32,
    minimized: bool,
    width: u32,
    height: u32,
    lookup_exe: F,
) -> Result<Option<u32>, PortError>
where
    F: FnOnce(u32) -> Result<Option<String>, PortError>,
{
    if minimized || width < 640 || height < 360 || !is_dota2_title(title) {
        return Ok(None);
    }
    if pid == 0 {
        return Err(PortError(
            "Windows process lookup failed: candidate window reported PID 0.".into(),
        ));
    }
    let exe = lookup_exe(pid)?;
    Ok(is_dota2_windows_window(title, exe.as_deref(), minimized, width, height).then_some(pid))
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
    for px in rgba[..strip_bytes].as_chunks::<4>().0 {
        rgb.push(px[0]);
        rgb.push(px[1]);
        rgb.push(px[2]);
    }
    Some(RgbFrame::with_source_height(width, strip_h, height, rgb))
}

#[cfg(target_os = "windows")]
fn resolve_windows_pid_exe(pid: u32) -> Result<Option<String>, PortError> {
    let sys_root = std::env::var("SystemRoot").ok();
    let windir = std::env::var("WINDIR").ok();
    let tasklist_path =
        resolve_windows_system32_exe(sys_root.as_deref(), windir.as_deref(), "tasklist.exe");
    let mut cmd = std::process::Command::new(&tasklist_path);
    crate::platform::configure_background_command(&mut cmd);
    let out = cmd
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .map_err(|e| {
            PortError(format!(
                "Failed to execute {} for PID lookup: {e}",
                tasklist_path.display()
            ))
        })?;
    evaluate_pid_lookup_output(
        out.status.success(),
        out.status.code(),
        &String::from_utf8_lossy(&out.stdout),
        pid,
    )
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

    fn verify_dota_window(win: &Window) -> Result<Option<u32>, PortError> {
        let Some((title, minimized, width, height, pid)) = Self::inspect_candidate(win) else {
            return Ok(None);
        };
        verify_dota_candidate_with(
            &title,
            pid,
            minimized,
            width,
            height,
            resolve_windows_pid_exe,
        )
    }

    fn find_dota_window() -> Result<Option<(Window, u32)>, PortError> {
        let windows = Window::all()
            .map_err(|e| PortError(format!("Windows window enumeration failed: {e}")))?;
        let mut first_lookup_err: Option<PortError> = None;
        for win in windows {
            match Self::verify_dota_window(&win) {
                Ok(Some(pid)) => return Ok(Some((win, pid))),
                Ok(None) => {}
                Err(err) => {
                    if first_lookup_err.is_none() {
                        first_lookup_err = Some(err);
                    }
                }
            }
        }
        if let Some(err) = first_lookup_err {
            return Err(err);
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
        let strip_rect = match (win.x().ok(), win.y().ok()) {
            (Some(wx), Some(wy)) => {
                super::compute_top_bar_capture_strip_rect(wx, wy, width as u32, height as u32)
            }
            _ => None,
        };
        rgba_to_topbar_rgb_frame(img.as_raw(), width, height)
            .map(|f| match strip_rect {
                Some(rect) => f.with_capture_strip_rect(rect),
                None => f,
            })
            .map(Some)
            .ok_or_else(|| {
                PortError("Windows capture returned an invalid RGBA frame buffer.".into())
            })
    }
}
