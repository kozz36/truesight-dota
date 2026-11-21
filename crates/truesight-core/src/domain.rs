use core::fmt;

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

/// Complete recommended item progression for a hero, role, bracket, and enemy matchup.
#[derive(Debug, Clone, PartialEq)]
pub struct RecommendedBuild {
    pub target_patch: &'static str,
    pub hero_id: HeroId,
    pub role: Role,
    pub bracket: Bracket,
    pub starting_items: Vec<ItemId>,
    pub steps: Vec<BuildStep>,
    pub next_unowned_index: Option<usize>,
}

/// Dota 2 patch version against which the curated counter-trait tables are maintained.
pub const COUNTER_TRAIT_TABLE_PATCH: &str = "7.40";

/// Maps a Dota 2 hero ID to the tactical counter traits they present to opponents.
pub fn hero_counter_traits(hero_id: HeroId) -> &'static [CounterTrait] {
    match hero_id.0 {
        // Phantom Assassin: Blur evasion, Coup de Grace/Blur passives, high physical crit burst.
        44 => &[
            CounterTrait::Evasion,
            CounterTrait::CriticalPassives,
            CounterTrait::HighPhysicalBurst,
        ],
        // Windranger: Windrun evasion.
        21 => &[CounterTrait::Evasion],
        // Huskar: Berserker's Blood / regeneration.
        59 => &[CounterTrait::HighHealingRegen],
        // Necrophos: Ghost Shroud ethereal save + Death Pulse / Sadist healing.
        36 => &[
            CounterTrait::HighHealingRegen,
            CounterTrait::EtherealGhostSave,
        ],
        // Morphling, Alchemist, Io: high healing/regen sustain.
        10 | 73 | 91 => &[CounterTrait::HighHealingRegen],
        // Bristleback: Bristleback passive + spell lifesteal sustain.
        99 => &[
            CounterTrait::CriticalPassives,
            CounterTrait::HighHealingRegen,
        ],
        // Spectre, Tidehunter: Dispersion / Kraken Shell critical passives.
        67 | 29 => &[CounterTrait::CriticalPassives],
        // Phantom Lancer, Naga Siren, Meepo: illusion/clone armies.
        12 | 89 | 82 => &[CounterTrait::IllusionsSummons],
        // Chaos Knight: Phantasm illusions + physical crit burst.
        81 => &[
            CounterTrait::IllusionsSummons,
            CounterTrait::HighPhysicalBurst,
        ],
        // Riki, Bounty Hunter: permanent / frequent invisibility.
        32 | 62 => &[CounterTrait::Invisibility],
        // Clinkz: Skeleton Walk invisibility + high physical burst.
        56 => &[CounterTrait::Invisibility, CounterTrait::HighPhysicalBurst],
        // Nyx Assassin: Vendetta invisibility + heavy magic burst/stun.
        88 => &[CounterTrait::Invisibility, CounterTrait::HeavyMagicDisable],
        // Lion, Shadow Shaman, Invoker, Earthshaker, Zeus: heavy magic damage / disables.
        26 | 27 | 74 | 7 | 22 => &[CounterTrait::HeavyMagicDisable],
        // Pugna: Decrepify ethereal save + heavy magic burst.
        45 => &[
            CounterTrait::EtherealGhostSave,
            CounterTrait::HeavyMagicDisable,
        ],
        // Sven, Templar Assassin, Ursa: high physical burst.
        18 | 46 | 70 => &[CounterTrait::HighPhysicalBurst],
        _ => &[],
    }
}

/// Maps a counter trait to canonical OpenDota/STRATZ item IDs that answer it.
pub fn counter_items_for_trait(trait_kind: CounterTrait) -> &'static [ItemId] {
    match trait_kind {
        // Monkey King Bar (135)
        CounterTrait::Evasion => &[ItemId(135)],
        // Black King Bar (116), Pipe of Insight (90), Lotus Orb (226)
        CounterTrait::HeavyMagicDisable => &[ItemId(116), ItemId(90), ItemId(226)],
        // Spirit Vessel (267), Shiva's Guard (119), Eye of Skadi (160)
        CounterTrait::HighHealingRegen => &[ItemId(267), ItemId(119), ItemId(160)],
        // Bloodthorn (250), Nullifier (225)
        CounterTrait::EtherealGhostSave => &[ItemId(250), ItemId(225)],
        // Silver Edge (249)
        CounterTrait::CriticalPassives => &[ItemId(249)],
        // Mjollnir (158), Battle Fury (145), Shiva's Guard (119)
        CounterTrait::IllusionsSummons => &[ItemId(158), ItemId(145), ItemId(119)],
        // Dust of Appearance (40), Gem of True Sight (30), Sentry Ward (43)
        CounterTrait::Invisibility => &[ItemId(40), ItemId(30), ItemId(43)],
        // Assault Cuirass (112), Shiva's Guard (119), Crimson Guard (242), Heaven's Halberd (210)
        CounterTrait::HighPhysicalBurst => &[ItemId(112), ItemId(119), ItemId(242), ItemId(210)],
    }
}

/// Deduplicated slice of all item IDs referenced by `counter_items_for_trait` for CI catalog validation.
pub fn all_referenced_counter_item_ids() -> &'static [ItemId] {
    &[
        ItemId(30),
        ItemId(40),
        ItemId(43),
        ItemId(90),
        ItemId(112),
        ItemId(116),
        ItemId(119),
        ItemId(135),
        ItemId(145),
        ItemId(158),
        ItemId(160),
        ItemId(210),
        ItemId(225),
        ItemId(226),
        ItemId(242),
        ItemId(249),
        ItemId(250),
        ItemId(267),
    ]
}
