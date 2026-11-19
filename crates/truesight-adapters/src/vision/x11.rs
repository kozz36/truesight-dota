#[cfg(target_os = "linux")]
use super::x11_geom::{X11GeometryClient, query_xwininfo_geometry};
#[cfg(any(target_os = "linux", test))]
use super::x11_geom::{X11WindowGeometry, parse_xprop_frame_extents, top_bar_intersects_window};
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

/// Evaluates whether a Dota 2 X11 window remains visible, on the active virtual desktop, and
/// unobstructed over its top bar (`100%x8%+0+0`) by higher-stacked windows.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn evaluate_x11_window_visibility_with<Fp, Fg>(
    wid: &str,
    root_props: &str,
    dota_props: &str,
    mut fetch_window_props: Fp,
    mut fetch_window_geometry: Fg,
) -> bool
where
    Fp: FnMut(&str) -> Option<String>,
    Fg: FnMut(&str) -> Option<X11WindowGeometry>,
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

    let dota_geom = fetch_window_geometry(wid);
    if let Some(geom) = dota_geom
        && (!geom.viewable || geom.width == 0 || geom.height == 0)
    {
        return false;
    }

    let Some(dota_geom) = dota_geom else {
        return true;
    };
    let Some(dota_id) = parse_xprop_window_id_u64(wid) else {
        return true;
    };

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
            let Some(other_geom) = fetch_window_geometry(other_wid) else {
                continue;
            };
            if !other_geom.viewable || other_geom.width == 0 || other_geom.height == 0 {
                continue;
            }
            let extents = parse_xprop_frame_extents(&other_props);
            if top_bar_intersects_window(dota_geom, other_geom, extents) {
                return false;
            }
        }
    }

    true
}

/// Selects the first candidate `"dota2"` window that is visible and unobstructed over its top bar.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn select_visible_dota_window_with<Fp, Fg>(
    root_props: &str,
    candidate_wids: &[String],
    mut fetch_window_props: Fp,
    mut fetch_window_geometry: Fg,
) -> Option<String>
where
    Fp: FnMut(&str) -> Option<String>,
    Fg: FnMut(&str) -> Option<X11WindowGeometry>,
{
    for wid in candidate_wids {
        let Some(props_str) = fetch_window_props(wid) else {
            continue;
        };
        if !has_dota2_wm_class(&props_str) {
            continue;
        }
        if evaluate_x11_window_visibility_with(
            wid,
            root_props,
            &props_str,
            &mut fetch_window_props,
            &mut fetch_window_geometry,
        ) {
            return Some(wid.clone());
        }
    }
    None
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

    fn query_root_visibility_props() -> Result<String, PortError> {
        let root_out = Command::new("xprop")
            .args(["-root", "_NET_CURRENT_DESKTOP", "_NET_CLIENT_LIST_STACKING"])
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
                "_NET_FRAME_EXTENTS",
            ])
            .output()
            .ok()?;
        props
            .status
            .success()
            .then(|| String::from_utf8_lossy(&props.stdout).into_owned())
    }

    fn find_dota_window<Fg>(
        root_props: &str,
        mut fetch_window_geometry: Fg,
    ) -> Result<Option<String>, PortError>
    where
        Fg: FnMut(&str) -> Option<X11WindowGeometry>,
    {
        let mut candidate_wids: Vec<String> = parse_xprop_stacking_ids(root_props)
            .into_iter()
            .rev()
            .map(|(_, wid)| wid)
            .collect();

        if candidate_wids.is_empty() {
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
                if wid.starts_with("0x") {
                    candidate_wids.push(wid.to_string());
                }
            }
        }

        Ok(select_visible_dota_window_with(
            root_props,
            &candidate_wids,
            Self::query_window_props,
            &mut fetch_window_geometry,
        ))
    }
}

#[cfg(target_os = "linux")]
impl ScreenFrameSource for X11TopBarFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
        let root_props = Self::query_root_visibility_props()?;
        let mut geom_client = None;
        let mut query_geom = |wid_str: &str| -> Option<X11WindowGeometry> {
            let client_opt = geom_client.get_or_insert_with(X11GeometryClient::connect);
            if let Some(client) = client_opt.as_mut() {
                let wid_num =
                    parse_xprop_window_id_u64(wid_str).and_then(|v| u32::try_from(v).ok())?;
                return client.query_window_geometry(wid_num);
            }
            query_xwininfo_geometry(wid_str)
        };

        if let Some(ref wid) = self.cached_window {
            match Self::query_window_props(wid) {
                Some(dota_props) if has_dota2_wm_class(&dota_props) => {
                    if !evaluate_x11_window_visibility_with(
                        wid,
                        &root_props,
                        &dota_props,
                        Self::query_window_props,
                        &mut query_geom,
                    ) {
                        return Ok(None);
                    }
                }
                _ => {
                    self.cached_window = None;
                }
            }
        }

        if self.cached_window.is_none() {
            self.cached_window = Self::find_dota_window(&root_props, &mut query_geom)?;
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
            // If Dota 2 window closed/minimized between lookup and import, wait cleanly;
            // if a Dota 2 window is still open and import failed, surface the error.
            if Self::find_dota_window(&root_props, &mut query_geom)?.is_none() {
                return Ok(None);
            }
            return Err(PortError(format!(
                "ImageMagick import capture failed (status {:?})",
                output.status.code()
            )));
        }

        let Some(frame) = parse_ppm_p6(&output.stdout) else {
            self.cached_window = None;
            return Err(PortError(
                "X11 capture returned malformed PPM P6 output.".into(),
            ));
        };
        if frame.width < 640 || frame.height < 40 {
            return Ok(None);
        }
        Ok(Some(frame))
    }
}
