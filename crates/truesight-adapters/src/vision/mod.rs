mod capture;
mod windows;
mod worker;
mod x11;

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
#[cfg(test)]
pub(crate) use x11::evaluate_x11_window_visibility_with;
pub use x11::{build_x11_import_args, is_dota2_xprop_output};

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
const INGAME_L1_THRESHOLD: f32 = 35.0;
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

fn descriptor_total_variance(desc: &[u8; DESC_LEN]) -> f32 {
    let total_cells = DESC_COLS * DESC_ROWS;
    let n = total_cells as f32;
    let (mut r_sum, mut g_sum, mut b_sum) = (0_f32, 0_f32, 0_f32);
    for cell in 0..total_cells {
        let idx = cell * 3;
        r_sum += f32::from(desc[idx]);
        g_sum += f32::from(desc[idx + 1]);
        b_sum += f32::from(desc[idx + 2]);
    }
    let (mr, mg, mb) = (r_sum / n, g_sum / n, b_sum / n);
    let mut var_sum = 0_f32;
    for cell in 0..total_cells {
        let idx = cell * 3;
        var_sum += (f32::from(desc[idx]) - mr).abs()
            + (f32::from(desc[idx + 1]) - mg).abs()
            + (f32::from(desc[idx + 2]) - mb).abs();
    }
    var_sum / (n * 3.0)
}

fn rank_descriptor_sets(
    entry_sets: &[&[(HeroId, [u8; DESC_LEN])]],
    query_desc: &[u8; DESC_LEN],
    threshold: f32,
    min_ncc8: f32,
    max_candidates: usize,
) -> Vec<(HeroId, f32)> {
    let mut best_by_hero: HashMap<HeroId, f32> = HashMap::new();

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
                    if mean_l1 < *prev {
                        *prev = mean_l1;
                    }
                })
                .or_insert(mean_l1);
        }
    }

    let mut ranked: Vec<(HeroId, f32)> = best_by_hero.into_iter().collect();
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.0.cmp(&b.0.0)));
    ranked.truncate(max_candidates);
    ranked
}

fn match_descriptor_sets(
    entry_sets: &[&[(HeroId, [u8; DESC_LEN])]],
    query_desc: &[u8; DESC_LEN],
) -> Option<(HeroId, f32)> {
    rank_descriptor_sets(entry_sets, query_desc, f32::MAX, -1.0, 1)
        .into_iter()
        .next()
}

/// Precomputed 8x4 cell-averaged RGB portrait catalog for all 127 Dota 2 heroes.
#[derive(Debug, Clone)]
pub struct HeroVisionCatalog {
    full_entries: Vec<(HeroId, [u8; DESC_LEN])>,
    inner_entries: Vec<(HeroId, [u8; DESC_LEN])>,
    upper_entries: Vec<(HeroId, [u8; DESC_LEN])>,
    ingame_core_entries: Vec<(HeroId, [u8; DESC_LEN])>,
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
        let ingame_core_rect = TopBarSlotRect {
            x: 5,
            y: 2,
            width: atlas.hero_width.saturating_sub(13),
            height: atlas.hero_height.saturating_sub(8),
        };

        let alt_atlas = crate::hero_alt_atlas::parse_hero_alt_atlas()?;
        if alt_atlas.width != atlas.hero_width || alt_atlas.height != atlas.hero_height {
            return None;
        }
        let total_entries = atlas.heroes.len() + alt_atlas.entries.len();

        let mut full_entries = Vec::with_capacity(total_entries);
        let mut inner_entries = Vec::with_capacity(total_entries);
        let mut upper_entries = Vec::with_capacity(total_entries);
        let mut ingame_core_entries = Vec::with_capacity(total_entries);

        for (hero_id, rgb) in atlas.heroes.into_iter().chain(alt_atlas.entries) {
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
            if let Some((desc, _)) =
                compute_descriptor_8x4(&rgb, atlas.hero_width, atlas.hero_height, ingame_core_rect)
            {
                ingame_core_entries.push((hero_id, desc));
            }
        }

        full_entries.sort_by_key(|(id, _)| id.0);
        inner_entries.sort_by_key(|(id, _)| id.0);
        upper_entries.sort_by_key(|(id, _)| id.0);
        ingame_core_entries.sort_by_key(|(id, _)| id.0);

        Some(Self {
            full_entries,
            inner_entries,
            upper_entries,
            ingame_core_entries,
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
        let query_rect = match layout {
            TopBarLayout::InGame => ingame_inner_core_rect(rect),
            TopBarLayout::SyntheticExact | TopBarLayout::PickPhase => rect,
        };
        let Some((query_desc, mean_intensity)) =
            compute_descriptor_8x4(frame_rgb, frame_w, frame_h, query_rect)
        else {
            return Vec::new();
        };
        if mean_intensity < DARK_INTENSITY_THRESHOLD {
            return Vec::new();
        }

        let (ov, os) = outer_column_stats(&query_desc);
        let tv = descriptor_total_variance(&query_desc);
        let ms = mean_descriptor_saturation(&query_desc);
        if ms < MIN_DESCRIPTOR_SATURATION || os < MIN_OUTER_SATURATION {
            return Vec::new();
        }

        type DescriptorSets<'a> = &'a [&'a [(HeroId, [u8; DESC_LEN])]];

        let saturated_dark_border = ov >= 12.0 && ms >= 28.0 && os >= 28.0;
        let (entry_sets, threshold, has_portrait_detail): (DescriptorSets<'_>, f32, bool) =
            match layout {
                TopBarLayout::SyntheticExact => (
                    &[&self.full_entries],
                    MATCH_L1_THRESHOLD,
                    tv >= 12.5 && ov >= 11.0,
                ),
                // PickPhase captures both nameplate-cropped upper cards and full/inner 16:9 slots.
                TopBarLayout::PickPhase => (
                    &[&self.upper_entries, &self.inner_entries, &self.full_entries],
                    PICKPHASE_L1_THRESHOLD,
                    tv >= 13.5
                        && (ov >= 13.5
                            || (ov >= 12.5 && tv >= 18.0 && os >= 20.0)
                            || saturated_dark_border
                            || (ov >= 7.5 && os >= 35.0)),
                ),
                TopBarLayout::InGame => (
                    &[&self.inner_entries, &self.ingame_core_entries],
                    INGAME_L1_THRESHOLD,
                    tv >= 13.5
                        && os >= 10.5
                        && (ov >= 13.5
                            || (ov >= 12.5 && tv >= 18.0 && os >= 20.0)
                            || saturated_dark_border
                            || (ov >= 9.5 && os >= 35.0)),
                ),
            };
        if !has_portrait_detail {
            return Vec::new();
        }

        let ranked = rank_descriptor_sets(
            entry_sets,
            &query_desc,
            threshold,
            MIN_MATCH_NCC_8,
            max_candidates,
        );
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
        ranked
            .into_iter()
            .map(|(hero_id, diff)| (hero_id, diff, ms))
            .collect()
    }
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
    pub rgb: Vec<u8>,
}

impl RgbFrame {
    /// Creates an `RgbFrame` without an explicit pre-crop window height.
    pub fn new(width: usize, height: usize, rgb: Vec<u8>) -> Self {
        Self {
            width,
            height,
            source_height: None,
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
            rgb,
        }
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
