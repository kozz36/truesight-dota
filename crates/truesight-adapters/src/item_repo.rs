use crate::item_catalog_data::{EMBEDDED_ITEM_CATALOG, StaticItemRow};
use std::collections::HashMap;
use truesight_core::domain::{
    Bracket, HeroId, HeroItemStat, ItemCatalogEntry, ItemId, ItemPhase, Role,
    all_referenced_counter_item_ids, is_counter_item_eligible_for_role,
};
use truesight_core::ports::{ItemMetaRepository, PortError};

/// Embedded binary RGB atlas of official Dota 2 item icons (`TSIA` header, `44x32` RGB per item).
pub const ITEM_ATLAS_BYTES: &[u8] = include_bytes!("../assets/item_atlas.bin");

/// Decoded item icon atlas indexed by canonical [`ItemId`].
#[derive(Debug, Clone)]
pub struct DecodedItemAtlas {
    pub item_width: usize,
    pub item_height: usize,
    pub items: HashMap<ItemId, Vec<u8>>,
}

/// Decodes the embedded binary RGB item atlas (`TSIA` header + `u16` IDs + RGB pixels).
pub fn parse_item_atlas() -> Option<DecodedItemAtlas> {
    if ITEM_ATLAS_BYTES.len() < 10 || &ITEM_ATLAS_BYTES[0..4] != b"TSIA" {
        return None;
    }
    let count = u16::from_le_bytes([ITEM_ATLAS_BYTES[4], ITEM_ATLAS_BYTES[5]]) as usize;
    let item_width = u16::from_le_bytes([ITEM_ATLAS_BYTES[6], ITEM_ATLAS_BYTES[7]]) as usize;
    let item_height = u16::from_le_bytes([ITEM_ATLAS_BYTES[8], ITEM_ATLAS_BYTES[9]]) as usize;
    let item_len = item_width.checked_mul(item_height)?.checked_mul(3)?;
    if item_len == 0 {
        return None;
    }

    let mut offset = 10usize;
    let mut items = HashMap::with_capacity(count);
    for _ in 0..count {
        let id_end = offset.checked_add(2)?;
        if id_end > ITEM_ATLAS_BYTES.len() {
            return None;
        }
        let iid = u16::from_le_bytes([ITEM_ATLAS_BYTES[offset], ITEM_ATLAS_BYTES[offset + 1]]);
        offset = id_end;
        let frame_end = offset.checked_add(item_len)?;
        if frame_end > ITEM_ATLAS_BYTES.len() {
            return None;
        }
        items.insert(ItemId(iid), ITEM_ATLAS_BYTES[offset..frame_end].to_vec());
        offset = frame_end;
    }

    Some(DecodedItemAtlas {
        item_width,
        item_height,
        items,
    })
}

/// Returns every canonical [`ItemId`] included in the embedded offline item catalog.
pub fn embedded_item_ids() -> Vec<ItemId> {
    EMBEDDED_ITEM_CATALOG
        .iter()
        .map(|row| ItemId(row.id))
        .collect()
}

/// Cached or test-injected item build override keyed by `(hero_id, optional role, optional bracket)`.
pub type HeroItemStatsOverride = (HeroId, Option<Role>, Option<Bracket>, Vec<HeroItemStat>);

/// Offline-first item catalog and hero build repository backed by embedded snapshots and optional live cache overrides.
#[derive(Debug, Clone, Default)]
pub struct SeededItemMetaRepository {
    pub custom_catalog: Vec<ItemCatalogEntry>,
    pub custom_hero_stats: Vec<HeroItemStatsOverride>,
    pub active_patch: Option<String>,
}

impl SeededItemMetaRepository {
    /// Creates a repository initialized with the embedded item catalog and baseline progressions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up an item catalog entry by slug, stripping an optional `"item_"` prefix.
    pub fn item_by_slug(&self, slug: &str) -> Option<ItemCatalogEntry> {
        let trimmed = slug.trim();
        let canonical = trimmed.strip_prefix("item_").unwrap_or(trimmed);
        if canonical.is_empty() {
            return None;
        }
        if let Some(entry) = self
            .custom_catalog
            .iter()
            .find(|e| e.slug.eq_ignore_ascii_case(canonical))
        {
            return Some(entry.clone());
        }
        EMBEDDED_ITEM_CATALOG
            .iter()
            .find(|r| r.slug.eq_ignore_ascii_case(canonical))
            .map(StaticItemRow::to_entry)
    }

    /// Returns the active Dota 2 patch string if known from cached metadata.
    pub fn active_patch(&self) -> Option<&str> {
        self.active_patch.as_deref()
    }
}

impl ItemMetaRepository for SeededItemMetaRepository {
    fn item_catalog(&self, item_id: ItemId) -> Option<ItemCatalogEntry> {
        if let Some(entry) = self.custom_catalog.iter().find(|e| e.id == item_id) {
            return Some(entry.clone());
        }
        EMBEDDED_ITEM_CATALOG
            .iter()
            .find(|r| r.id == item_id.0)
            .map(StaticItemRow::to_entry)
    }

    fn hero_item_stats(
        &self,
        hero_id: HeroId,
        role: Role,
        bracket: Bracket,
    ) -> Result<Vec<HeroItemStat>, PortError> {
        let scale = bracket_timing_scale(bracket);
        if let Some((_, _, matched_bracket, stats)) = self
            .custom_hero_stats
            .iter()
            .find(|(hid, r, b, _)| *hid == hero_id && *r == Some(role) && *b == Some(bracket))
            .or_else(|| {
                self.custom_hero_stats
                    .iter()
                    .find(|(hid, r, b, _)| *hid == hero_id && *r == Some(role) && b.is_none())
            })
            .or_else(|| {
                self.custom_hero_stats
                    .iter()
                    .find(|(hid, r, b, _)| *hid == hero_id && r.is_none() && *b == Some(bracket))
            })
            .or_else(|| {
                self.custom_hero_stats
                    .iter()
                    .find(|(hid, r, b, _)| *hid == hero_id && r.is_none() && b.is_none())
            })
        {
            let apply_fallback_scale = matched_bracket.is_none();
            let mut out: Vec<HeroItemStat> = stats
                .iter()
                .copied()
                .filter(|stat| {
                    stat.phase == ItemPhase::Starting
                        || stat.phase == ItemPhase::Situational
                        || is_valid_timed_progression_item_for_phase(
                            self,
                            stat.item_id,
                            stat.phase,
                        )
                })
                .map(|mut stat| {
                    if apply_fallback_scale
                        && matches!(
                            stat.phase,
                            ItemPhase::Early | ItemPhase::Core | ItemPhase::Late
                        )
                    {
                        stat.avg_time_mins = (stat.avg_time_mins * scale).max(1.0);
                    }
                    stat
                })
                .collect();
            append_counter_situational_candidates(&mut out, role, scale);
            return Ok(out);
        }

        Ok(build_embedded_hero_stats(hero_id, role, bracket))
    }
}

/// Returns `true` if `item_id` represents a completed build target, a standalone non-combinable
/// progression item, or an Early/Core high-value stepping stone (`Ultimate Orb`, `Sacred Relic`,
/// `Hyperstone`) eligible for `phase` rather than raw cheap stats or consumables.
pub(crate) fn is_valid_timed_progression_item_for_phase(
    repo: &SeededItemMetaRepository,
    item_id: ItemId,
    phase: ItemPhase,
) -> bool {
    // Exclude basic raw stats/ingredients (`Ogre Axe`, `Blade of Alacrity`, `Oblivion Staff`,
    // `Perseverance`, `Cornucopia`), consumables, wards, smokes, and detection items.
    if matches!(
        item_id.0,
        2..=10
            | 12..=23
            | 25..=34
            | 38..=40
            | 42..=46
            | 56..=61
            | 67
            | 69
            | 188
            | 216
            | 218
            | 237
            | 240
            | 265
            | 1122
            | 1123
            | 1125
            | 1575
            | 1802
            | 1847..=1849
            | 1872
    ) {
        return false;
    }
    let Some(entry) = repo.item_catalog(item_id) else {
        return false;
    };
    if (entry.cost == 0 && entry.is_consumable_drop) || entry.is_recipe {
        return false;
    }
    let allow_high_value_early_core =
        matches!(phase, ItemPhase::Early | ItemPhase::Core) && matches!(item_id.0, 24 | 54 | 55);
    allow_high_value_early_core
        || matches!(item_id.0, 1 | 37 | 41 | 609)
        || !entry.components.is_empty()
        || EMBEDDED_ITEM_CATALOG
            .iter()
            .any(|row| row.id == item_id.0 && !row.components.is_empty())
}

/// Strict timed progression item check used for untimed/noisy feeds (`Late` strictness).
pub(crate) fn is_valid_timed_progression_item(
    repo: &SeededItemMetaRepository,
    item_id: ItemId,
) -> bool {
    is_valid_timed_progression_item_for_phase(repo, item_id, ItemPhase::Late)
}

fn stat(
    item_id: u16,
    phase: ItemPhase,
    avg_time_mins: f32,
    matches: u32,
    wins: u32,
) -> HeroItemStat {
    HeroItemStat {
        item_id: ItemId(item_id),
        phase,
        avg_time_mins,
        match_count: matches,
        win_count: wins,
    }
}

fn bracket_timing_scale(bracket: Bracket) -> f32 {
    match bracket {
        Bracket::Herald => 1.18,
        Bracket::Guardian => 1.13,
        Bracket::Crusader => 1.08,
        Bracket::Archon => 1.03,
        Bracket::Legend => 1.00,
        Bracket::Ancient => 0.96,
        Bracket::Divine => 0.92,
        Bracket::Immortal => 0.89,
        _ => 1.00,
    }
}

fn append_counter_situational_candidates(
    stats: &mut Vec<HeroItemStat>,
    role: Role,
    time_scale: f32,
) {
    for &counter_id in all_referenced_counter_item_ids() {
        if !is_counter_item_eligible_for_role(counter_id, role)
            || stats.iter().any(|s| s.item_id == counter_id)
        {
            continue;
        }
        let base_time = match counter_id.0 {
            30 | 40 | 43 => 14.0,
            267 | 210 | 226 | 242 | 90 => 22.0,
            _ => 27.0,
        };
        stats.push(HeroItemStat {
            item_id: counter_id,
            phase: ItemPhase::Situational,
            avg_time_mins: base_time * time_scale,
            match_count: 220,
            win_count: 114,
        });
    }
}

fn build_embedded_hero_stats(hero_id: HeroId, role: Role, bracket: Bracket) -> Vec<HeroItemStat> {
    let scale = bracket_timing_scale(bracket);
    let mut stats = match (hero_id.0, role) {
        // Anti-Mage (1): BF -> Manta -> Basher/Skadi -> Abyssal/Butterfly
        (1, Role::Carry) => vec![
            stat(11, ItemPhase::Starting, 0.0, 1200, 640),
            stat(44, ItemPhase::Starting, 0.0, 1150, 610),
            stat(14, ItemPhase::Starting, 0.0, 950, 505),
            stat(16, ItemPhase::Starting, 0.0, 900, 480),
            stat(63, ItemPhase::Early, 6.5 * scale, 1400, 740),
            stat(36, ItemPhase::Early, 5.5 * scale, 950, 500),
            stat(145, ItemPhase::Core, 14.5 * scale, 1350, 735),
            stat(147, ItemPhase::Core, 21.5 * scale, 1300, 725),
            stat(160, ItemPhase::Late, 28.5 * scale, 780, 445),
            stat(139, ItemPhase::Late, 29.5 * scale, 740, 430),
            stat(143, ItemPhase::Core, 25.0 * scale, 680, 380),
            stat(208, ItemPhase::Late, 37.0 * scale, 620, 365),
        ],
        // Axe (2): Phase -> Blink -> Blade Mail/Vanguard -> BKB -> Shiva/Assault
        (2, Role::Offlane) => vec![
            stat(11, ItemPhase::Starting, 0.0, 1000, 520),
            stat(44, ItemPhase::Starting, 0.0, 1100, 575),
            stat(73, ItemPhase::Early, 5.0 * scale, 850, 450),
            stat(50, ItemPhase::Early, 7.0 * scale, 1200, 640),
            stat(125, ItemPhase::Early, 8.5 * scale, 700, 375),
            stat(1, ItemPhase::Core, 12.5 * scale, 1450, 790),
            stat(127, ItemPhase::Core, 16.5 * scale, 1320, 720),
            stat(116, ItemPhase::Core, 23.0 * scale, 1050, 580),
            stat(119, ItemPhase::Late, 30.0 * scale, 680, 390),
            stat(112, ItemPhase::Late, 33.0 * scale, 540, 310),
        ],
        // Crystal Maiden (5): Tranquil -> Wand -> Glimmer -> Force -> BKB -> Aghs
        (5, Role::HardSupport | Role::SoftSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 1100, 580),
            stat(34, ItemPhase::Starting, 0.0, 950, 500),
            stat(16, ItemPhase::Starting, 0.0, 900, 470),
            stat(214, ItemPhase::Early, 7.0 * scale, 1250, 665),
            stat(36, ItemPhase::Early, 6.0 * scale, 1100, 585),
            stat(254, ItemPhase::Core, 16.0 * scale, 1180, 645),
            stat(102, ItemPhase::Core, 21.5 * scale, 980, 535),
            stat(609, ItemPhase::Core, 24.0 * scale, 720, 405),
            stat(116, ItemPhase::Late, 29.0 * scale, 650, 365),
            stat(108, ItemPhase::Late, 33.0 * scale, 600, 345),
        ],
        // Earthshaker (7): Arcane -> Blink -> Aghs -> Force/Eul -> Octarine
        (7, Role::SoftSupport | Role::Mid | Role::Offlane) => vec![
            stat(44, ItemPhase::Starting, 0.0, 1050, 545),
            stat(34, ItemPhase::Starting, 0.0, 920, 480),
            stat(16, ItemPhase::Starting, 0.0, 890, 460),
            stat(180, ItemPhase::Early, 7.5 * scale, 1250, 660),
            stat(36, ItemPhase::Early, 6.0 * scale, 1000, 525),
            stat(1, ItemPhase::Core, 14.5 * scale, 1420, 775),
            stat(108, ItemPhase::Core, 23.0 * scale, 1020, 565),
            stat(102, ItemPhase::Core, 22.0 * scale, 720, 395),
            stat(100, ItemPhase::Core, 21.0 * scale, 690, 380),
            stat(235, ItemPhase::Late, 31.5 * scale, 720, 415),
        ],
        // Juggernaut (8): Phase/Treads -> Maelstrom/BF -> Manta -> Butterfly/Mjollnir -> Abyssal
        (8, Role::Carry) => vec![
            stat(11, ItemPhase::Starting, 0.0, 1100, 580),
            stat(44, ItemPhase::Starting, 0.0, 1050, 555),
            stat(50, ItemPhase::Early, 6.5 * scale, 1150, 620),
            stat(63, ItemPhase::Early, 6.5 * scale, 920, 485),
            stat(166, ItemPhase::Core, 14.5 * scale, 1250, 690),
            stat(145, ItemPhase::Core, 15.0 * scale, 1050, 580),
            stat(147, ItemPhase::Core, 21.0 * scale, 1320, 735),
            stat(158, ItemPhase::Late, 28.0 * scale, 960, 550),
            stat(139, ItemPhase::Late, 31.0 * scale, 880, 515),
            stat(208, ItemPhase::Late, 34.0 * scale, 720, 420),
        ],
        // Pudge (14): Phase/Tranquil -> Blink -> Vanguard/Aghs -> Lotus/Heart
        (14, Role::SoftSupport | Role::Offlane | Role::HardSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 1050, 540),
            stat(34, ItemPhase::Starting, 0.0, 920, 475),
            stat(214, ItemPhase::Early, 7.0 * scale, 980, 515),
            stat(36, ItemPhase::Early, 5.5 * scale, 1040, 545),
            stat(1, ItemPhase::Core, 14.5 * scale, 1260, 675),
            stat(108, ItemPhase::Core, 23.0 * scale, 1080, 595),
            stat(102, ItemPhase::Core, 20.0 * scale, 780, 420),
            stat(114, ItemPhase::Late, 31.5 * scale, 690, 395),
            stat(226, ItemPhase::Late, 30.0 * scale, 640, 360),
        ],
        // Storm Spirit (17): Bottle/Treads -> Witch Blade -> Kaya & Sange -> BKB -> Parasma/Hex
        (17, Role::Mid) => vec![
            stat(77, ItemPhase::Starting, 0.0, 980, 495),
            stat(44, ItemPhase::Starting, 0.0, 960, 485),
            stat(41, ItemPhase::Early, 2.5 * scale, 1220, 625),
            stat(63, ItemPhase::Early, 7.0 * scale, 1180, 610),
            stat(534, ItemPhase::Core, 15.0 * scale, 1140, 615),
            stat(273, ItemPhase::Core, 20.0 * scale, 1080, 595),
            stat(116, ItemPhase::Core, 23.5 * scale, 980, 540),
            stat(1806, ItemPhase::Late, 29.0 * scale, 820, 470),
            stat(96, ItemPhase::Late, 32.5 * scale, 740, 435),
        ],
        // Sven (18): Treads -> Echo Sabre -> Blink/BKB -> Harpoon -> Daedalus
        (18, Role::Carry) => vec![
            stat(11, ItemPhase::Starting, 0.0, 1080, 560),
            stat(44, ItemPhase::Starting, 0.0, 1040, 540),
            stat(63, ItemPhase::Early, 6.5 * scale, 1280, 680),
            stat(252, ItemPhase::Core, 13.5 * scale, 1180, 645),
            stat(1, ItemPhase::Core, 17.5 * scale, 1120, 615),
            stat(116, ItemPhase::Core, 22.0 * scale, 1220, 675),
            stat(939, ItemPhase::Late, 27.5 * scale, 890, 510),
            stat(141, ItemPhase::Late, 31.5 * scale, 860, 505),
        ],
        // Lion (26) / Shadow Shaman (27) / Witch Doctor (30): Arcane/Tranquil -> Blink/Glimmer -> Shard -> Aghs
        (26 | 27 | 30, Role::SoftSupport | Role::HardSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 1060, 550),
            stat(34, ItemPhase::Starting, 0.0, 960, 500),
            stat(16, ItemPhase::Starting, 0.0, 900, 465),
            stat(180, ItemPhase::Early, 7.5 * scale, 1180, 625),
            stat(36, ItemPhase::Early, 6.0 * scale, 1040, 545),
            stat(1, ItemPhase::Core, 16.0 * scale, 1140, 625),
            stat(254, ItemPhase::Core, 17.5 * scale, 1080, 590),
            stat(609, ItemPhase::Core, 20.0 * scale, 1020, 565),
            stat(102, ItemPhase::Core, 23.0 * scale, 840, 460),
            stat(108, ItemPhase::Late, 29.5 * scale, 960, 545),
            stat(232, ItemPhase::Late, 30.5 * scale, 720, 405),
        ],
        // Riki (32): Carry vs SoftSupport
        (32, Role::Carry | Role::Mid) => vec![
            stat(11, ItemPhase::Starting, 0.0, 1020, 540),
            stat(44, ItemPhase::Starting, 0.0, 980, 520),
            stat(63, ItemPhase::Early, 6.5 * scale, 1220, 650),
            stat(75, ItemPhase::Early, 5.0 * scale, 880, 465),
            stat(174, ItemPhase::Core, 14.0 * scale, 1340, 735),
            stat(147, ItemPhase::Core, 20.5 * scale, 1080, 600),
            stat(108, ItemPhase::Core, 22.5 * scale, 960, 535),
            stat(1097, ItemPhase::Late, 28.5 * scale, 940, 545),
            stat(141, ItemPhase::Late, 31.5 * scale, 780, 455),
        ],
        (32, Role::SoftSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 980, 515),
            stat(34, ItemPhase::Starting, 0.0, 920, 480),
            stat(214, ItemPhase::Early, 7.0 * scale, 1020, 545),
            stat(609, ItemPhase::Core, 17.0 * scale, 1260, 700),
            stat(174, ItemPhase::Core, 19.5 * scale, 1040, 575),
            stat(1097, ItemPhase::Late, 29.5 * scale, 780, 445),
            stat(108, ItemPhase::Late, 32.0 * scale, 640, 365),
        ],
        // Beastmaster (38): Offlane / Mid
        (38, Role::Offlane | Role::Mid) => vec![
            stat(11, ItemPhase::Starting, 0.0, 960, 495),
            stat(44, ItemPhase::Starting, 0.0, 980, 505),
            stat(180, ItemPhase::Early, 7.5 * scale, 1040, 540),
            stat(50, ItemPhase::Early, 7.5 * scale, 980, 510),
            stat(164, ItemPhase::Core, 11.5 * scale, 1280, 710),
            stat(1, ItemPhase::Core, 16.5 * scale, 1120, 620),
            stat(108, ItemPhase::Core, 21.0 * scale, 1160, 645),
            stat(116, ItemPhase::Late, 26.5 * scale, 960, 535),
            stat(119, ItemPhase::Late, 31.5 * scale, 740, 425),
        ],
        // Faceless Void (41): Treads -> Maelstrom -> Manta -> BKB -> Mjollnir/Butterfly/Skadi
        (41, Role::Carry) => vec![
            stat(11, ItemPhase::Starting, 0.0, 1100, 560),
            stat(44, ItemPhase::Starting, 0.0, 1080, 550),
            stat(63, ItemPhase::Early, 6.5 * scale, 1320, 680),
            stat(166, ItemPhase::Core, 14.5 * scale, 1280, 675),
            stat(147, ItemPhase::Core, 21.0 * scale, 1160, 625),
            stat(158, ItemPhase::Late, 26.5 * scale, 1140, 630),
            stat(116, ItemPhase::Late, 25.0 * scale, 1080, 585),
            stat(139, ItemPhase::Late, 31.5 * scale, 840, 480),
        ],
        // Phantom Assassin (44): Treads -> BF -> Desolator/BKB -> Abyssal/Satanic
        (44, Role::Carry) => vec![
            stat(11, ItemPhase::Starting, 0.0, 1150, 595),
            stat(44, ItemPhase::Starting, 0.0, 1100, 570),
            stat(63, ItemPhase::Early, 7.0 * scale, 1280, 670),
            stat(145, ItemPhase::Core, 15.0 * scale, 1360, 730),
            stat(168, ItemPhase::Core, 20.5 * scale, 1220, 665),
            stat(116, ItemPhase::Core, 23.0 * scale, 1180, 645),
            stat(208, ItemPhase::Late, 30.0 * scale, 920, 530),
            stat(156, ItemPhase::Late, 33.0 * scale, 780, 450),
        ],
        // Dazzle (50): Arcane -> Solar Crest -> Glimmer/Force -> Shard -> Aghs/Octarine
        (50, Role::SoftSupport | Role::HardSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 1020, 530),
            stat(34, ItemPhase::Starting, 0.0, 940, 490),
            stat(180, ItemPhase::Early, 7.5 * scale, 1240, 650),
            stat(36, ItemPhase::Early, 6.0 * scale, 1040, 545),
            stat(229, ItemPhase::Core, 16.5 * scale, 1180, 645),
            stat(254, ItemPhase::Core, 18.5 * scale, 1060, 575),
            stat(269, ItemPhase::Core, 20.5 * scale, 880, 485),
            stat(609, ItemPhase::Core, 21.5 * scale, 980, 545),
            stat(102, ItemPhase::Core, 23.0 * scale, 820, 445),
            stat(235, ItemPhase::Late, 31.5 * scale, 720, 415),
        ],
        // Jakiro (64): Arcane/Tranquil -> Force/Glimmer -> Shard -> Atos/Gleipnir -> Aghs
        (64, Role::SoftSupport | Role::HardSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 1020, 530),
            stat(34, ItemPhase::Starting, 0.0, 920, 480),
            stat(180, ItemPhase::Early, 7.5 * scale, 1220, 635),
            stat(36, ItemPhase::Early, 6.0 * scale, 1000, 520),
            stat(100, ItemPhase::Core, 17.5 * scale, 1080, 585),
            stat(102, ItemPhase::Core, 19.0 * scale, 1020, 550),
            stat(254, ItemPhase::Core, 18.5 * scale, 940, 510),
            stat(609, ItemPhase::Core, 21.0 * scale, 920, 505),
            stat(108, ItemPhase::Late, 30.5 * scale, 840, 480),
        ],
        // Invoker (74): Midas/Treads -> Orchid/Vessel/Witch Blade -> Aghs -> BKB -> Hex
        (74, Role::Mid | Role::SoftSupport) => vec![
            stat(77, ItemPhase::Starting, 0.0, 1000, 515),
            stat(44, ItemPhase::Starting, 0.0, 980, 505),
            stat(65, ItemPhase::Early, 8.0 * scale, 980, 515),
            stat(63, ItemPhase::Early, 7.0 * scale, 1100, 580),
            stat(98, ItemPhase::Core, 16.5 * scale, 920, 500),
            stat(267, ItemPhase::Core, 15.5 * scale, 880, 480),
            stat(108, ItemPhase::Core, 22.5 * scale, 1150, 635),
            stat(116, ItemPhase::Late, 27.5 * scale, 960, 535),
            stat(96, ItemPhase::Late, 32.0 * scale, 680, 395),
        ],
        // Lycan (77): Treads -> Helm Dom -> Echo/Harpoon -> BKB -> Assault/Abyssal
        (77, Role::Offlane | Role::Carry | Role::Mid) => vec![
            stat(11, ItemPhase::Starting, 0.0, 980, 510),
            stat(44, ItemPhase::Starting, 0.0, 960, 500),
            stat(63, ItemPhase::Early, 7.0 * scale, 1120, 595),
            stat(164, ItemPhase::Core, 11.5 * scale, 1340, 740),
            stat(252, ItemPhase::Core, 16.0 * scale, 1080, 595),
            stat(939, ItemPhase::Core, 22.0 * scale, 1140, 640),
            stat(116, ItemPhase::Core, 24.0 * scale, 980, 545),
            stat(112, ItemPhase::Late, 30.0 * scale, 840, 485),
            stat(208, ItemPhase::Late, 33.5 * scale, 680, 395),
        ],
        // Shadow Demon (79): Arcane -> Glimmer/Force -> Shard -> Aether/Aghs -> Octarine
        (79, Role::SoftSupport | Role::HardSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 980, 500),
            stat(34, ItemPhase::Starting, 0.0, 920, 470),
            stat(180, ItemPhase::Early, 7.5 * scale, 1180, 610),
            stat(36, ItemPhase::Early, 6.0 * scale, 1020, 530),
            stat(254, ItemPhase::Core, 17.5 * scale, 1080, 585),
            stat(102, ItemPhase::Core, 19.5 * scale, 1040, 560),
            stat(232, ItemPhase::Core, 21.5 * scale, 890, 480),
            stat(609, ItemPhase::Core, 22.0 * scale, 880, 475),
            stat(108, ItemPhase::Late, 31.0 * scale, 820, 465),
        ],
        // Disruptor (87): Arcane -> Glimmer/Force -> Shard -> Blink -> Aghs
        (87, Role::HardSupport | Role::SoftSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 1060, 550),
            stat(34, ItemPhase::Starting, 0.0, 980, 510),
            stat(180, ItemPhase::Early, 7.5 * scale, 1260, 665),
            stat(36, ItemPhase::Early, 6.0 * scale, 1080, 565),
            stat(254, ItemPhase::Core, 16.5 * scale, 1140, 620),
            stat(102, ItemPhase::Core, 19.5 * scale, 1060, 575),
            stat(609, ItemPhase::Core, 21.0 * scale, 980, 540),
            stat(1, ItemPhase::Core, 22.5 * scale, 840, 460),
            stat(108, ItemPhase::Late, 30.5 * scale, 940, 535),
        ],
        // Nyx Assassin (88): SoftSupport vs Mid
        (88, Role::SoftSupport | Role::HardSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 1040, 550),
            stat(34, ItemPhase::Starting, 0.0, 960, 505),
            stat(16, ItemPhase::Starting, 0.0, 900, 475),
            stat(180, ItemPhase::Early, 7.5 * scale, 1340, 725),
            stat(36, ItemPhase::Early, 6.0 * scale, 1080, 575),
            stat(1107, ItemPhase::Core, 16.0 * scale, 1260, 700),
            stat(104, ItemPhase::Core, 17.5 * scale, 1180, 650),
            stat(1, ItemPhase::Core, 18.5 * scale, 1080, 595),
            stat(609, ItemPhase::Core, 21.0 * scale, 1020, 565),
            stat(100, ItemPhase::Core, 22.0 * scale, 840, 460),
            stat(108, ItemPhase::Late, 30.0 * scale, 920, 530),
            stat(176, ItemPhase::Late, 32.5 * scale, 780, 445),
        ],
        (88, Role::Mid | Role::Offlane) => vec![
            stat(77, ItemPhase::Starting, 0.0, 940, 495),
            stat(44, ItemPhase::Starting, 0.0, 960, 505),
            stat(180, ItemPhase::Early, 7.0 * scale, 1180, 635),
            stat(104, ItemPhase::Core, 14.5 * scale, 1280, 710),
            stat(1107, ItemPhase::Core, 16.5 * scale, 1160, 645),
            stat(1, ItemPhase::Core, 19.0 * scale, 1020, 565),
            stat(108, ItemPhase::Late, 27.5 * scale, 920, 530),
            stat(176, ItemPhase::Late, 30.5 * scale, 860, 495),
        ],
        // Medusa (94): Treads -> Maelstrom/Manta -> Butterfly -> Skadi/Daedalus
        (94, Role::Carry) => vec![
            stat(11, ItemPhase::Starting, 0.0, 1020, 520),
            stat(44, ItemPhase::Starting, 0.0, 980, 500),
            stat(63, ItemPhase::Early, 6.5 * scale, 1240, 635),
            stat(147, ItemPhase::Core, 16.5 * scale, 1340, 715),
            stat(166, ItemPhase::Core, 14.5 * scale, 1040, 545),
            stat(139, ItemPhase::Core, 24.0 * scale, 1180, 650),
            stat(160, ItemPhase::Late, 29.5 * scale, 1080, 600),
            stat(141, ItemPhase::Late, 32.5 * scale, 860, 485),
        ],
        // Troll Warlord (95): Phase -> Battle Fury -> Sange & Yasha / BKB -> Skadi/Satanic
        (95, Role::Carry) => vec![
            stat(11, ItemPhase::Starting, 0.0, 1080, 565),
            stat(44, ItemPhase::Starting, 0.0, 1040, 545),
            stat(75, ItemPhase::Early, 5.0 * scale, 900, 470),
            stat(50, ItemPhase::Early, 6.5 * scale, 1280, 680),
            stat(145, ItemPhase::Core, 15.0 * scale, 1320, 725),
            stat(154, ItemPhase::Core, 20.5 * scale, 1180, 655),
            stat(116, ItemPhase::Core, 23.0 * scale, 1220, 675),
            stat(156, ItemPhase::Late, 29.5 * scale, 940, 540),
            stat(160, ItemPhase::Late, 32.0 * scale, 840, 485),
        ],
        // Magnus (97): Offlane / Mid / SoftSupport
        (97, Role::Offlane | Role::Mid) => vec![
            stat(11, ItemPhase::Starting, 0.0, 980, 505),
            stat(44, ItemPhase::Starting, 0.0, 980, 505),
            stat(63, ItemPhase::Early, 6.5 * scale, 1180, 615),
            stat(1, ItemPhase::Core, 12.5 * scale, 1420, 775),
            stat(252, ItemPhase::Core, 16.5 * scale, 1160, 640),
            stat(939, ItemPhase::Core, 22.0 * scale, 1100, 615),
            stat(108, ItemPhase::Late, 27.5 * scale, 960, 545),
            stat(110, ItemPhase::Late, 33.5 * scale, 820, 475),
        ],
        (97, Role::SoftSupport | Role::HardSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 980, 505),
            stat(34, ItemPhase::Starting, 0.0, 920, 475),
            stat(180, ItemPhase::Early, 7.5 * scale, 1140, 595),
            stat(1, ItemPhase::Core, 14.0 * scale, 1320, 715),
            stat(102, ItemPhase::Core, 21.0 * scale, 1080, 585),
            stat(609, ItemPhase::Core, 22.5 * scale, 900, 490),
            stat(108, ItemPhase::Late, 30.5 * scale, 840, 475),
        ],
        // Grimstroke (121): Arcane -> Aether Lens -> Glimmer/Force -> Shard -> Aghs
        (121, Role::SoftSupport | Role::HardSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 1020, 530),
            stat(34, ItemPhase::Starting, 0.0, 940, 490),
            stat(180, ItemPhase::Early, 7.5 * scale, 1260, 665),
            stat(36, ItemPhase::Early, 6.0 * scale, 1040, 545),
            stat(232, ItemPhase::Core, 16.5 * scale, 1220, 665),
            stat(254, ItemPhase::Core, 18.5 * scale, 1080, 590),
            stat(102, ItemPhase::Core, 20.5 * scale, 1020, 555),
            stat(609, ItemPhase::Core, 21.5 * scale, 980, 540),
            stat(108, ItemPhase::Late, 30.5 * scale, 940, 535),
            stat(176, ItemPhase::Late, 33.0 * scale, 720, 410),
        ],
        // Void Spirit (126): Treads -> Echo Sabre/Orchid -> Aghs -> BKB -> Harpoon/Parasma
        (126, Role::Mid) => vec![
            stat(77, ItemPhase::Starting, 0.0, 980, 505),
            stat(44, ItemPhase::Starting, 0.0, 960, 495),
            stat(41, ItemPhase::Early, 2.5 * scale, 1180, 610),
            stat(63, ItemPhase::Early, 6.5 * scale, 1260, 660),
            stat(252, ItemPhase::Core, 14.5 * scale, 1180, 645),
            stat(98, ItemPhase::Core, 15.5 * scale, 1120, 610),
            stat(108, ItemPhase::Core, 21.0 * scale, 1240, 685),
            stat(116, ItemPhase::Late, 25.5 * scale, 1080, 595),
            stat(939, ItemPhase::Late, 28.0 * scale, 960, 545),
            stat(1806, ItemPhase::Late, 31.5 * scale, 780, 445),
        ],
        // Marci (136): Support vs Core
        (136, Role::SoftSupport | Role::HardSupport) => vec![
            stat(44, ItemPhase::Starting, 0.0, 980, 505),
            stat(34, ItemPhase::Starting, 0.0, 920, 475),
            stat(50, ItemPhase::Early, 7.0 * scale, 1140, 600),
            stat(36, ItemPhase::Early, 5.5 * scale, 1020, 535),
            stat(229, ItemPhase::Core, 16.0 * scale, 1180, 645),
            stat(1, ItemPhase::Core, 17.5 * scale, 1080, 590),
            stat(609, ItemPhase::Core, 21.0 * scale, 960, 525),
            stat(108, ItemPhase::Late, 30.0 * scale, 820, 470),
        ],
        (136, Role::Carry | Role::Offlane | Role::Mid) => vec![
            stat(11, ItemPhase::Starting, 0.0, 980, 505),
            stat(44, ItemPhase::Starting, 0.0, 980, 505),
            stat(50, ItemPhase::Early, 6.5 * scale, 1220, 645),
            stat(143, ItemPhase::Core, 15.5 * scale, 1240, 675),
            stat(116, ItemPhase::Core, 21.0 * scale, 1260, 695),
            stat(141, ItemPhase::Late, 29.0 * scale, 920, 525),
            stat(208, ItemPhase::Late, 32.0 * scale, 840, 480),
        ],
        _ => match role {
            Role::Carry => vec![
                stat(11, ItemPhase::Starting, 0.0, 1000, 520),
                stat(44, ItemPhase::Starting, 0.0, 1000, 520),
                stat(75, ItemPhase::Early, 5.0 * scale, 820, 430),
                stat(63, ItemPhase::Early, 7.0 * scale, 1200, 630),
                stat(166, ItemPhase::Core, 15.0 * scale, 920, 495),
                stat(147, ItemPhase::Core, 21.0 * scale, 900, 490),
                stat(116, ItemPhase::Core, 24.0 * scale, 950, 525),
                stat(141, ItemPhase::Late, 30.0 * scale, 700, 395),
                stat(160, ItemPhase::Late, 32.0 * scale, 650, 370),
            ],
            Role::Mid => vec![
                stat(77, ItemPhase::Starting, 0.0, 950, 490),
                stat(44, ItemPhase::Starting, 0.0, 950, 490),
                stat(41, ItemPhase::Early, 2.5 * scale, 1080, 560),
                stat(63, ItemPhase::Early, 7.0 * scale, 1100, 575),
                stat(98, ItemPhase::Core, 16.0 * scale, 860, 465),
                stat(1, ItemPhase::Core, 15.0 * scale, 840, 455),
                stat(116, ItemPhase::Core, 23.0 * scale, 960, 530),
                stat(108, ItemPhase::Late, 28.0 * scale, 780, 435),
                stat(119, ItemPhase::Late, 32.0 * scale, 600, 340),
            ],
            Role::Offlane => vec![
                stat(11, ItemPhase::Starting, 0.0, 950, 495),
                stat(44, ItemPhase::Starting, 0.0, 1000, 520),
                stat(73, ItemPhase::Early, 5.5 * scale, 900, 470),
                stat(50, ItemPhase::Early, 7.0 * scale, 1100, 580),
                stat(125, ItemPhase::Early, 9.0 * scale, 760, 405),
                stat(1, ItemPhase::Core, 14.0 * scale, 1100, 595),
                stat(116, ItemPhase::Core, 23.0 * scale, 940, 515),
                stat(226, ItemPhase::Late, 28.5 * scale, 660, 365),
                stat(112, ItemPhase::Late, 32.0 * scale, 580, 325),
            ],
            Role::SoftSupport => vec![
                stat(44, ItemPhase::Starting, 0.0, 1000, 520),
                stat(34, ItemPhase::Starting, 0.0, 920, 480),
                stat(180, ItemPhase::Early, 7.5 * scale, 1100, 580),
                stat(36, ItemPhase::Early, 6.0 * scale, 1000, 525),
                stat(102, ItemPhase::Core, 18.0 * scale, 980, 530),
                stat(100, ItemPhase::Core, 21.0 * scale, 860, 465),
                stat(254, ItemPhase::Core, 17.0 * scale, 820, 445),
                stat(609, ItemPhase::Core, 22.0 * scale, 760, 415),
                stat(108, ItemPhase::Late, 31.0 * scale, 600, 340),
            ],
            Role::HardSupport => vec![
                stat(44, ItemPhase::Starting, 0.0, 1050, 545),
                stat(34, ItemPhase::Starting, 0.0, 950, 495),
                stat(214, ItemPhase::Early, 7.5 * scale, 1120, 590),
                stat(36, ItemPhase::Early, 6.5 * scale, 1050, 550),
                stat(254, ItemPhase::Core, 17.5 * scale, 1020, 555),
                stat(102, ItemPhase::Core, 22.0 * scale, 940, 510),
                stat(232, ItemPhase::Core, 24.0 * scale, 680, 370),
                stat(108, ItemPhase::Late, 31.0 * scale, 540, 305),
            ],
            _ => Vec::new(),
        },
    };

    append_counter_situational_candidates(&mut stats, role, scale);
    stats
}
