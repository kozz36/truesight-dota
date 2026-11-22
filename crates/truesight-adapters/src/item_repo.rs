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

struct StaticItemRow {
    id: u16,
    slug: &'static str,
    display_name: &'static str,
    cost: u32,
    components: &'static [u16],
    is_recipe: bool,
    is_consumable_drop: bool,
}

impl StaticItemRow {
    fn to_entry(&self) -> ItemCatalogEntry {
        ItemCatalogEntry {
            id: ItemId(self.id),
            slug: self.slug.to_string(),
            display_name: self.display_name.to_string(),
            cost: self.cost,
            components: self.components.iter().copied().map(ItemId).collect(),
            is_recipe: self.is_recipe,
            is_consumable_drop: self.is_consumable_drop,
        }
    }
}

const fn item_row(
    id: u16,
    slug: &'static str,
    display_name: &'static str,
    cost: u32,
    components: &'static [u16],
) -> StaticItemRow {
    StaticItemRow {
        id,
        slug,
        display_name,
        cost,
        components,
        is_recipe: false,
        is_consumable_drop: false,
    }
}

const fn drop_row(
    id: u16,
    slug: &'static str,
    display_name: &'static str,
    components: &'static [u16],
) -> StaticItemRow {
    StaticItemRow {
        id,
        slug,
        display_name,
        cost: 0,
        components,
        is_recipe: false,
        is_consumable_drop: true,
    }
}

const EMBEDDED_ITEM_CATALOG: &[StaticItemRow] = &[
    item_row(1, "blink", "Blink Dagger", 2250, &[]),
    item_row(2, "blades_of_attack", "Blades of Attack", 450, &[]),
    item_row(3, "broadsword", "Broadsword", 1000, &[]),
    item_row(4, "chainmail", "Chainmail", 500, &[]),
    item_row(5, "claymore", "Claymore", 1350, &[]),
    item_row(6, "helm_of_iron_will", "Helm of Iron Will", 975, &[]),
    item_row(8, "mithril_hammer", "Mithril Hammer", 1600, &[]),
    item_row(9, "platemail", "Platemail", 1400, &[]),
    item_row(10, "quarterstaff", "Quarterstaff", 900, &[]),
    item_row(11, "quelling_blade", "Quelling Blade", 100, &[]),
    item_row(12, "ring_of_protection", "Ring of Protection", 175, &[]),
    item_row(13, "gauntlets", "Gauntlets of Strength", 140, &[]),
    item_row(14, "slippers", "Slippers of Agility", 140, &[]),
    item_row(15, "mantle", "Mantle of Intelligence", 140, &[]),
    item_row(16, "branches", "Iron Branch", 55, &[]),
    item_row(17, "belt_of_strength", "Belt of Strength", 450, &[]),
    item_row(18, "boots_of_elves", "Band of Elvenskin", 450, &[]),
    item_row(19, "robe", "Robe of the Magi", 450, &[]),
    item_row(20, "circlet", "Circlet", 155, &[]),
    item_row(21, "ogre_axe", "Ogre Axe", 1000, &[]),
    item_row(22, "blade_of_alacrity", "Blade of Alacrity", 1000, &[]),
    item_row(23, "staff_of_wizardry", "Staff of Wizardry", 1000, &[]),
    item_row(24, "ultimate_orb", "Ultimate Orb", 2800, &[]),
    item_row(25, "gloves", "Gloves of Haste", 450, &[]),
    item_row(26, "lifesteal", "Morbid Mask", 900, &[]),
    item_row(27, "ring_of_regen", "Ring of Regen", 175, &[]),
    item_row(28, "sobi_mask", "Sage's Mask", 175, &[]),
    item_row(29, "boots", "Boots of Speed", 500, &[]),
    item_row(30, "gem", "Gem of True Sight", 900, &[]),
    item_row(31, "cloak", "Cloak", 900, &[]),
    item_row(32, "talisman_of_evasion", "Talisman of Evasion", 1300, &[]),
    item_row(34, "magic_stick", "Magic Stick", 200, &[]),
    item_row(36, "magic_wand", "Magic Wand", 460, &[34, 16, 16]),
    item_row(37, "ghost", "Ghost Scepter", 1500, &[]),
    item_row(40, "dust", "Dust of Appearance", 80, &[]),
    item_row(41, "bottle", "Bottle", 675, &[]),
    item_row(43, "ward_sentry", "Sentry Ward", 50, &[]),
    item_row(44, "tango", "Tango", 90, &[]),
    item_row(46, "tpscroll", "Town Portal Scroll", 100, &[]),
    item_row(48, "travel_boots", "Boots of Travel", 2500, &[29]),
    item_row(50, "phase_boots", "Phase Boots", 1450, &[29, 4, 2]),
    item_row(51, "demon_edge", "Demon Edge", 2200, &[]),
    item_row(52, "eagle", "Eaglesong", 2800, &[]),
    item_row(53, "reaver", "Reaver", 2800, &[]),
    item_row(54, "relic", "Sacred Relic", 3400, &[]),
    item_row(55, "hyperstone", "Hyperstone", 2000, &[]),
    item_row(56, "ring_of_health", "Ring of Health", 700, &[]),
    item_row(57, "void_stone", "Void Stone", 700, &[]),
    item_row(58, "mystic_staff", "Mystic Staff", 2800, &[]),
    item_row(59, "energy_booster", "Energy Booster", 800, &[]),
    item_row(60, "point_booster", "Point Booster", 1200, &[]),
    item_row(61, "vitality_booster", "Vitality Booster", 1000, &[]),
    item_row(63, "power_treads", "Power Treads", 1400, &[29, 25, 17]),
    item_row(65, "hand_of_midas", "Hand of Midas", 2200, &[25]),
    item_row(67, "oblivion_staff", "Oblivion Staff", 1625, &[485, 19, 28]),
    item_row(69, "pers", "Perseverance", 1400, &[56, 57]),
    item_row(73, "bracer", "Bracer", 505, &[20, 13]),
    item_row(75, "wraith_band", "Wraith Band", 505, &[20, 14]),
    item_row(77, "null_talisman", "Null Talisman", 505, &[20, 15]),
    item_row(79, "mekansm", "Mekansm", 1775, &[94, 4]),
    item_row(81, "vladmir", "Vladmir's Offering", 2200, &[86, 88, 26, 2]),
    item_row(86, "buckler", "Buckler", 425, &[12]),
    item_row(88, "ring_of_basilius", "Ring of Basilius", 425, &[28]),
    item_row(90, "pipe", "Pipe of Insight", 3725, &[279, 31, 1848]),
    item_row(92, "urn_of_shadows", "Urn of Shadows", 875, &[28, 12, 20]),
    item_row(94, "headdress", "Headdress", 425, &[27]),
    item_row(96, "sheepstick", "Scythe of Vyse", 5200, &[58, 1802]),
    item_row(98, "orchid", "Orchid Malevolence", 3275, &[67, 5]),
    item_row(
        100,
        "cyclone",
        "Eul's Scepter of Divinity",
        2600,
        &[23, 57, 244],
    ),
    item_row(102, "force_staff", "Force Staff", 2200, &[23, 593]),
    item_row(104, "dagon", "Dagon", 3000, &[60, 261, 1849]),
    item_row(
        108,
        "ultimate_scepter",
        "Aghanim's Scepter",
        4200,
        &[60, 23, 21, 22],
    ),
    item_row(110, "refresher", "Refresher Orb", 5000, &[279, 1802]),
    item_row(112, "assault", "Assault Cuirass", 5125, &[86, 55, 9]),
    item_row(114, "heart", "Heart of Tarrasque", 5300, &[53, 279]),
    item_row(116, "black_king_bar", "Black King Bar", 4050, &[8, 21]),
    item_row(119, "shivas_guard", "Shiva's Guard", 4500, &[9, 1847, 1872]),
    item_row(123, "sphere", "Linken's Sphere", 4800, &[69, 24]),
    item_row(125, "vanguard", "Vanguard", 1700, &[61, 56]),
    item_row(127, "blade_mail", "Blade Mail", 2350, &[3, 1847]),
    item_row(129, "soul_booster", "Soul Booster", 3000, &[60, 61, 59]),
    item_row(133, "rapier", "Divine Rapier", 5600, &[54, 51]),
    item_row(
        135,
        "monkey_king_bar",
        "Monkey King Bar",
        5000,
        &[51, 485, 10],
    ),
    item_row(137, "radiance", "Radiance", 4700, &[54, 32]),
    item_row(139, "butterfly", "Butterfly", 5000, &[52, 32, 10]),
    item_row(141, "greater_crit", "Daedalus", 5200, &[149, 51]),
    item_row(143, "basher", "Skull Basher", 2875, &[8, 17]),
    item_row(145, "bfury", "Battle Fury", 3900, &[69, 3, 3, 11]),
    item_row(147, "manta", "Manta Style", 4550, &[170, 1122]),
    item_row(149, "lesser_crit", "Crystalys", 2000, &[5, 2]),
    item_row(151, "armlet", "Armlet of Mordiggian", 2500, &[6, 25, 2]),
    item_row(152, "invis_sword", "Shadow Blade", 3250, &[5, 485, 215]),
    item_row(154, "sange_and_yasha", "Sange and Yasha", 4000, &[162, 170]),
    item_row(156, "satanic", "Satanic", 5050, &[53, 5, 26]),
    item_row(158, "mjollnir", "Mjollnir", 5500, &[166, 55]),
    item_row(160, "skadi", "Eye of Skadi", 5900, &[24, 24, 1575]),
    item_row(162, "sange", "Sange", 2000, &[21, 17]),
    item_row(
        164,
        "helm_of_the_dominator",
        "Helm of the Dominator",
        2550,
        &[6, 261],
    ),
    item_row(166, "maelstrom", "Maelstrom", 2950, &[8, 25]),
    item_row(168, "desolator", "Desolator", 3500, &[8, 8, 240]),
    item_row(170, "yasha", "Yasha", 2000, &[22, 18]),
    item_row(174, "diffusal_blade", "Diffusal Blade", 2500, &[22, 19]),
    item_row(176, "ethereal_blade", "Ethereal Blade", 5200, &[24, 37]),
    item_row(178, "soul_ring", "Soul Ring", 805, &[12, 13, 13]),
    item_row(180, "arcane_boots", "Arcane Boots", 1500, &[29, 88, 1849]),
    item_row(181, "orb_of_venom", "Orb of Venom", 350, &[]),
    item_row(
        190,
        "veil_of_discord",
        "Veil of Discord",
        1700,
        &[473, 19, 593],
    ),
    item_row(206, "rod_of_atos", "Rod of Atos", 2250, &[23, 61]),
    item_row(208, "abyssal_blade", "Abyssal Blade", 6150, &[143, 236]),
    item_row(210, "heavens_halberd", "Heaven's Halberd", 2400, &[21, 6]),
    item_row(214, "tranquil_boots", "Tranquil Boots", 900, &[29, 244, 27]),
    item_row(215, "shadow_amulet", "Shadow Amulet", 900, &[]),
    item_row(225, "nullifier", "Nullifier", 4350, &[54, 1847]),
    item_row(226, "lotus_orb", "Lotus Orb", 3850, &[69, 9, 59]),
    item_row(229, "solar_crest", "Solar Crest", 2575, &[1128, 4, 244]),
    item_row(
        231,
        "guardian_greaves",
        "Guardian Greaves",
        4350,
        &[180, 79],
    ),
    item_row(232, "aether_lens", "Aether Lens", 2100, &[59, 57]),
    item_row(235, "octarine_core", "Octarine Core", 5100, &[129, 1802]),
    item_row(236, "dragon_lance", "Dragon Lance", 2000, &[22, 17]),
    item_row(237, "faerie_fire", "Faerie Fire", 65, &[]),
    item_row(240, "blight_stone", "Orb of Blight", 300, &[]),
    item_row(242, "crimson_guard", "Crimson Guard", 3725, &[125, 86, 6]),
    item_row(244, "wind_lace", "Wind Lace", 225, &[]),
    item_row(249, "silver_edge", "Silver Edge", 5700, &[152, 51]),
    item_row(250, "bloodthorn", "Bloodthorn", 6250, &[98, 67]),
    item_row(252, "echo_sabre", "Echo Sabre", 2700, &[21, 3, 57]),
    item_row(254, "glimmer_cape", "Glimmer Cape", 2150, &[215, 1848]),
    item_row(256, "aeon_disk", "Aeon Disk", 3000, &[61, 59]),
    item_row(259, "kaya", "Kaya", 2000, &[23, 19]),
    item_row(261, "crown", "Crown", 450, &[]),
    item_row(263, "hurricane_pike", "Hurricane Pike", 4550, &[102, 236]),
    item_row(267, "spirit_vessel", "Spirit Vessel", 2725, &[92, 1122]),
    item_row(269, "holy_locket", "Holy Locket", 2250, &[36, 261]),
    item_row(273, "kaya_and_sange", "Kaya and Sange", 4000, &[259, 162]),
    item_row(277, "yasha_and_kaya", "Yasha and Kaya", 4000, &[170, 259]),
    item_row(279, "ring_of_tarrasque", "Ring of Tarrasque", 1700, &[]),
    item_row(473, "voodoo_mask", "Voodoo Mask", 650, &[]),
    item_row(485, "blitz_knuckles", "Blitz Knuckles", 1000, &[]),
    item_row(534, "witch_blade", "Witch Blade", 2775, &[67, 4]),
    item_row(593, "fluffy_hat", "Fluffy Hat", 250, &[]),
    item_row(598, "mage_slayer", "Mage Slayer", 3100, &[69, 31, 2, 181]),
    item_row(609, "aghanims_shard", "Aghanim's Shard", 1400, &[]),
    item_row(610, "wind_waker", "Wind Waker", 6800, &[100, 58]),
    item_row(939, "harpoon", "Harpoon", 4700, &[252, 236]),
    item_row(1097, "disperser", "Disperser", 6100, &[174, 52]),
    item_row(1107, "phylactery", "Phylactery", 2600, &[69, 1122]),
    item_row(1122, "diadem", "Diadem", 1000, &[]),
    item_row(1128, "pavise", "Pavise", 1250, &[1849, 593, 12]),
    item_row(1466, "gungir", "Gleipnir", 4650, &[206, 60, 1872]),
    item_row(1575, "orb_of_frost", "Orb of Frost", 300, &[]),
    item_row(1802, "tiara_of_selemene", "Tiara of Selemene", 1700, &[]),
    item_row(1806, "devastator", "Parasma", 5975, &[534, 58]),
    item_row(1808, "angels_demise", "Khanda", 5600, &[1107, 129]),
    item_row(1847, "splintmail", "Splintmail", 950, &[]),
    item_row(1848, "shawl", "Shawl", 450, &[]),
    item_row(1849, "wizard_hat", "Wizard Hat", 250, &[]),
    item_row(1872, "chasm_stone", "Chasm Stone", 900, &[]),
    drop_row(4204, "famango", "Healing Lotus", &[]),
    drop_row(
        4205,
        "great_famango",
        "Great Healing Lotus",
        &[4204, 4204, 4204],
    ),
];

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
        if let Some((_, _, _, stats)) = self
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
                    .find(|(hid, r, b, _)| *hid == hero_id && r.is_none() && b.is_none())
            })
        {
            let mut out = stats.clone();
            append_counter_situational_candidates(&mut out, role, scale);
            return Ok(out);
        }

        Ok(build_embedded_hero_stats(hero_id, role, bracket))
    }
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
            stat(208, ItemPhase::Late, 35.5 * scale, 620, 365),
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
