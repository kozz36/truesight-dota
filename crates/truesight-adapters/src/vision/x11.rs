#[cfg(target_os = "linux")]
use super::{RgbFrame, ScreenFrameSource, parse_ppm_p6};
#[cfg(target_os = "linux")]
use std::process::Command;
#[cfg(target_os = "linux")]
use truesight_core::ports::PortError;

fn has_dota2_wm_class(props_str: &str) -> bool {
    props_str.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("WM_CLASS") && trimmed.contains("\"dota2\"")
    })
}

fn is_window_hidden_or_iconic(props_str: &str) -> bool {
    if props_str.contains("_NET_WM_STATE_HIDDEN") {
        return true;
    }
    props_str.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("window state:")
            && (trimmed.contains("Iconic") || trimmed.contains("Withdrawn"))
    })
}

/// Validates `xprop` output so only visible Dota 2 (`WM_CLASS = "dota2"`) windows qualify.
pub fn is_dota2_xprop_output(props_str: &str) -> bool {
    if is_window_hidden_or_iconic(props_str) {
        return false;
    }
    has_dota2_wm_class(props_str)
}

#[cfg(any(target_os = "linux", test))]
fn is_truesight_window(props_str: &str) -> bool {
    props_str.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("WM_CLASS")
            && trimmed.to_ascii_lowercase().contains("\"truesight-dota\"")
    })
}

#[cfg(any(target_os = "linux", test))]
fn parse_xprop_window_id_u64(tok: &str) -> Option<u64> {
    let clean = tok.trim().trim_end_matches(',');
    let hex = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))?;
    u64::from_str_radix(hex, 16).ok().filter(|&v| v != 0)
}

#[cfg(any(target_os = "linux", test))]
fn parse_xprop_stacking_ids(root_props: &str) -> Vec<(u64, String)> {
    let mut ids = Vec::new();
    for line in root_props.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("_NET_CLIENT_LIST_STACKING") {
            continue;
        }
        let tail = trimmed
            .split_once('#')
            .or_else(|| trimmed.split_once('='))
            .map(|(_, rhs)| rhs)
            .unwrap_or("");
        for tok in tail.split(|c: char| c.is_whitespace() || c == ',') {
            if let Some(num) = parse_xprop_window_id_u64(tok) {
                ids.push((num, tok.to_string()));
            }
        }
    }
    ids
}

#[cfg(any(target_os = "linux", test))]
fn parse_xprop_cardinal(props_str: &str, atom: &str) -> Option<u32> {
    for line in props_str.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with(atom) {
            continue;
        }
        let (_, rhs) = trimmed.split_once('=')?;
        let tok = rhs.split([',', ' ']).find(|s| !s.is_empty())?;
        if let Some(hex) = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")) {
            return u32::from_str_radix(hex, 16).ok();
        }
        return tok.parse::<u32>().ok();
    }
    None
}

#[cfg(any(target_os = "linux", test))]
fn parse_xprop_desktop_geometry(props_str: &str) -> Option<(usize, usize)> {
    for line in props_str.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("_NET_DESKTOP_GEOMETRY") {
            continue;
        }
        let (_, rhs) = trimmed.split_once('=')?;
        let mut parts = rhs
            .split(',')
            .map(str::trim)
            .filter_map(|s| s.parse::<usize>().ok());
        let w = parts.next()?;
        let h = parts.next()?;
        return Some((w, h));
    }
    None
}

/// Evaluates whether a cached Dota 2 X11 window remains visible, on the active desktop, and
/// unobstructed by other foreground windows on single-monitor setups (preserving multi-monitor).
#[cfg(any(target_os = "linux", test))]
pub(crate) fn evaluate_x11_window_visibility_with<F>(
    wid: &str,
    root_props: &str,
    dota_props: &str,
    last_frame_width: Option<usize>,
    mut fetch_window_props: F,
) -> bool
where
    F: FnMut(&str) -> Option<String>,
{
    if !is_dota2_xprop_output(dota_props) {
        return false;
    }

    let cur_desktop = parse_xprop_cardinal(root_props, "_NET_CURRENT_DESKTOP");
    let dota_desktop = parse_xprop_cardinal(dota_props, "_NET_WM_DESKTOP");
    if let (Some(cur), Some(desk)) = (cur_desktop, dota_desktop)
        && desk != u32::MAX
        && desk != cur
    {
        return false;
    }

    if let (Some(fw), Some((desk_w, _)), Some(dota_id)) = (
        last_frame_width,
        parse_xprop_desktop_geometry(root_props),
        parse_xprop_window_id_u64(wid),
    ) && desk_w <= fw
    {
        let stack = parse_xprop_stacking_ids(root_props);
        if let Some(pos) = stack.iter().position(|(id, _)| *id == dota_id) {
            for (_, other_wid) in &stack[pos + 1..] {
                let Some(other_props) = fetch_window_props(other_wid) else {
                    continue;
                };
                if is_window_hidden_or_iconic(&other_props) || is_truesight_window(&other_props) {
                    continue;
                }
                if let (Some(cur), Some(other_desk)) = (
                    cur_desktop,
                    parse_xprop_cardinal(&other_props, "_NET_WM_DESKTOP"),
                ) && other_desk != u32::MAX
                    && other_desk != cur
                {
                    continue;
                }
                return false;
            }
        }
    }

    true
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
    last_frame_width: Option<usize>,
}

#[cfg(target_os = "linux")]
impl X11TopBarFrameSource {
    pub fn new() -> Self {
        Self::default()
    }

    fn query_root_visibility_props() -> Result<String, PortError> {
        let root_out = Command::new("xprop")
            .args([
                "-root",
                "_NET_CURRENT_DESKTOP",
                "_NET_DESKTOP_GEOMETRY",
                "_NET_CLIENT_LIST_STACKING",
            ])
            .output()
            .map_err(|e| PortError(format!("Failed to execute xprop: {e}")))?;
        if !root_out.status.success() {
            return Err(PortError(format!(
                "X11 display unreachable via xprop (status {:?})",
                root_out.status.code()
            )));
        }
        Ok(String::from_utf8_lossy(&root_out.stdout).into_owned())
    }

    fn query_window_props(wid: &str) -> Option<String> {
        let props = Command::new("xprop")
            .args([
                "-id",
                wid,
                "WM_CLASS",
                "WM_STATE",
                "_NET_WM_STATE",
                "_NET_WM_DESKTOP",
            ])
            .output()
            .ok()?;
        props
            .status
            .success()
            .then(|| String::from_utf8_lossy(&props.stdout).into_owned())
    }

    fn find_dota_window(root_props: &str) -> Result<Option<String>, PortError> {
        let root_out = Command::new("xprop")
            .args(["-root", "_NET_CLIENT_LIST"])
            .output()
            .map_err(|e| PortError(format!("Failed to execute xprop: {e}")))?;
        if !root_out.status.success() {
            return Err(PortError(format!(
                "X11 display unreachable via xprop (status {:?})",
                root_out.status.code()
            )));
        }
        let root_text = String::from_utf8_lossy(&root_out.stdout);
        for tok in root_text.split_whitespace() {
            let wid = tok.trim_end_matches(',');
            if !wid.starts_with("0x") {
                continue;
            }
            let Some(props_str) = Self::query_window_props(wid) else {
                continue;
            };
            if !evaluate_x11_window_visibility_with(wid, root_props, &props_str, None, |_| None) {
                continue;
            }
            return Ok(Some(wid.to_string()));
        }
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
impl ScreenFrameSource for X11TopBarFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
        let root_props = Self::query_root_visibility_props()?;

        if let Some(ref wid) = self.cached_window {
            match Self::query_window_props(wid) {
                Some(dota_props) if has_dota2_wm_class(&dota_props) => {
                    if !evaluate_x11_window_visibility_with(
                        wid,
                        &root_props,
                        &dota_props,
                        self.last_frame_width,
                        Self::query_window_props,
                    ) {
                        return Ok(None);
                    }
                }
                _ => {
                    self.cached_window = None;
                    self.last_frame_width = None;
                }
            }
        }

        if self.cached_window.is_none() {
            self.cached_window = Self::find_dota_window(&root_props)?;
        }
        let Some(ref wid) = self.cached_window else {
            return Ok(None);
        };

        let output = Command::new("import")
            .args(build_x11_import_args(wid))
            .output()
            .map_err(|e| PortError(format!("Failed to execute ImageMagick import: {e}")))?;

        if !output.status.success() {
            self.cached_window = None;
            self.last_frame_width = None;
            // If Dota 2 window closed/minimized between lookup and import, wait cleanly;
            // if a Dota 2 window is still open and import failed, surface the error.
            if Self::find_dota_window(&root_props)?.is_none() {
                return Ok(None);
            }
            return Err(PortError(format!(
                "ImageMagick import capture failed (status {:?})",
                output.status.code()
            )));
        }

        let Some(frame) = parse_ppm_p6(&output.stdout) else {
            self.cached_window = None;
            self.last_frame_width = None;
            return Err(PortError(
                "X11 capture returned malformed PPM P6 output.".into(),
            ));
        };
        if frame.width < 640 || frame.height < 40 {
            return Ok(None);
        }
        self.last_frame_width = Some(frame.width);
        Ok(Some(frame))
    }
}
