#[cfg(target_os = "linux")]
use super::{parse_ppm_p6, RgbFrame, ScreenFrameSource};
#[cfg(target_os = "linux")]
use std::process::Command;
#[cfg(target_os = "linux")]
use truesight_core::ports::PortError;

/// Validates `xprop` output so only visible Dota 2 (`WM_CLASS = "dota2"`) windows qualify.
pub fn is_dota2_xprop_output(props_str: &str) -> bool {
    if props_str.contains("_NET_WM_STATE_HIDDEN") {
        return false;
    }
    props_str.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("WM_CLASS") && trimmed.contains("\"dota2\"")
    })
}

/// Builds the ImageMagick `import` arguments for silent top-bar X11 capture.
pub fn build_x11_import_args(window_id: &str) -> [&str; 8] {
    [
        "-silent",
        "-window",
        window_id,
        "-depth",
        "8",
        "-crop",
        "100%x8%+0+0",
        "ppm:-",
    ]
}

/// Live Linux X11/XWayland top-bar frame source capturing the `"dota2"` window strip via `xprop` + `import`.
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct X11TopBarFrameSource {
    cached_window: Option<String>,
}

#[cfg(target_os = "linux")]
impl X11TopBarFrameSource {
    pub fn new() -> Self {
        Self::default()
    }

    fn find_dota_window() -> Option<String> {
        let root_out = Command::new("xprop")
            .args(["-root", "_NET_CLIENT_LIST"])
            .output()
            .ok()?;
        if !root_out.status.success() {
            return None;
        }
        let root_text = String::from_utf8_lossy(&root_out.stdout);
        for tok in root_text.split_whitespace() {
            let wid = tok.trim_end_matches(',');
            if !wid.starts_with("0x") {
                continue;
            }
            let props = Command::new("xprop")
                .args(["-id", wid, "WM_CLASS", "_NET_WM_STATE"])
                .output()
                .ok();
            let Some(props) = props else {
                continue;
            };
            let props_str = String::from_utf8_lossy(&props.stdout);
            if !is_dota2_xprop_output(&props_str) {
                continue;
            }
            return Some(wid.to_string());
        }
        None
    }
}

#[cfg(target_os = "linux")]
impl ScreenFrameSource for X11TopBarFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
        if self.cached_window.is_none() {
            self.cached_window = Self::find_dota_window();
        }
        let Some(ref wid) = self.cached_window else {
            return Ok(None);
        };

        let output = Command::new("import")
            .args(build_x11_import_args(wid))
            .output();

        let Ok(out) = output else {
            self.cached_window = None;
            return Ok(None);
        };
        if !out.status.success() {
            self.cached_window = None;
            return Ok(None);
        }

        let Some(frame) = parse_ppm_p6(&out.stdout) else {
            self.cached_window = None;
            return Ok(None);
        };
        if frame.width < 640 || frame.height < 40 {
            return Ok(None);
        }
        Ok(Some(frame))
    }
}
