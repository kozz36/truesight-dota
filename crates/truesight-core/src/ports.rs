use crate::domain::{Bracket, Faction, HeroId, HeroItemStat, ItemCatalogEntry, ItemId, Role, Side};
use crate::math::MatchupMatrix;
use core::fmt;

/// Driven port for loading and refreshing hero meta winrates and pairwise matchup matrices.
pub trait StatsRepository {
    type Error: core::error::Error;

    fn load_matrix(&self, bracket: Bracket) -> Result<MatchupMatrix, Self::Error>;
}

/// Driven port for item catalog metadata and role/bracket hero item build statistics.
pub trait ItemMetaRepository {
    /// Looks up static catalog metadata (cost, components, recipe/drop flags) for an item.
    fn item_catalog(&self, item_id: ItemId) -> Option<ItemCatalogEntry>;

    /// Loads empirical item purchase statistics for a hero in a given role and medal bracket.
    fn hero_item_stats(
        &self,
        hero_id: HeroId,
        role: Role,
        bracket: Bracket,
    ) -> Result<Vec<HeroItemStat>, PortError>;
}

/// Driving/Driven port for live draft events (GSI game state, manual quick-pick, or Phase 2 vision).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DraftEvent {
    GameRulesStateChanged {
        state_name: String,
        game_time_secs: i32,
    },
    FactionDetected {
        faction: Faction,
    },
    PlayerSteamIdDetected {
        steam32: u64,
    },
    LocalPlayerHeroDetected {
        hero_slug: String,
    },
    PlayerHeroMapped {
        steam32: u64,
        faction: Faction,
        hero_slug: String,
    },
    PickDetected {
        side: Side,
        slot: usize,
        hero_id: HeroId,
        role: Option<Role>,
    },
    BanDetected {
        hero_id: HeroId,
    },
    /// Local player's active inventory, backpack, stash, TP, and neutral item slugs parsed from GSI.
    InventoryUpdated {
        item_slugs: Vec<String>,
    },
    DraftReset,
}

pub trait DraftEventSource {
    type Error: core::error::Error;

    /// Non-blocking poll returning any queued draft/game-state events.
    fn poll_events(&mut self) -> Result<Vec<DraftEvent>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortError(pub String);

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for PortError {}
