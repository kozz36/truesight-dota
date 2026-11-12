use crate::domain::{Bracket, HeroId, Role, Side};
use crate::math::MatchupMatrix;
use core::fmt;

/// Driven port for loading and refreshing hero meta winrates and pairwise matchup matrices.
pub trait StatsRepository {
    type Error: core::error::Error;

    fn load_matrix(&self, bracket: Bracket) -> Result<MatchupMatrix, Self::Error>;
}

/// Driving/Driven port for live draft events (GSI game state, manual quick-pick, or Phase 2 vision).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DraftEvent {
    GameRulesStateChanged {
        state_name: String,
        game_time_secs: i32,
    },
    LocalPlayerHeroDetected {
        hero_slug: String,
    },
    PickDetected {
        side: Side,
        slot: usize,
        hero_id: HeroId,
        role: Option<Role>,
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
