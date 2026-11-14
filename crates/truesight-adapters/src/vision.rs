use std::collections::HashMap;
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

    let mut rad = [TopBarSlotRect {
        x: 0,
        y,
        width: slot_w,
        height: slot_h,
    }; 5];
    let mut dire = [TopBarSlotRect {
        x: 0,
        y,
        width: slot_w,
        height: slot_h,
    }; 5];

    for i in 0..5 {
        rad[i].x = rad_start_x + i * step;
        dire[i].x = dire_start_x + i * step;
    }

    (rad, dire)
}

const DESC_COLS: usize = 8;
const DESC_ROWS: usize = 4;
const DESC_LEN: usize = DESC_COLS * DESC_ROWS * 3;
const DARK_INTENSITY_THRESHOLD: f32 = 12.0;
const MATCH_L1_THRESHOLD: f32 = 25.0;

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
        let y1 = (rect.y + ((cy + 1) * rect.height) / DESC_ROWS).max(y0 + 1).min(rect.y + rect.height);
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

/// Precomputed 8x4 cell-averaged RGB portrait catalog for all 127 Dota 2 heroes.
#[derive(Debug, Clone)]
pub struct HeroVisionCatalog {
    entries: Vec<(HeroId, [u8; DESC_LEN])>,
}

impl HeroVisionCatalog {
    pub fn from_embedded() -> Option<Self> {
        let atlas = parse_hero_atlas()?;
        let rect = TopBarSlotRect {
            x: 0,
            y: 0,
            width: atlas.hero_width,
            height: atlas.hero_height,
        };
        let mut entries = Vec::with_capacity(atlas.heroes.len());
        for (hero_id, rgb) in atlas.heroes {
            if let Some((desc, _)) =
                compute_descriptor_8x4(&rgb, atlas.hero_width, atlas.hero_height, rect)
            {
                entries.push((hero_id, desc));
            }
        }
        entries.sort_by_key(|(id, _)| id.0);
        Some(Self { entries })
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

        let mut best_hero = None;
        let mut best_diff = f32::MAX;

        for (hero_id, candidate_desc) in &self.entries {
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

        if best_diff < MATCH_L1_THRESHOLD {
            best_hero
        } else {
            None
        }
    }
}

/// Captured screen frame in tightly packed 24-bit RGB (`width * height * 3` bytes).
#[derive(Debug, Clone)]
pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

/// Port abstraction for PipeWire / X11 / test frame capture backends.
pub trait ScreenFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError>;
}

/// Driving adapter matching the 10 Dota 2 top-bar hero portrait slots into `DraftEvent::PickDetected`.
pub struct VisionCaptureAdapter<S: ScreenFrameSource> {
    source: S,
    catalog: HeroVisionCatalog,
    ally_faction: Faction,
    last_ally_faction: Option<Faction>,
    last_radiant: [Option<HeroId>; 5],
    last_dire: [Option<HeroId>; 5],
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
        }
    }

    pub fn set_ally_faction(&mut self, faction: Faction) {
        self.ally_faction = faction;
    }
}

impl<S: ScreenFrameSource> DraftEventSource for VisionCaptureAdapter<S> {
    type Error = PortError;

    fn poll_events(&mut self) -> Result<Vec<DraftEvent>, Self::Error> {
        let Some(frame) = self.source.capture_frame()? else {
            return Ok(Vec::new());
        };

        let (rad_rects, dire_rects) = compute_topbar_slot_rects(frame.width, frame.height);
        let (rad_side, dire_side) = match self.ally_faction {
            Faction::Radiant => (Side::Allies, Side::Enemies),
            Faction::Dire => (Side::Enemies, Side::Allies),
        };
        let faction_changed = self.last_ally_faction != Some(self.ally_faction);

        let mut events = Vec::new();

        for slot in 0..5 {
            let detected =
                self.catalog
                    .match_portrait_crop(&frame.rgb, frame.width, frame.height, rad_rects[slot]);
            if let Some(hero_id) = detected {
                if faction_changed || self.last_radiant[slot] != Some(hero_id) {
                    events.push(DraftEvent::PickDetected {
                        side: rad_side,
                        slot,
                        hero_id,
                        role: Role::ALL.get(slot).copied(),
                    });
                }
            }
            self.last_radiant[slot] = detected;
        }

        for slot in 0..5 {
            let detected = self.catalog.match_portrait_crop(
                &frame.rgb,
                frame.width,
                frame.height,
                dire_rects[slot],
            );
            if let Some(hero_id) = detected {
                if faction_changed || self.last_dire[slot] != Some(hero_id) {
                    events.push(DraftEvent::PickDetected {
                        side: dire_side,
                        slot,
                        hero_id,
                        role: Role::ALL.get(slot).copied(),
                    });
                }
            }
            self.last_dire[slot] = detected;
        }

        self.last_ally_faction = Some(self.ally_faction);
        Ok(events)
    }
}
