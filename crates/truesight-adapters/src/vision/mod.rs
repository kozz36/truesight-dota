mod capture;
mod windows;
mod worker;
mod x11;
mod x11_geom;

pub use capture::{
    CaptureAvailability, CaptureBackendKind, CaptureRuntimeHealth, detect_capture_availability,
    detect_capture_availability_for, spawn_platform_worker, spawn_platform_worker_for,
};
#[cfg(target_os = "windows")]
pub use windows::WindowsTopBarFrameSource;
pub use windows::{
    evaluate_pid_lookup_output, is_dota2_windows_window, parse_tasklist_exe_for_pid,
    resolve_windows_system32_exe, rgba_to_topbar_rgb_frame, verify_dota_candidate_with,
};
pub use worker::{BoundedVisionWorker, ScreenFrameSource, VisionCaptureAdapter};
#[cfg(test)]
pub(crate) use worker::{dedup_layout_candidates, select_best_layout_candidates};
#[cfg(target_os = "linux")]
pub use x11::X11TopBarFrameSource;
pub use x11::{build_x11_import_args, is_dota2_xprop_output};
#[cfg(test)]
pub(crate) use x11::{
    evaluate_x11_window_visibility_with, resolve_window_geometry_with,
    select_visible_dota_window_with,
};
#[cfg(all(test, target_os = "linux"))]
pub(crate) use x11_geom::X11GeometryClient;
#[cfg(test)]
pub(crate) use x11_geom::{
    X11LocalDisplay, X11WindowGeometry, parse_x11_local_display, parse_xauth_cookie_for_local,
    parse_xprop_frame_extents, parse_xwininfo_geometry, top_bar_intersects_window,
};

use std::collections::HashMap;
use truesight_core::domain::HeroId;

/// Embedded binary RGB atlas generated from Dota 2 official portraits + Gem icon.
pub const HERO_ATLAS_BYTES: &[u8] = include_bytes!("../../assets/hero_atlas.bin");

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

/// Computes Pick Phase top-bar slot coordinates calibrated on real Dota 2 (`2560x1440` scaled),
/// starting below the `y = 0..7` player-color strip and clearing the `/` / `\` slanted borders.
pub fn compute_pickphase_topbar_slot_rects(
    frame_w: usize,
    virtual_h: usize,
) -> ([TopBarSlotRect; 5], [TopBarSlotRect; 5]) {
    let scale_x = (frame_w as f32) / 2560.0;
    let scale_y = (virtual_h as f32) / 1440.0;
    let y = (9.0 * scale_y).round() as usize;
    let width = (118.0 * scale_x).round().max(8.0) as usize;
    let height = (82.0 * scale_y).round().max(8.0) as usize;

    let mut rad_xs = [0_usize; 5];
    let mut dire_xs = [0_usize; 5];
    for i in 0..5 {
        rad_xs[i] = ((280.0 + (i as f32) * 165.0 + 18.0) * scale_x).round() as usize;
        dire_xs[i] = ((1475.0 + (i as f32) * 165.0 + 8.0) * scale_x).round() as usize;
    }

    build_topbar_rects(rad_xs, dire_xs, y, width, height)
}

/// Insets a `PickPhase` portrait slot rectangle to its upper badge-free portrait core (`22/36`
/// of portrait height), excluding bottom Dota Plus tier emblems and info corner badges while
/// staying strictly below the top player-color strip.
pub(crate) fn pickphase_upper_core_rect(rect: TopBarSlotRect) -> TopBarSlotRect {
    let height = (((rect.height as f32) * (22.0 / 36.0)).round() as usize).min(rect.height);
    if rect.width >= DESC_COLS && height >= DESC_ROWS {
        TopBarSlotRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height,
        }
    } else {
        rect
    }
}

/// Insets an `InGame` HUD slot rectangle to its interior portrait core, excluding the top
/// player-color strip, Dota Plus corner triangles, and spectator selection glow borders.
pub(super) fn ingame_inner_core_rect(rect: TopBarSlotRect) -> TopBarSlotRect {
    let left = ((rect.width as f32) * (5.0 / 57.0)).round() as usize;
    let right = ((rect.width as f32) * (5.0 / 57.0)).round() as usize;
    let top = ((rect.height as f32) * (5.0 / 33.0)).round() as usize;
    let bottom = ((rect.height as f32) * (1.0 / 33.0)).round() as usize;
    let core_w = rect.width.saturating_sub(left + right);
    let core_h = rect.height.saturating_sub(top + bottom);
    if core_w >= DESC_COLS && core_h >= DESC_ROWS {
        TopBarSlotRect {
            x: rect.x + left,
            y: rect.y + top,
            width: core_w,
            height: core_h,
        }
    } else {
        rect
    }
}

/// Insets an `InGame` HUD slot rectangle to its badge-free upper portrait core, aligned with
/// real `82x46 @ y=6` Dota 2 HUD portraits while staying within `rect` and excluding the top
/// player-color strip and bottom Dota Plus tier emblems.
pub(crate) fn ingame_upper_core_rect(rect: TopBarSlotRect) -> TopBarSlotRect {
    let left = ((rect.width as f32) * (6.0 / 76.0)).round() as usize;
    // Floor top inset so scaled resolutions (e.g. 1920x1080 where rect.y already rounds up)
    // avoid double-rounding downward into the badge region.
    let top = ((rect.height as f32) * (6.0 / 44.0)).floor() as usize;
    let max_w = rect.width.saturating_sub(left);
    let max_h = rect.height.saturating_sub(top);
    let width = (((rect.width as f32) * (70.0 / 76.0)).round() as usize).min(max_w);
    let height = (((rect.height as f32) * (26.0 / 44.0)).round() as usize).min(max_h);
    if width >= DESC_COLS && height >= DESC_ROWS {
        TopBarSlotRect {
            x: rect.x + left,
            y: rect.y + top,
            width,
            height,
        }
    } else {
        rect
    }
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
const MATCH_L1_THRESHOLD: f32 = 14.5;
const PICKPHASE_L1_THRESHOLD: f32 = 31.0;
/// Stricter L1 ceiling for the partial upper-core `PickPhase` crop (`22/36` portrait height) so
/// bottom-badged portraits match accurately without promoting empty/darkened blind-pick slots.
const PICKPHASE_UPPER_CORE_L1_THRESHOLD: f32 = 26.0;
const INGAME_L1_THRESHOLD: f32 = 35.0;
/// Stricter L1 ceiling for the partial upper-core `InGame` crop (`20/36` portrait height),
/// aligned with `MAX_OVERWRITE_L1_DIFF` and `weak_cutoff` so shifted or half-obscured portraits
/// abstain instead of locking a wrong hero on frame 2.
const INGAME_UPPER_CORE_L1_THRESHOLD: f32 = 26.0;
const MIN_DESCRIPTOR_SATURATION: f32 = 13.0;
const MIN_OUTER_SATURATION: f32 = 9.0;
const MIN_MATCH_NCC_8: f32 = 0.68;

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

fn descriptor_ncc(a: &[u8; DESC_LEN], b: &[u8; DESC_LEN]) -> f32 {
    let n = DESC_LEN as f32;
    let mut sum_a = 0_u32;
    let mut sum_b = 0_u32;
    for i in 0..DESC_LEN {
        sum_a += u32::from(a[i]);
        sum_b += u32::from(b[i]);
    }
    let mean_a = (sum_a as f32) / n;
    let mean_b = (sum_b as f32) / n;
    let mut num = 0.0_f32;
    let mut var_a = 0.0_f32;
    let mut var_b = 0.0_f32;
    for i in 0..DESC_LEN {
        let da = f32::from(a[i]) - mean_a;
        let db = f32::from(b[i]) - mean_b;
        num += da * db;
        var_a += da * da;
        var_b += db * db;
    }
    let denom = (var_a * var_b).sqrt();
    if denom < 1e-6 { 0.0 } else { num / denom }
}

fn descriptor_region_stats(desc: &[u8; DESC_LEN], rows: &[usize], cols: &[usize]) -> (f32, f32) {
    let n = (rows.len() * cols.len()) as f32;
    let (mut r_sum, mut g_sum, mut b_sum, mut sat_sum) = (0_f32, 0_f32, 0_f32, 0_f32);
    for &cy in rows {
        for &cx in cols {
            let idx = (cy * DESC_COLS + cx) * 3;
            let (r, g, b) = (
                f32::from(desc[idx]),
                f32::from(desc[idx + 1]),
                f32::from(desc[idx + 2]),
            );
            r_sum += r;
            g_sum += g;
            b_sum += b;
            sat_sum += r.max(g).max(b) - r.min(g).min(b);
        }
    }
    let (mr, mg, mb) = (r_sum / n, g_sum / n, b_sum / n);
    let mut var_sum = 0_f32;
    for &cy in rows {
        for &cx in cols {
            let idx = (cy * DESC_COLS + cx) * 3;
            var_sum += (f32::from(desc[idx]) - mr).abs()
                + (f32::from(desc[idx + 1]) - mg).abs()
                + (f32::from(desc[idx + 2]) - mb).abs();
        }
    }
    (var_sum / (n * 3.0), sat_sum / n)
}

/// Rejects half-obscured/partial `InGame` slots whose left, right, top, or bottom edge band is
/// flat desaturated chrome/void. Across all 176 embedded hero/alt portraits, every edge band
/// keeps `var >= 4.10` and `sat >= 10.88`.
fn has_flat_occluded_border(desc: &[u8; DESC_LEN]) -> bool {
    const ALL_ROWS: &[usize] = &[0, 1, 2, 3];
    const ALL_COLS: &[usize] = &[0, 1, 2, 3, 4, 5, 6, 7];
    [
        (ALL_ROWS, &[0, 1][..]),
        (ALL_ROWS, &[6, 7][..]),
        (&[0, 1][..], ALL_COLS),
        (&[2, 3][..], ALL_COLS),
    ]
    .into_iter()
    .any(|(rows, cols)| {
        let (var, sat) = descriptor_region_stats(desc, rows, cols);
        var < 2.5 && sat < 5.0
    })
}

fn accumulate_descriptor_matches(
    entry_sets: &[&[(HeroId, [u8; DESC_LEN])]],
    query_desc: &[u8; DESC_LEN],
    query_saturation: f32,
    threshold: f32,
    min_ncc8: f32,
    best_by_hero: &mut HashMap<HeroId, (f32, f32)>,
) {
    for entries in entry_sets {
        for (hero_id, candidate_desc) in *entries {
            let mut sum_abs = 0_u32;
            for i in 0..DESC_LEN {
                sum_abs += u32::from(query_desc[i].abs_diff(candidate_desc[i]));
            }
            let mean_l1 = (sum_abs as f32) / (DESC_LEN as f32);
            if mean_l1 >= threshold {
                continue;
            }
            // Reject smooth UI gradients or uncorrelated highlights that share a coarse mean color
            // without matching the spatial luminance/color structure of a real hero portrait.
            if min_ncc8 > -1.0 && descriptor_ncc(query_desc, candidate_desc) < min_ncc8 {
                continue;
            }
            best_by_hero
                .entry(*hero_id)
                .and_modify(|prev| {
                    if mean_l1 < prev.0 {
                        *prev = (mean_l1, query_saturation);
                    }
                })
                .or_insert((mean_l1, query_saturation));
        }
    }
}

fn match_descriptor_sets(
    entry_sets: &[&[(HeroId, [u8; DESC_LEN])]],
    query_desc: &[u8; DESC_LEN],
) -> Option<(HeroId, f32)> {
    let mut best_by_hero = HashMap::new();
    accumulate_descriptor_matches(
        entry_sets,
        query_desc,
        0.0,
        f32::MAX,
        -1.0,
        &mut best_by_hero,
    );
    best_by_hero
        .into_iter()
        .map(|(hero_id, (diff, _))| (hero_id, diff))
        .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.0.cmp(&b.0.0)))
}

/// Precomputed 8x4 cell-averaged RGB portrait catalog for all 127 Dota 2 heroes.
#[derive(Debug, Clone)]
pub struct HeroVisionCatalog {
    full_entries: Vec<(HeroId, [u8; DESC_LEN])>,
    inner_entries: Vec<(HeroId, [u8; DESC_LEN])>,
    upper_entries: Vec<(HeroId, [u8; DESC_LEN])>,
    ingame_core_entries: Vec<(HeroId, [u8; DESC_LEN])>,
    ingame_upper_core_entries: Vec<(HeroId, [u8; DESC_LEN])>,
}

impl HeroVisionCatalog {
    pub fn from_embedded() -> Option<Self> {
        let atlas = parse_hero_atlas()?;
        let (hw, hh) = (atlas.hero_width, atlas.hero_height);
        let rect_specs = [
            (0, 0, hw, hh),
            (4, 2, hw.saturating_sub(8), hh.saturating_sub(4)),
            (4, 2, hw.saturating_sub(8), 22.min(hh.saturating_sub(2))),
            (5, 2, hw.saturating_sub(13), hh.saturating_sub(8)),
            (5, 2, hw.saturating_sub(10), 20.min(hh.saturating_sub(2))),
        ];

        let alt_atlas = crate::hero_alt_atlas::parse_hero_alt_atlas()?;
        if alt_atlas.width != hw || alt_atlas.height != hh {
            return None;
        }
        let total_entries = atlas.heroes.len() + alt_atlas.entries.len();
        let mut buckets: [Vec<(HeroId, [u8; DESC_LEN])>; 5] =
            std::array::from_fn(|_| Vec::with_capacity(total_entries));

        for (hero_id, rgb) in atlas.heroes.into_iter().chain(alt_atlas.entries) {
            for ((x, y, width, height), entries) in rect_specs.into_iter().zip(&mut buckets) {
                let rect = TopBarSlotRect {
                    x,
                    y,
                    width,
                    height,
                };
                if let Some((desc, _)) = compute_descriptor_8x4(&rgb, hw, hh, rect) {
                    entries.push((hero_id, desc));
                }
            }
        }

        for entries in &mut buckets {
            entries.sort_by_key(|(id, _)| id.0);
        }
        let [
            full_entries,
            inner_entries,
            upper_entries,
            ingame_core_entries,
            ingame_upper_core_entries,
        ] = buckets;

        Some(Self {
            full_entries,
            inner_entries,
            upper_entries,
            ingame_core_entries,
            ingame_upper_core_entries,
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
        let (query_desc, mean_intensity) =
            compute_descriptor_8x4(frame_rgb, frame_w, frame_h, rect)?;
        if mean_intensity < DARK_INTENSITY_THRESHOLD {
            return None;
        }
        let (best_hero, best_diff) = match_descriptor_sets(&[&self.full_entries], &query_desc)?;
        if best_diff < MATCH_L1_THRESHOLD {
            Some(best_hero)
        } else {
            None
        }
    }

    fn match_slot_candidates_with_layout(
        &self,
        frame_rgb: &[u8],
        frame_w: usize,
        frame_h: usize,
        rect: TopBarSlotRect,
        layout: TopBarLayout,
        max_candidates: usize,
    ) -> Vec<(HeroId, f32, f32)> {
        type DescriptorSets<'a> = &'a [&'a [(HeroId, [u8; DESC_LEN])]];

        let passes: [Option<(TopBarSlotRect, DescriptorSets<'_>, f32)>; 2] = match layout {
            TopBarLayout::SyntheticExact => [
                Some((rect, &[&self.full_entries], MATCH_L1_THRESHOLD)),
                None,
            ],
            TopBarLayout::PickPhase => [
                Some((
                    rect,
                    &[&self.inner_entries, &self.full_entries, &self.upper_entries],
                    PICKPHASE_L1_THRESHOLD,
                )),
                Some((
                    pickphase_upper_core_rect(rect),
                    &[&self.upper_entries],
                    PICKPHASE_UPPER_CORE_L1_THRESHOLD,
                )),
            ],
            TopBarLayout::InGame => [
                Some((
                    ingame_inner_core_rect(rect),
                    &[&self.inner_entries, &self.ingame_core_entries],
                    INGAME_L1_THRESHOLD,
                )),
                Some((
                    ingame_upper_core_rect(rect),
                    &[&self.ingame_upper_core_entries],
                    INGAME_UPPER_CORE_L1_THRESHOLD,
                )),
            ],
        };

        let mut best_by_hero: HashMap<HeroId, (f32, f32)> = HashMap::new();
        for (query_rect, entry_sets, threshold) in passes.into_iter().flatten() {
            let Some((query_desc, mean_intensity)) =
                compute_descriptor_8x4(frame_rgb, frame_w, frame_h, query_rect)
            else {
                continue;
            };
            if mean_intensity < DARK_INTENSITY_THRESHOLD {
                continue;
            }

            let Some(ms) = slot_portrait_saturation_if_detailed(layout, &query_desc) else {
                continue;
            };

            accumulate_descriptor_matches(
                entry_sets,
                &query_desc,
                ms,
                threshold,
                MIN_MATCH_NCC_8,
                &mut best_by_hero,
            );
        }

        let mut ranked: Vec<(HeroId, f32, f32)> = best_by_hero
            .into_iter()
            .map(|(hero_id, (diff, sat))| (hero_id, diff, sat))
            .collect();
        ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.0.cmp(&b.0.0)));

        // A weak near-tie is not identity evidence: interior overlays or non-HUD chrome can shift
        // the nearest RGB template to an unrelated hero. Abstain before collision recovery can
        // promote indistinguishable alternatives, while keeping strong matches intact.
        let weak_cutoff = match layout {
            TopBarLayout::InGame => Some(26.0),
            TopBarLayout::PickPhase => Some(24.0),
            TopBarLayout::SyntheticExact => None,
        };
        if let Some(cutoff) = weak_cutoff
            && ranked.len() >= 2
            && ranked[0].1 > cutoff
            && ranked[1].1 - ranked[0].1 < 2.0
        {
            return Vec::new();
        }
        ranked.truncate(max_candidates);
        ranked
    }
}

fn slot_portrait_saturation_if_detailed(
    layout: TopBarLayout,
    query_desc: &[u8; DESC_LEN],
) -> Option<f32> {
    let (ov, os) = descriptor_region_stats(query_desc, &[0, 1, 2, 3], &[0, 1, 6, 7]);
    let (tv, ms) = descriptor_region_stats(query_desc, &[0, 1, 2, 3], &[0, 1, 2, 3, 4, 5, 6, 7]);
    if ms < MIN_DESCRIPTOR_SATURATION || os < MIN_OUTER_SATURATION {
        return None;
    }

    let common_edge = ov >= 13.5
        || (ov >= 12.5 && tv >= 18.0 && os >= 20.0)
        || (ov >= 12.0 && ms >= 28.0 && os >= 28.0);
    let has_detail = match layout {
        TopBarLayout::SyntheticExact => tv >= 12.5 && ov >= 11.0,
        // PickPhase captures both nameplate-cropped upper cards and full/inner 16:9 slots.
        TopBarLayout::PickPhase => tv >= 13.5 && (common_edge || (ov >= 7.5 && os >= 35.0)),
        TopBarLayout::InGame => {
            !has_flat_occluded_border(query_desc)
                && tv >= 13.5
                && os >= 10.5
                && (common_edge || (ov >= 9.5 && os >= 35.0))
        }
    };
    has_detail.then_some(ms)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TopBarLayout {
    SyntheticExact,
    PickPhase,
    InGame,
}

/// Game-phase policy controlling which Dota 2 top-bar layouts vision evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VisionCaptureMode {
    /// Evaluate all top-bar layouts (default when GSI rules state is unavailable).
    Any = 0,
    /// Hero selection / strategy phase: skip `InGame` HUD matching.
    DraftOnly = 1,
    /// Loading, lobby, post-game, or disconnected: skip frame capture.
    Suppressed = 2,
}

impl VisionCaptureMode {
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::DraftOnly,
            2 => Self::Suppressed,
            _ => Self::Any,
        }
    }
}

/// Captured screen frame in tightly packed 24-bit RGB (`width * height * 3` bytes).
#[derive(Debug, Clone)]
pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    /// Full pre-crop window height in pixels when `height` only carries the top-bar strip.
    pub source_height: Option<usize>,
    /// Desktop-coordinate top-bar capture strip rectangle `(x, y, width, height)` when known.
    pub capture_strip_rect: Option<(f32, f32, f32, f32)>,
    pub rgb: Vec<u8>,
}

impl RgbFrame {
    /// Creates an `RgbFrame` without an explicit pre-crop window height.
    pub fn new(width: usize, height: usize, rgb: Vec<u8>) -> Self {
        Self {
            width,
            height,
            source_height: None,
            capture_strip_rect: None,
            rgb,
        }
    }

    /// Creates a top-bar cropped `RgbFrame` preserving the original window height for layout scaling.
    pub fn with_source_height(
        width: usize,
        height: usize,
        source_height: usize,
        rgb: Vec<u8>,
    ) -> Self {
        Self {
            width,
            height,
            source_height: Some(source_height),
            capture_strip_rect: None,
            rgb,
        }
    }

    /// Attaches the desktop-coordinate top-bar capture strip rectangle `(x, y, width, height)`.
    pub fn with_capture_strip_rect(mut self, rect: (f32, f32, f32, f32)) -> Self {
        self.capture_strip_rect = Some(rect);
        self
    }
}

/// Computes the desktop-coordinate top-bar capture strip rectangle `(x, y, width, height)`
/// for a Dota 2 window at `(win_x, win_y)` with dimensions `(win_w, win_h)`.
pub fn compute_top_bar_capture_strip_rect(
    win_x: i32,
    win_y: i32,
    win_w: u32,
    win_h: u32,
) -> Option<(f32, f32, f32, f32)> {
    if win_w == 0 || win_h == 0 {
        return None;
    }
    let strip_h = ((win_h as usize * 8).div_ceil(100))
        .max(60)
        .min(win_h as usize) as f32;
    Some((win_x as f32, win_y as f32, win_w as f32, strip_h))
}

/// Parses `xrandr --listmonitors` output into desktop monitor rectangles `(x, y, width, height)`.
pub fn parse_xrandr_listmonitors(output: &str) -> Vec<(f32, f32, f32, f32)> {
    let mut monitors = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("Monitors:") {
            continue;
        }
        for token in trimmed.split_whitespace() {
            if let Some(rect) = parse_xrandr_monitor_geometry(token) {
                monitors.push(rect);
                break;
            }
        }
    }
    monitors
}

fn parse_xrandr_monitor_geometry(token: &str) -> Option<(f32, f32, f32, f32)> {
    let (w_part, rest) = token.split_once('x')?;
    let w_str = w_part.split('/').next()?;
    let width: u32 = w_str.parse().ok()?;
    if width == 0 {
        return None;
    }
    let offset_start = rest.find(['+', '-'])?;
    let h_part = &rest[..offset_start];
    let offsets = &rest[offset_start..];
    let h_str = h_part.split('/').next()?;
    let height: u32 = h_str.parse().ok()?;
    if height == 0 {
        return None;
    }
    let second_sign = offsets[1..].find(['+', '-']).map(|idx| idx + 1)?;
    let x: i32 = offsets[..second_sign].parse().ok()?;
    let y: i32 = offsets[second_sign..].parse().ok()?;
    Some((x as f32, y as f32, width as f32, height as f32))
}

/// Queries native desktop monitor rectangles `(x, y, width, height)` when available.
pub fn detect_native_monitor_rects() -> Vec<(f32, f32, f32, f32)> {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_none() {
            return Vec::new();
        }
        let Ok(output) = std::process::Command::new("xrandr")
            .arg("--listmonitors")
            .output()
        else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        parse_xrandr_listmonitors(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(target_os = "windows")]
    {
        let Ok(monitors) = xcap::Monitor::all() else {
            return Vec::new();
        };
        monitors
            .into_iter()
            .filter_map(|m| {
                let x = m.x().ok()?;
                let y = m.y().ok()?;
                let w = m.width().ok()?;
                let h = m.height().ok()?;
                if w == 0 || h == 0 {
                    return None;
                }
                Some((x as f32, y as f32, w as f32, h as f32))
            })
            .collect()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Vec::new()
    }
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
    let width = std::str::from_utf8(next_token(&mut pos)?)
        .ok()?
        .parse::<usize>()
        .ok()?;
    let height = std::str::from_utf8(next_token(&mut pos)?)
        .ok()?
        .parse::<usize>()
        .ok()?;
    let maxval = std::str::from_utf8(next_token(&mut pos)?)
        .ok()?
        .parse::<usize>()
        .ok()?;
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

    Some(RgbFrame::new(width, height, bytes[pos..end].to_vec()))
}
