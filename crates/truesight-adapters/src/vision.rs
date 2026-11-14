use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use truesight_core::domain::{Faction, HeroId, Role, Side};
use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

/// Embedded binary RGB atlas generated from Dota 2 official portraits + Gem icon.
pub const HERO_ATLAS_BYTES: &[u8] = include_bytes!("../assets/hero_atlas.bin");

#[derive(Debug, Clone)]
pub struct DecodedHeroAtlas {
    pub hero_width: usize,
    pub hero_height: usize,
    pub gem_width: usize,
    pub gem_height: usize,
    pub gem_rgb: Vec<u8>,
    pub heroes: HashMap<HeroId, Vec<u8>>,
}

/// Decodes the embedded binary RGB atlas (`TSHA` header, Gem icon, and 127 hero portraits).
pub fn parse_hero_atlas() -> Option<DecodedHeroAtlas> {
    if HERO_ATLAS_BYTES.len() < 14 || &HERO_ATLAS_BYTES[0..4] != b"TSHA" {
        return None;
    }
    let count = u16::from_le_bytes([HERO_ATLAS_BYTES[4], HERO_ATLAS_BYTES[5]]) as usize;
    let hero_width = u16::from_le_bytes([HERO_ATLAS_BYTES[6], HERO_ATLAS_BYTES[7]]) as usize;
    let hero_height = u16::from_le_bytes([HERO_ATLAS_BYTES[8], HERO_ATLAS_BYTES[9]]) as usize;
    let gem_width = u16::from_le_bytes([HERO_ATLAS_BYTES[10], HERO_ATLAS_BYTES[11]]) as usize;
    let gem_height = u16::from_le_bytes([HERO_ATLAS_BYTES[12], HERO_ATLAS_BYTES[13]]) as usize;

    let gem_len = gem_width.checked_mul(gem_height)?.checked_mul(3)?;
    let mut offset = 14usize;
    let gem_end = offset.checked_add(gem_len)?;
    if gem_end > HERO_ATLAS_BYTES.len() {
        return None;
    }
    let gem_rgb = HERO_ATLAS_BYTES[offset..gem_end].to_vec();
    offset = gem_end;

    let hero_len = hero_width.checked_mul(hero_height)?.checked_mul(3)?;
    let mut heroes = HashMap::with_capacity(count);
    for _ in 0..count {
        let id_end = offset.checked_add(2)?;
        if id_end > HERO_ATLAS_BYTES.len() {
            return None;
        }
        let hid = u16::from_le_bytes([HERO_ATLAS_BYTES[offset], HERO_ATLAS_BYTES[offset + 1]]);
        offset = id_end;
        let frame_end = offset.checked_add(hero_len)?;
        if frame_end > HERO_ATLAS_BYTES.len() {
            return None;
        }
        heroes.insert(HeroId(hid), HERO_ATLAS_BYTES[offset..frame_end].to_vec());
        offset = frame_end;
    }

    Some(DecodedHeroAtlas {
        hero_width,
        hero_height,
        gem_width,
        gem_height,
        gem_rgb,
        heroes,
    })
}

/// Top-bar hero portrait rectangle in frame pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopBarSlotRect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

fn build_topbar_rects(
    rad_xs: [usize; 5],
    dire_xs: [usize; 5],
    y: usize,
    width: usize,
    height: usize,
) -> ([TopBarSlotRect; 5], [TopBarSlotRect; 5]) {
    let mut rad = [TopBarSlotRect {
        x: 0,
        y,
        width,
        height,
    }; 5];
    let mut dire = [TopBarSlotRect {
        x: 0,
        y,
        width,
        height,
    }; 5];
    for i in 0..5 {
        rad[i].x = rad_xs[i];
        dire[i].x = dire_xs[i];
    }
    (rad, dire)
}

/// Computes the 5 Radiant (left) and 5 Dire (right) top-bar portrait slot rects for a 16:9 frame.
pub fn compute_topbar_slot_rects(
    frame_w: usize,
    frame_h: usize,
) -> ([TopBarSlotRect; 5], [TopBarSlotRect; 5]) {
    let slot_w = ((frame_w as f32) * (64.0 / 1920.0)).round().max(8.0) as usize;
    let slot_h = ((frame_h as f32) * (36.0 / 1080.0)).round().max(6.0) as usize;
    let y = ((frame_h as f32) * (8.0 / 1080.0)).round() as usize;
    let gap = ((frame_w as f32) * (4.0 / 1920.0)).round().max(1.0) as usize;
    let clock_half = ((frame_w as f32) * (68.0 / 1920.0)).round().max(8.0) as usize;
    let step = slot_w + gap;
    let center_x = frame_w / 2;

    let rad_start_x = center_x.saturating_sub(clock_half + 5 * step);
    let dire_start_x = center_x + clock_half + gap;

    let mut rad_xs = [0_usize; 5];
    let mut dire_xs = [0_usize; 5];
    for i in 0..5 {
        rad_xs[i] = rad_start_x + i * step;
        dire_xs[i] = dire_start_x + i * step;
    }

    build_topbar_rects(rad_xs, dire_xs, y, slot_w, slot_h)
}

/// Computes Pick Phase top-bar slot coordinates calibrated on real Dota 2 (`2560x1440` scaled).
pub fn compute_pickphase_topbar_slot_rects(
    frame_w: usize,
    virtual_h: usize,
) -> ([TopBarSlotRect; 5], [TopBarSlotRect; 5]) {
    let scale_x = (frame_w as f32) / 2560.0;
    let scale_y = (virtual_h as f32) / 1440.0;
    let y = (4.0 * scale_y).round() as usize;
    let width = (121.0 * scale_x).round().max(8.0) as usize;
    let height = (56.0 * scale_y).round().max(8.0) as usize;

    let mut rad_xs = [0_usize; 5];
    let mut dire_xs = [0_usize; 5];
    for i in 0..5 {
        rad_xs[i] = ((282.0 + (i as f32) * 165.0 + 12.0) * scale_x).round() as usize;
        dire_xs[i] = ((1470.0 + (i as f32) * 165.0 + 12.0) * scale_x).round() as usize;
    }

    build_topbar_rects(rad_xs, dire_xs, y, width, height)
}

/// Computes In-Game HUD top-bar slot coordinates calibrated on real Dota 2 (`2560x1440` scaled).
pub fn compute_ingame_topbar_slot_rects(
    frame_w: usize,
    virtual_h: usize,
) -> ([TopBarSlotRect; 5], [TopBarSlotRect; 5]) {
    let scale_x = (frame_w as f32) / 2560.0;
    let scale_y = (virtual_h as f32) / 1440.0;
    let y = (2.0 * scale_y).round() as usize;
    let width = (76.0 * scale_x).round().max(8.0) as usize;
    let height = (44.0 * scale_y).round().max(8.0) as usize;

    let mut rad_xs = [0_usize; 5];
    let mut dire_xs = [0_usize; 5];
    for i in 0..5 {
        rad_xs[i] = ((728.0 + (i as f32) * 83.5) * scale_x).round() as usize;
        dire_xs[i] = ((1420.0 + (i as f32) * 83.5) * scale_x).round() as usize;
    }

    build_topbar_rects(rad_xs, dire_xs, y, width, height)
}

const DESC_COLS: usize = 8;
const DESC_ROWS: usize = 4;
const DESC_LEN: usize = DESC_COLS * DESC_ROWS * 3;
const DARK_INTENSITY_THRESHOLD: f32 = 12.0;
const MATCH_L1_THRESHOLD: f32 = 25.0;
const PICKPHASE_L1_THRESHOLD: f32 = 31.0;
const INGAME_L1_THRESHOLD: f32 = 34.5;

fn compute_descriptor_8x4(
    rgb: &[u8],
    img_w: usize,
    img_h: usize,
    rect: TopBarSlotRect,
) -> Option<([u8; DESC_LEN], f32)> {
    if rect.width == 0
        || rect.height == 0
        || rect.x.checked_add(rect.width)? > img_w
        || rect.y.checked_add(rect.height)? > img_h
    {
        return None;
    }
    let required = img_w.checked_mul(img_h)?.checked_mul(3)?;
    if rgb.len() < required {
        return None;
    }

    let mut out = [0_u8; DESC_LEN];
    let mut total_intensity = 0_u64;

    for cy in 0..DESC_ROWS {
        let y0 = rect.y + (cy * rect.height) / DESC_ROWS;
        let y1 = (rect.y + ((cy + 1) * rect.height) / DESC_ROWS)
            .max(y0 + 1)
            .min(rect.y + rect.height);
        for cx in 0..DESC_COLS {
            let x0 = rect.x + (cx * rect.width) / DESC_COLS;
            let x1 = (rect.x + ((cx + 1) * rect.width) / DESC_COLS)
                .max(x0 + 1)
                .min(rect.x + rect.width);

            let mut sum_r = 0_u64;
            let mut sum_g = 0_u64;
            let mut sum_b = 0_u64;
            let mut count = 0_u64;

            for py in y0..y1 {
                let row_base = py * img_w * 3;
                for px in x0..x1 {
                    let idx = row_base + px * 3;
                    sum_r += u64::from(rgb[idx]);
                    sum_g += u64::from(rgb[idx + 1]);
                    sum_b += u64::from(rgb[idx + 2]);
                    count += 1;
                }
            }

            let count = count.max(1);
            let r = (sum_r / count) as u8;
            let g = (sum_g / count) as u8;
            let b = (sum_b / count) as u8;

            let out_idx = (cy * DESC_COLS + cx) * 3;
            out[out_idx] = r;
            out[out_idx + 1] = g;
            out[out_idx + 2] = b;
            total_intensity += u64::from(r) + u64::from(g) + u64::from(b);
        }
    }

    let mean_intensity = (total_intensity as f32) / (DESC_LEN as f32);
    Some((out, mean_intensity))
}

fn outer_column_stats(desc: &[u8; DESC_LEN]) -> (f32, f32) {
    let mut r_sum = 0_f32;
    let mut g_sum = 0_f32;
    let mut b_sum = 0_f32;
    let mut sat_sum = 0_f32;
    let outer_cols = [0_usize, 1, 6, 7];
    let n = (DESC_ROWS * outer_cols.len()) as f32;

    for cy in 0..DESC_ROWS {
        for &cx in &outer_cols {
            let idx = (cy * DESC_COLS + cx) * 3;
            let r = f32::from(desc[idx]);
            let g = f32::from(desc[idx + 1]);
            let b = f32::from(desc[idx + 2]);
            r_sum += r;
            g_sum += g;
            b_sum += b;
            sat_sum += r.max(g).max(b) - r.min(g).min(b);
        }
    }

    let mr = r_sum / n;
    let mg = g_sum / n;
    let mb = b_sum / n;
    let mut var_sum = 0_f32;
    for cy in 0..DESC_ROWS {
        for &cx in &outer_cols {
            let idx = (cy * DESC_COLS + cx) * 3;
            var_sum += (f32::from(desc[idx]) - mr).abs()
                + (f32::from(desc[idx + 1]) - mg).abs()
                + (f32::from(desc[idx + 2]) - mb).abs();
        }
    }

    (var_sum / (n * 3.0), sat_sum / n)
}

fn mean_descriptor_saturation(desc: &[u8; DESC_LEN]) -> f32 {
    let mut sat_sum = 0_f32;
    let total_cells = DESC_COLS * DESC_ROWS;
    for cell in 0..total_cells {
        let idx = cell * 3;
        let r = f32::from(desc[idx]);
        let g = f32::from(desc[idx + 1]);
        let b = f32::from(desc[idx + 2]);
        sat_sum += r.max(g).max(b) - r.min(g).min(b);
    }
    sat_sum / (total_cells as f32)
}

fn match_descriptor(
    entries: &[(HeroId, [u8; DESC_LEN])],
    query_desc: &[u8; DESC_LEN],
) -> Option<(HeroId, f32)> {
    let mut best_hero = None;
    let mut best_diff = f32::MAX;

    for (hero_id, candidate_desc) in entries {
        let mut sum_abs = 0_u32;
        for i in 0..DESC_LEN {
            sum_abs += u32::from(query_desc[i].abs_diff(candidate_desc[i]));
        }
        let mean_l1 = (sum_abs as f32) / (DESC_LEN as f32);
        if mean_l1 < best_diff {
            best_diff = mean_l1;
            best_hero = Some(*hero_id);
        }
    }

    best_hero.map(|id| (id, best_diff))
}

/// Precomputed 8x4 cell-averaged RGB portrait catalog for all 127 Dota 2 heroes.
#[derive(Debug, Clone)]
pub struct HeroVisionCatalog {
    full_entries: Vec<(HeroId, [u8; DESC_LEN])>,
    inner_entries: Vec<(HeroId, [u8; DESC_LEN])>,
    upper_entries: Vec<(HeroId, [u8; DESC_LEN])>,
}

impl HeroVisionCatalog {
    pub fn from_embedded() -> Option<Self> {
        let atlas = parse_hero_atlas()?;
        let full_rect = TopBarSlotRect {
            x: 0,
            y: 0,
            width: atlas.hero_width,
            height: atlas.hero_height,
        };
        let inner_rect = TopBarSlotRect {
            x: 4,
            y: 2,
            width: atlas.hero_width.saturating_sub(8),
            height: atlas.hero_height.saturating_sub(4),
        };
        let upper_rect = TopBarSlotRect {
            x: 4,
            y: 2,
            width: atlas.hero_width.saturating_sub(8),
            height: 22.min(atlas.hero_height.saturating_sub(2)),
        };

        let mut full_entries = Vec::with_capacity(atlas.heroes.len());
        let mut inner_entries = Vec::with_capacity(atlas.heroes.len());
        let mut upper_entries = Vec::with_capacity(atlas.heroes.len());

        for (hero_id, rgb) in atlas.heroes {
            if let Some((desc, _)) =
                compute_descriptor_8x4(&rgb, atlas.hero_width, atlas.hero_height, full_rect)
            {
                full_entries.push((hero_id, desc));
            }
            if let Some((desc, _)) =
                compute_descriptor_8x4(&rgb, atlas.hero_width, atlas.hero_height, inner_rect)
            {
                inner_entries.push((hero_id, desc));
            }
            if let Some((desc, _)) =
                compute_descriptor_8x4(&rgb, atlas.hero_width, atlas.hero_height, upper_rect)
            {
                upper_entries.push((hero_id, desc));
            }
        }

        full_entries.sort_by_key(|(id, _)| id.0);
        inner_entries.sort_by_key(|(id, _)| id.0);
        upper_entries.sort_by_key(|(id, _)| id.0);

        Some(Self {
            full_entries,
            inner_entries,
            upper_entries,
        })
    }

    /// Matches a cropped top-bar portrait rectangle inside an RGB frame to the closest hero.
    pub fn match_portrait_crop(
        &self,
        frame_rgb: &[u8],
        frame_w: usize,
        frame_h: usize,
        rect: TopBarSlotRect,
    ) -> Option<HeroId> {
        let (query_desc, mean_intensity) = compute_descriptor_8x4(frame_rgb, frame_w, frame_h, rect)?;
        if mean_intensity < DARK_INTENSITY_THRESHOLD {
            return None;
        }
        let (best_hero, best_diff) = match_descriptor(&self.full_entries, &query_desc)?;
        if best_diff < MATCH_L1_THRESHOLD {
            Some(best_hero)
        } else {
            None
        }
    }

    fn match_slot_with_layout(
        &self,
        frame_rgb: &[u8],
        frame_w: usize,
        frame_h: usize,
        rect: TopBarSlotRect,
        layout: TopBarLayout,
    ) -> Option<(HeroId, f32, f32)> {
        let (query_desc, mean_intensity) = compute_descriptor_8x4(frame_rgb, frame_w, frame_h, rect)?;
        if mean_intensity < DARK_INTENSITY_THRESHOLD {
            return None;
        }

        let (entries, threshold, min_outer) = match layout {
            TopBarLayout::SyntheticExact => (&self.full_entries, MATCH_L1_THRESHOLD, None),
            TopBarLayout::PickPhase => (
                &self.upper_entries,
                PICKPHASE_L1_THRESHOLD,
                Some((14.0_f32, 20.0_f32)),
            ),
            TopBarLayout::InGame => (
                &self.inner_entries,
                INGAME_L1_THRESHOLD,
                Some((8.0_f32, 10.0_f32)),
            ),
        };

        if let Some((min_var, min_sat)) = min_outer {
            let (ov, os) = outer_column_stats(&query_desc);
            if ov < min_var && os < min_sat {
                return None;
            }
        }

        let (best_hero, best_diff) = match_descriptor(entries, &query_desc)?;
        if best_diff < threshold {
            Some((best_hero, best_diff, mean_descriptor_saturation(&query_desc)))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopBarLayout {
    SyntheticExact,
    PickPhase,
    InGame,
}

/// Captured screen frame in tightly packed 24-bit RGB (`width * height * 3` bytes).
#[derive(Debug, Clone)]
pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

/// Parses binary PPM (`P6`) image bytes into an `RgbFrame`.
pub fn parse_ppm_p6(bytes: &[u8]) -> Option<RgbFrame> {
    let mut pos = 0_usize;
    let len = bytes.len();

    let next_token = |pos: &mut usize| -> Option<&[u8]> {
        while *pos < len {
            let b = bytes[*pos];
            if b == b'#' {
                while *pos < len && bytes[*pos] != b'\n' {
                    *pos += 1;
                }
            } else if b.is_ascii_whitespace() {
                *pos += 1;
            } else {
                break;
            }
        }
        let start = *pos;
        while *pos < len && !bytes[*pos].is_ascii_whitespace() && bytes[*pos] != b'#' {
            *pos += 1;
        }
        if *pos > start {
            Some(&bytes[start..*pos])
        } else {
            None
        }
    };

    if next_token(&mut pos)? != b"P6" {
        return None;
    }
    let width = std::str::from_utf8(next_token(&mut pos)?).ok()?.parse::<usize>().ok()?;
    let height = std::str::from_utf8(next_token(&mut pos)?).ok()?.parse::<usize>().ok()?;
    let maxval = std::str::from_utf8(next_token(&mut pos)?).ok()?.parse::<usize>().ok()?;
    if width == 0 || height == 0 || maxval != 255 {
        return None;
    }

    if pos >= len || !bytes[pos].is_ascii_whitespace() {
        return None;
    }
    pos += 1;

    let expected_len = width.checked_mul(height)?.checked_mul(3)?;
    let end = pos.checked_add(expected_len)?;
    if end > len {
        return None;
    }

    Some(RgbFrame {
        width,
        height,
        rgb: bytes[pos..end].to_vec(),
    })
}

/// Port abstraction for PipeWire / X11 / test frame capture backends.
pub trait ScreenFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError>;
}

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

/// Live X11/XWayland top-bar frame source capturing the `"dota2"` window strip via `xprop` + `import`.
#[derive(Debug, Default)]
pub struct X11TopBarFrameSource {
    cached_window: Option<String>,
}

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

impl ScreenFrameSource for X11TopBarFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
        if self.cached_window.is_none() {
            self.cached_window = Self::find_dota_window();
        }
        let Some(ref wid) = self.cached_window else {
            return Ok(None);
        };

        let output = Command::new("import")
            .args(["-window", wid, "-depth", "8", "-crop", "100%x8%+0+0", "ppm:-"])
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

const MIN_DETECTED_SLOTS_QUORUM: usize = 2;
const MIN_OVERWRITE_SATURATION: f32 = 12.0;
const MAX_OVERWRITE_L1_DIFF: f32 = 26.0;
const OVERWRITE_CONFIRM_FRAMES: u8 = 2;

type SlotCandidate = Option<(HeroId, f32, f32)>;

fn push_pick_event(events: &mut Vec<DraftEvent>, side: Side, slot: usize, hero_id: HeroId) {
    events.push(DraftEvent::PickDetected {
        side,
        slot,
        hero_id,
        role: Role::ALL.get(slot).copied(),
    });
}

/// Applies per-slot hysteresis and dead-portrait saturation checks before mutating locked picks.
fn resolve_slot_transition(
    side: Side,
    slot: usize,
    faction_changed: bool,
    candidate: SlotCandidate,
    last_slot: &mut Option<HeroId>,
    pending_slot: &mut Option<(HeroId, u8)>,
    events: &mut Vec<DraftEvent>,
) {
    match *last_slot {
        None => {
            *pending_slot = None;
            if let Some((hero_id, _, _)) = candidate {
                *last_slot = Some(hero_id);
                push_pick_event(events, side, slot, hero_id);
            }
        }
        Some(prev_id) => match candidate {
            Some((cand_id, diff, sat)) if cand_id != prev_id => {
                // Ignore desaturated dead/respawn overlays or weak matches on already-locked slots.
                if sat < MIN_OVERWRITE_SATURATION || diff > MAX_OVERWRITE_L1_DIFF {
                    *pending_slot = None;
                    if faction_changed {
                        push_pick_event(events, side, slot, prev_id);
                    }
                    return;
                }
                let next_count = match *pending_slot {
                    Some((pending_id, count)) if pending_id == cand_id => count.saturating_add(1),
                    _ => 1,
                };
                if next_count >= OVERWRITE_CONFIRM_FRAMES {
                    *last_slot = Some(cand_id);
                    *pending_slot = None;
                    push_pick_event(events, side, slot, cand_id);
                } else {
                    *pending_slot = Some((cand_id, next_count));
                    if faction_changed {
                        push_pick_event(events, side, slot, prev_id);
                    }
                }
            }
            _ => {
                *pending_slot = None;
                if faction_changed {
                    push_pick_event(events, side, slot, prev_id);
                }
            }
        },
    }
}

/// Driving adapter matching the 10 Dota 2 top-bar hero portrait slots into `DraftEvent::PickDetected`.
pub struct VisionCaptureAdapter<S: ScreenFrameSource> {
    source: S,
    catalog: HeroVisionCatalog,
    ally_faction: Faction,
    last_ally_faction: Option<Faction>,
    last_radiant: [Option<HeroId>; 5],
    last_dire: [Option<HeroId>; 5],
    pending_radiant: [Option<(HeroId, u8)>; 5],
    pending_dire: [Option<(HeroId, u8)>; 5],
}

impl<S: ScreenFrameSource> VisionCaptureAdapter<S> {
    pub fn new(source: S, catalog: HeroVisionCatalog) -> Self {
        Self {
            source,
            catalog,
            ally_faction: Faction::Radiant,
            last_ally_faction: None,
            last_radiant: [None; 5],
            last_dire: [None; 5],
            pending_radiant: [None; 5],
            pending_dire: [None; 5],
        }
    }

    pub fn set_ally_faction(&mut self, faction: Faction) {
        self.ally_faction = faction;
    }

    /// Clears cached per-slot state so stationary portraits re-emit on the next poll.
    pub fn reset_state(&mut self) {
        self.last_ally_faction = None;
        self.last_radiant = [None; 5];
        self.last_dire = [None; 5];
        self.pending_radiant = [None; 5];
        self.pending_dire = [None; 5];
    }

    fn evaluate_side_slots(
        &self,
        frame: &RgbFrame,
        rects: &[TopBarSlotRect; 5],
        layout: TopBarLayout,
        seen: &mut Vec<HeroId>,
        score: &mut f32,
    ) -> [SlotCandidate; 5] {
        let mut candidates = [None; 5];
        for (slot, rect) in rects.iter().enumerate() {
            if let Some((hid, diff, sat)) =
                self.catalog
                    .match_slot_with_layout(&frame.rgb, frame.width, frame.height, *rect, layout)
            {
                candidates[slot] = Some((hid, diff, sat));
                if !seen.contains(&hid) {
                    seen.push(hid);
                    *score += 35.5 - diff;
                    if layout == TopBarLayout::SyntheticExact && diff < 8.0 {
                        *score += 16.0;
                    }
                }
            }
        }
        candidates
    }

    fn evaluate_layout(
        &self,
        frame: &RgbFrame,
        virtual_h: usize,
        layout: TopBarLayout,
    ) -> (f32, [SlotCandidate; 5], [SlotCandidate; 5]) {
        let (rad_rects, dire_rects) = match layout {
            TopBarLayout::SyntheticExact => compute_topbar_slot_rects(frame.width, virtual_h),
            TopBarLayout::PickPhase => compute_pickphase_topbar_slot_rects(frame.width, virtual_h),
            TopBarLayout::InGame => compute_ingame_topbar_slot_rects(frame.width, virtual_h),
        };

        let mut seen = Vec::with_capacity(10);
        let mut score = 0.0_f32;
        let rad = self.evaluate_side_slots(frame, &rad_rects, layout, &mut seen, &mut score);
        let dire = self.evaluate_side_slots(frame, &dire_rects, layout, &mut seen, &mut score);

        (score, rad, dire)
    }
}

impl<S: ScreenFrameSource> DraftEventSource for VisionCaptureAdapter<S> {
    type Error = PortError;

    fn poll_events(&mut self) -> Result<Vec<DraftEvent>, Self::Error> {
        let Some(frame) = self.source.capture_frame()? else {
            return Ok(Vec::new());
        };

        let virtual_h = if frame.height * 4 < frame.width {
            (frame.width * 9) / 16
        } else {
            frame.height
        };

        let mut best_score = 0.0_f32;
        let mut best_rad = [None; 5];
        let mut best_dire = [None; 5];

        for layout in [
            TopBarLayout::SyntheticExact,
            TopBarLayout::PickPhase,
            TopBarLayout::InGame,
        ] {
            let (score, rad, dire) = self.evaluate_layout(&frame, virtual_h, layout);
            if score > best_score {
                best_score = score;
                best_rad = rad;
                best_dire = dire;
            }
        }

        let detected_count = best_rad.iter().flatten().count() + best_dire.iter().flatten().count();
        if detected_count < MIN_DETECTED_SLOTS_QUORUM {
            return Ok(Vec::new());
        }

        let (rad_side, dire_side) = match self.ally_faction {
            Faction::Radiant => (Side::Allies, Side::Enemies),
            Faction::Dire => (Side::Enemies, Side::Allies),
        };
        let faction_changed = self.last_ally_faction != Some(self.ally_faction);

        let mut events = Vec::new();
        for slot in 0..5 {
            resolve_slot_transition(
                rad_side,
                slot,
                faction_changed,
                best_rad[slot],
                &mut self.last_radiant[slot],
                &mut self.pending_radiant[slot],
                &mut events,
            );
            resolve_slot_transition(
                dire_side,
                slot,
                faction_changed,
                best_dire[slot],
                &mut self.last_dire[slot],
                &mut self.pending_dire[slot],
                &mut events,
            );
        }

        self.last_ally_faction = Some(self.ally_faction);
        Ok(events)
    }
}

/// Bounded background worker polling Dota 2 top-bar frames at ~1 Hz.
pub struct BoundedVisionWorker {
    rx: Receiver<DraftEvent>,
    ally_faction_code: Arc<AtomicU8>,
    reset_state: Arc<(AtomicBool, std::sync::Mutex<u64>)>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl BoundedVisionWorker {
    pub fn spawn() -> Option<Self> {
        Self::spawn_with_source(X11TopBarFrameSource::new())
    }

    /// Spawns a bounded background vision worker backed by a custom frame source.
    pub fn spawn_with_source<S: ScreenFrameSource + Send + 'static>(source: S) -> Option<Self> {
        let catalog = HeroVisionCatalog::from_embedded()?;
        let (tx, rx) = mpsc::sync_channel(32);
        let ally_faction_code = Arc::new(AtomicU8::new(0));
        let reset_state = Arc::new((AtomicBool::new(false), std::sync::Mutex::new(0)));
        let shutdown = Arc::new(AtomicBool::new(false));
        let faction_ref = Arc::clone(&ally_faction_code);
        let reset_ref = Arc::clone(&reset_state);
        let shutdown_ref = Arc::clone(&shutdown);

        let worker = thread::Builder::new()
            .name("truesight-vision-worker".into())
            .spawn(move || {
                run_vision_loop(source, catalog, tx, faction_ref, reset_ref, shutdown_ref)
            })
            .ok()?;

        Some(Self {
            rx,
            ally_faction_code,
            reset_state,
            shutdown,
            worker: Some(worker),
        })
    }

    pub fn set_ally_faction(&self, faction: Faction) {
        let code = if faction == Faction::Dire { 1 } else { 0 };
        self.ally_faction_code.store(code, Ordering::Relaxed);
    }

    /// Invalidates in-flight pre-reset polls, drains queued events, and resets detector state.
    pub fn reset(&mut self) {
        let mut seq = self.reset_state.1.lock().unwrap_or_else(|e| e.into_inner());
        *seq = seq.wrapping_add(1);
        self.reset_state.0.store(true, Ordering::Release);
        while self.rx.try_recv().is_ok() {}
    }
}

impl DraftEventSource for BoundedVisionWorker {
    type Error = PortError;

    fn poll_events(&mut self) -> Result<Vec<DraftEvent>, Self::Error> {
        let mut batch = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            batch.push(ev);
        }
        Ok(batch)
    }
}

impl Drop for BoundedVisionWorker {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn run_vision_loop<S: ScreenFrameSource>(
    source: S,
    catalog: HeroVisionCatalog,
    tx: SyncSender<DraftEvent>,
    ally_faction_code: Arc<AtomicU8>,
    reset_state: Arc<(AtomicBool, std::sync::Mutex<u64>)>,
    shutdown: Arc<AtomicBool>,
) {
    let mut adapter = VisionCaptureAdapter::new(source, catalog);
    while !shutdown.load(Ordering::Acquire) {
        let poll_seq = {
            let seq = *reset_state.1.lock().unwrap_or_else(|e| e.into_inner());
            if reset_state.0.swap(false, Ordering::AcqRel) {
                adapter.reset_state();
            }
            seq
        };
        let dire = ally_faction_code.load(Ordering::Relaxed) == 1;
        adapter.set_ally_faction(if dire { Faction::Dire } else { Faction::Radiant });

        if let Ok(events) = adapter.poll_events() {
            let seq = reset_state.1.lock().unwrap_or_else(|e| e.into_inner());
            if *seq == poll_seq {
                for ev in events {
                    if matches!(tx.try_send(ev), Err(TrySendError::Disconnected(_))) {
                        return;
                    }
                }
            }
        }

        for _ in 0..24 {
            if shutdown.load(Ordering::Acquire) || reset_state.0.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}
