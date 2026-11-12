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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Allies,
    Enemies,
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
}

impl Default for DraftState {
    fn default() -> Self {
        Self {
            allies: [None; TEAM_SIZE],
            enemies: [None; TEAM_SIZE],
            bans: [None; 16],
            bracket: Bracket::Legend,
            target_role: Role::Carry,
        }
    }
}

impl DraftState {
    pub fn is_unavailable(&self, hero_id: HeroId) -> bool {
        self.allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == hero_id)
            || self
                .enemies
                .iter()
                .flatten()
                .any(|p| p.hero_id == hero_id)
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
}

impl HeroProfile {
    #[inline]
    pub fn meta_winrate(&self, role: Role) -> f32 {
        self.role_winrates[role.index()].clamp(0.05, 0.95)
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
