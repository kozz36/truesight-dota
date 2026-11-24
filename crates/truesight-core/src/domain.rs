use core::fmt;
use std::sync::OnceLock;

pub const MAX_HEROES: usize = 160;
pub const TEAM_SIZE: usize = 5;

/// Compact zero-cost identifier for a Dota 2 hero (`0..MAX_HEROES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeroId(pub u16);

impl HeroId {
    #[inline]
    pub const fn as_index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Role {
    Carry,       // Pos 1 (Safe Lane)
    Mid,         // Pos 2 (Middle)
    Offlane,     // Pos 3 (Off Lane)
    SoftSupport, // Pos 4 (Roam / Soft Support)
    HardSupport, // Pos 5 (Hard Support)
}

impl Role {
    pub const ALL: [Self; 5] = [
        Self::Carry,
        Self::Mid,
        Self::Offlane,
        Self::SoftSupport,
        Self::HardSupport,
    ];

    #[inline]
    pub const fn index(self) -> usize {
        match self {
            Self::Carry => 0,
            Self::Mid => 1,
            Self::Offlane => 2,
            Self::SoftSupport => 3,
            Self::HardSupport => 4,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Carry => "Pos 1 · Carry",
            Self::Mid => "Pos 2 · Mid",
            Self::Offlane => "Pos 3 · Offlane",
            Self::SoftSupport => "Pos 4 · Soft Sup",
            Self::HardSupport => "Pos 5 · Hard Sup",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Bracket {
    Herald,
    Guardian,
    Crusader,
    Archon,
    Legend,
    Ancient,
    Divine,
    Immortal,
}

impl Bracket {
    pub const ALL: [Self; 8] = [
        Self::Herald,
        Self::Guardian,
        Self::Crusader,
        Self::Archon,
        Self::Legend,
        Self::Ancient,
        Self::Divine,
        Self::Immortal,
    ];

    #[inline]
    pub const fn index(self) -> usize {
        match self {
            Self::Herald => 0,
            Self::Guardian => 1,
            Self::Crusader => 2,
            Self::Archon => 3,
            Self::Legend => 4,
            Self::Ancient => 5,
            Self::Divine => 6,
            Self::Immortal => 7,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Herald => "Herald",
            Self::Guardian => "Guardian",
            Self::Crusader => "Crusader",
            Self::Archon => "Archon",
            Self::Legend => "Legend",
            Self::Ancient => "Ancient",
            Self::Divine => "Divine",
            Self::Immortal => "Immortal",
        }
    }

    /// Maps an OpenDota `/api/players/{id}` `rank_tier` (`10..=89`) to the matching medal bracket.
    ///
    /// Returns `None` for uncalibrated (`0`) or out-of-range values.
    #[inline]
    pub const fn from_rank_tier(rank_tier: u8) -> Option<Self> {
        match rank_tier / 10 {
            1 => Some(Self::Herald),
            2 => Some(Self::Guardian),
            3 => Some(Self::Crusader),
            4 => Some(Self::Archon),
            5 => Some(Self::Legend),
            6 => Some(Self::Ancient),
            7 => Some(Self::Divine),
            8 => Some(Self::Immortal),
            _ => None,
        }
    }

    /// Parses a persisted bracket label or numeric tier index (`"herald"`..`"immortal"`, `"1"`..`"8"`).
    pub fn parse_label(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "herald" => Some(Self::Herald),
            "2" | "guardian" => Some(Self::Guardian),
            "3" | "crusader" => Some(Self::Crusader),
            "4" | "archon" => Some(Self::Archon),
            "5" | "legend" => Some(Self::Legend),
            "6" | "ancient" => Some(Self::Ancient),
            "7" | "divine" => Some(Self::Divine),
            "8" | "immortal" => Some(Self::Immortal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Allies,
    Enemies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Faction {
    Radiant,
    Dire,
}

impl Faction {
    #[inline]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Radiant => Self::Dire,
            Self::Dire => Self::Radiant,
        }
    }

    #[inline]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Radiant => "RADIANT",
            Self::Dire => "DIRE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttackType {
    Melee,
    Ranged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftPick {
    pub hero_id: HeroId,
    pub role: Option<Role>,
}

/// Stack-allocated 5v5 draft state. Implements `Copy` for zero-allocation simulations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftState {
    pub allies: [Option<DraftPick>; TEAM_SIZE],
    pub enemies: [Option<DraftPick>; TEAM_SIZE],
    pub bans: [Option<HeroId>; 16],
    pub bracket: Bracket,
    pub target_role: Role,
    pub ally_faction: Faction,
}

impl Default for DraftState {
    fn default() -> Self {
        Self {
            allies: [None; TEAM_SIZE],
            enemies: [None; TEAM_SIZE],
            bans: [None; 16],
            bracket: Bracket::Legend,
            target_role: Role::Carry,
            ally_faction: Faction::Radiant,
        }
    }
}

impl DraftState {
    pub fn is_unavailable(&self, hero_id: HeroId) -> bool {
        self.allies.iter().flatten().any(|p| p.hero_id == hero_id)
            || self.enemies.iter().flatten().any(|p| p.hero_id == hero_id)
            || self.bans.iter().flatten().any(|&b| b == hero_id)
    }

    pub fn set_ally(&mut self, slot: usize, pick: Option<DraftPick>) -> Result<(), DomainError> {
        if slot >= TEAM_SIZE {
            return Err(DomainError::InvalidSlot(slot));
        }
        if let Some(p) = pick {
            if self.is_unavailable(p.hero_id)
                && self.allies[slot].map(|cur| cur.hero_id) != Some(p.hero_id)
            {
                return Err(DomainError::DuplicateHero(p.hero_id));
            }
        }
        self.allies[slot] = pick;
        Ok(())
    }

    pub fn set_enemy(&mut self, slot: usize, pick: Option<DraftPick>) -> Result<(), DomainError> {
        if slot >= TEAM_SIZE {
            return Err(DomainError::InvalidSlot(slot));
        }
        if let Some(p) = pick {
            if self.is_unavailable(p.hero_id)
                && self.enemies[slot].map(|cur| cur.hero_id) != Some(p.hero_id)
            {
                return Err(DomainError::DuplicateHero(p.hero_id));
            }
        }
        self.enemies[slot] = pick;
        Ok(())
    }

    pub fn toggle_ban(&mut self, hero_id: HeroId) -> Result<bool, DomainError> {
        if let Some(slot) = self.bans.iter_mut().find(|b| **b == Some(hero_id)) {
            *slot = None;
            return Ok(false);
        }
        if self.is_unavailable(hero_id) {
            return Err(DomainError::DuplicateHero(hero_id));
        }
        if let Some(empty) = self.bans.iter_mut().find(|b| b.is_none()) {
            *empty = Some(hero_id);
            Ok(true)
        } else {
            Err(DomainError::BanListFull)
        }
    }

    pub fn clear(&mut self) {
        self.allies = [None; TEAM_SIZE];
        self.enemies = [None; TEAM_SIZE];
        self.bans = [None; 16];
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeroProfile {
    pub id: HeroId,
    pub slug: String,
    pub display_name: String,
    pub primary_roles: Vec<Role>,
    /// Base meta winrate in `[0.0, 1.0]` per role index (`Role::index()`).
    pub role_winrates: [f32; 5],
    /// Personal matches and wins for player comfort weighting.
    pub personal_matches: u32,
    pub personal_wins: u32,
    pub attack_type: AttackType,
    pub has_disable: bool,
    pub has_initiation: bool,
    /// 7-day patch winrate momentum delta (`recent_wr - week_wr`), clamped to `[-0.03, 0.03]`.
    pub trend_delta: f32,
    /// Recency decay factor `[0.25, 1.0]` computed from `last_played` Unix timestamp.
    pub comfort_decay: f32,
}

impl HeroProfile {
    #[inline]
    pub fn meta_winrate(&self, role: Role) -> f32 {
        (self.role_winrates[role.index()] + 0.45 * self.trend_delta).clamp(0.05, 0.95)
    }

    /// Personal Comfort Engine v3: combines empirical-Bayes performance on time-decayed
    /// effective matches (`g_eff = personal_matches * comfort_decay`, prior `m = 16`)
    /// with an active practice volume mastery bonus `0.0065 * ln(1 + g_eff)`.
    /// Heroes with `g_eff < 0.15` (unplayed for many months) decay cleanly to `0.0`.
    #[inline]
    pub fn comfort_delta(&self) -> f32 {
        if self.personal_matches == 0 {
            return 0.0;
        }
        let d = self.comfort_decay.clamp(0.0, 1.0);
        let g_eff = (self.personal_matches as f32) * d;
        if g_eff < 0.15 {
            return 0.0;
        }
        let w_eff = (self.personal_wins as f32) * d;
        let perf_wr = (w_eff + 16.0 * 0.50) / (g_eff + 16.0);
        let perf_edge = perf_wr - 0.50;
        let vol_bonus = if perf_wr >= 0.50 && g_eff >= 0.50 {
            0.0065 * (1.0 + g_eff).ln()
        } else {
            0.0
        };
        (perf_edge + vol_bonus).clamp(-0.05, 0.05)
    }
}

/// Pairwise empirical statistics between two heroes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PairStats {
    pub with_matches: u32,
    pub with_wins: u32,
    pub vs_matches: u32,
    pub vs_wins: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DomainError {
    InvalidSlot(usize),
    DuplicateHero(HeroId),
    HeroOutOfBounds(HeroId),
    BanListFull,
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlot(s) => write!(f, "draft slot {s} is out of range (expected 0..5)"),
            Self::DuplicateHero(id) => write!(f, "hero id {} is already picked or banned", id.0),
            Self::HeroOutOfBounds(id) => {
                write!(f, "hero id {} exceeds MAX_HEROES ({MAX_HEROES})", id.0)
            }
            Self::BanListFull => write!(f, "all 16 ban slots are already occupied"),
        }
    }
}

impl core::error::Error for DomainError {}

/// Compact zero-cost identifier for a Dota 2 item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u16);

/// Game phase bucket for a recommended item build step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemPhase {
    Starting,
    Early,
    Core,
    Late,
    Situational,
}

/// Tactical enemy mechanic or threat countered by specific item choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CounterTrait {
    Evasion,
    HeavyMagicDisable,
    HighHealingRegen,
    EtherealGhostSave,
    CriticalPassives,
    IllusionsSummons,
    Invisibility,
    HighPhysicalBurst,
}

impl CounterTrait {
    /// All counter-trait variants in canonical index order.
    pub const ALL: [Self; 8] = [
        Self::Evasion,
        Self::HeavyMagicDisable,
        Self::HighHealingRegen,
        Self::EtherealGhostSave,
        Self::CriticalPassives,
        Self::IllusionsSummons,
        Self::Invisibility,
        Self::HighPhysicalBurst,
    ];

    /// Human-readable short label for UI and CLI display.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Evasion => "Evasion",
            Self::HeavyMagicDisable => "Heavy Magic / Disable",
            Self::HighHealingRegen => "High Healing / Regen",
            Self::EtherealGhostSave => "Ethereal / Ghost Save",
            Self::CriticalPassives => "Critical Passives",
            Self::IllusionsSummons => "Illusions / Summons",
            Self::Invisibility => "Invisibility",
            Self::HighPhysicalBurst => "High Physical Burst",
        }
    }

    #[inline]
    const fn index(self) -> usize {
        match self {
            Self::Evasion => 0,
            Self::HeavyMagicDisable => 1,
            Self::HighHealingRegen => 2,
            Self::EtherealGhostSave => 3,
            Self::CriticalPassives => 4,
            Self::IllusionsSummons => 5,
            Self::Invisibility => 6,
            Self::HighPhysicalBurst => 7,
        }
    }

    /// Parses a snake_case slug from `data/counter_traits.toml` into a `CounterTrait`.
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "evasion" => Some(Self::Evasion),
            "heavy_magic_disable" => Some(Self::HeavyMagicDisable),
            "high_healing_regen" => Some(Self::HighHealingRegen),
            "ethereal_ghost_save" => Some(Self::EtherealGhostSave),
            "critical_passives" => Some(Self::CriticalPassives),
            "illusions_summons" => Some(Self::IllusionsSummons),
            "invisibility" => Some(Self::Invisibility),
            "high_physical_burst" => Some(Self::HighPhysicalBurst),
            _ => None,
        }
    }
}

/// Static catalog metadata for a Dota 2 item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemCatalogEntry {
    pub id: ItemId,
    pub slug: String,
    pub display_name: String,
    pub cost: u32,
    pub components: Vec<ItemId>,
    pub is_recipe: bool,
    pub is_consumable_drop: bool,
}

/// Empirical purchase statistics for a hero item in a specific role and bracket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeroItemStat {
    pub item_id: ItemId,
    pub phase: ItemPhase,
    pub avg_time_mins: f32,
    pub match_count: u32,
    pub win_count: u32,
}

/// Alternative item option within the same phase and score tolerance of a primary step.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildVariant {
    pub item_id: ItemId,
    pub score: f32,
    pub avg_time_mins: f32,
    pub counter_trait: Option<CounterTrait>,
}

/// Single primary step in a sequential item build recommendation.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildStep {
    pub item_id: ItemId,
    pub phase: ItemPhase,
    pub avg_time_mins: f32,
    pub score: f32,
    pub owned: bool,
    pub owned_components: usize,
    pub total_components: usize,
    pub counter_trait: Option<CounterTrait>,
    pub variants: Vec<BuildVariant>,
}

/// Situational or enemy-counter item presented separately from the timed core progression.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionalBuildItem {
    pub item_id: ItemId,
    pub avg_time_mins: f32,
    pub score: f32,
    pub owned: bool,
    pub owned_components: usize,
    pub total_components: usize,
    pub counter_trait: Option<CounterTrait>,
}

/// Complete recommended item progression for a hero, role, bracket, and enemy matchup.
#[derive(Debug, Clone, PartialEq)]
pub struct RecommendedBuild {
    pub target_patch: &'static str,
    pub hero_id: HeroId,
    pub role: Role,
    pub bracket: Bracket,
    pub starting_items: Vec<ItemId>,
    pub steps: Vec<BuildStep>,
    pub optional_items: Vec<OptionalBuildItem>,
    pub next_unowned_index: Option<usize>,
}

/// Returns whether a counter-trait item makes sense for `role` so carries are not shown
/// team-aura utility (e.g. Pipe) and supports are not shown carry DPS items (e.g. Battle Fury / MKB).
#[must_use]
pub const fn is_counter_item_eligible_for_role(item_id: ItemId, role: Role) -> bool {
    match item_id.0 {
        // Carry / semi-carry DPS & anti-evasion/passive damage items
        145 => matches!(role, Role::Carry),
        135 | 158 | 160 | 225 | 249 | 250 => matches!(role, Role::Carry | Role::Mid),
        // Core teamfight BKB & armor/anti-heal scaling
        112 => matches!(role, Role::Carry | Role::Offlane),
        116 => matches!(role, Role::Carry | Role::Mid | Role::Offlane),
        119 => matches!(role, Role::Mid | Role::Offlane),
        // Offlane team mitigation & initiation/utility
        90 | 242 => matches!(role, Role::Offlane),
        210 => matches!(role, Role::Offlane | Role::SoftSupport),
        // Support & utility aura/dispel/heal-reduction items
        37 => matches!(role, Role::SoftSupport | Role::HardSupport),
        226 => matches!(role, Role::Offlane | Role::SoftSupport | Role::HardSupport),
        267 => matches!(
            role,
            Role::Mid | Role::Offlane | Role::SoftSupport | Role::HardSupport
        ),
        // Detection items: Dust for active gankers/cores, Gem for frontline/supports, Sentry for supports
        40 => true,
        30 => matches!(role, Role::Offlane | Role::SoftSupport | Role::HardSupport),
        43 => matches!(role, Role::SoftSupport | Role::HardSupport),
        _ => true,
    }
}

/// Compile-time embedded contents of the patch-versioned counter-trait data file.
pub const COUNTER_TRAIT_DATA_TOML: &str = include_str!("../../../data/counter_traits.toml");

/// Dota 2 patch version against which the curated counter-trait tables are maintained.
pub const COUNTER_TRAIT_TABLE_PATCH: &str = "7.40";

/// ISO-8601 review date of the curated counter-trait tables in `data/counter_traits.toml`.
pub const COUNTER_TRAIT_TABLE_REVIEWED_AT: &str = "2026-11-21";

#[derive(Debug, Clone, PartialEq, Eq)]
struct CounterTraitTable {
    patch: String,
    reviewed_at: String,
    traits: [Vec<ItemId>; 8],
    heroes: Vec<(u16, Vec<CounterTrait>)>,
    all_referenced_items: Vec<ItemId>,
}

impl CounterTraitTable {
    fn empty() -> Self {
        Self {
            patch: String::new(),
            reviewed_at: String::new(),
            traits: Default::default(),
            heroes: Vec::new(),
            all_referenced_items: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TomlSection {
    Root,
    Traits,
    Heroes,
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_quotes = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && in_quotes {
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == '#' && !in_quotes {
            return &line[..idx];
        }
    }
    line
}

fn parse_quoted_str(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or("expected double-quoted TOML string")?;
    Ok(inner.to_owned())
}

fn parse_array_elements(raw: &str) -> Result<Vec<&str>, &'static str> {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or("expected TOML array `[...]`")?;
    let mut items = Vec::new();
    for part in inner.split(',') {
        let token = part.trim();
        if !token.is_empty() {
            items.push(token);
        }
    }
    Ok(items)
}

fn parse_counter_trait_toml(raw: &str) -> Result<CounterTraitTable, &'static str> {
    let mut table = CounterTraitTable::empty();
    let mut section = TomlSection::Root;

    for raw_line in raw.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let header = line[1..line.len() - 1].trim();
            section = match header {
                "traits" => TomlSection::Traits,
                "heroes" => TomlSection::Heroes,
                _ => return Err("unknown TOML section in counter_traits.toml"),
            };
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or("expected `key = value` in counter_traits.toml")?;
        let key = key.trim();
        let value = value.trim();

        match section {
            TomlSection::Root => match key {
                "patch" => table.patch = parse_quoted_str(value)?,
                "reviewed_at" => table.reviewed_at = parse_quoted_str(value)?,
                _ => return Err("unknown root key in counter_traits.toml"),
            },
            TomlSection::Traits => {
                let trait_kind =
                    CounterTrait::from_slug(key).ok_or("unknown counter trait slug in [traits]")?;
                let mut items = Vec::new();
                for token in parse_array_elements(value)? {
                    let id: u16 = token
                        .parse()
                        .map_err(|_| "invalid item ID integer in [traits]")?;
                    items.push(ItemId(id));
                }
                table.traits[trait_kind.index()] = items;
            }
            TomlSection::Heroes => {
                let hero_id: u16 = key
                    .parse()
                    .map_err(|_| "invalid hero ID integer in [heroes]")?;
                let mut traits = Vec::new();
                for token in parse_array_elements(value)? {
                    let slug = parse_quoted_str(token)?;
                    let trait_kind = CounterTrait::from_slug(&slug)
                        .ok_or("unknown counter trait slug in [heroes]")?;
                    traits.push(trait_kind);
                }
                table.heroes.push((hero_id, traits));
            }
        }
    }

    if table.patch.is_empty() || table.reviewed_at.is_empty() {
        return Err("counter_traits.toml must define non-empty `patch` and `reviewed_at`");
    }

    table.heroes.sort_unstable_by_key(|(id, _)| *id);

    let mut all_items: Vec<ItemId> = table
        .traits
        .iter()
        .flat_map(|items| items.iter().copied())
        .collect();
    all_items.sort_unstable();
    all_items.dedup();
    table.all_referenced_items = all_items;

    Ok(table)
}

fn counter_trait_table() -> &'static CounterTraitTable {
    static TABLE: OnceLock<CounterTraitTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        parse_counter_trait_toml(COUNTER_TRAIT_DATA_TOML)
            .unwrap_or_else(|_| CounterTraitTable::empty())
    })
}

/// Returns the patch string parsed from `data/counter_traits.toml`.
pub fn counter_trait_table_patch() -> &'static str {
    counter_trait_table().patch.as_str()
}

/// Returns the ISO-8601 review date parsed from `data/counter_traits.toml`.
pub fn counter_trait_table_reviewed_at() -> &'static str {
    counter_trait_table().reviewed_at.as_str()
}

/// Maps a Dota 2 hero ID to the tactical counter traits they present to opponents.
pub fn hero_counter_traits(hero_id: HeroId) -> &'static [CounterTrait] {
    let heroes = &counter_trait_table().heroes;
    match heroes.binary_search_by_key(&hero_id.0, |(id, _)| *id) {
        Ok(idx) => heroes[idx].1.as_slice(),
        Err(_) => &[],
    }
}

/// Maps a counter trait to canonical OpenDota/STRATZ item IDs that answer it.
pub fn counter_items_for_trait(trait_kind: CounterTrait) -> &'static [ItemId] {
    counter_trait_table().traits[trait_kind.index()].as_slice()
}

/// Deduplicated slice of all item IDs referenced by `counter_items_for_trait` for CI catalog validation.
pub fn all_referenced_counter_item_ids() -> &'static [ItemId] {
    counter_trait_table().all_referenced_items.as_slice()
}
