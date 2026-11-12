use crate::snapshot_data::OPENDOTA_HERO_SNAPSHOT;
use crate::snapshot_pairs::OPENDOTA_CALIBRATED_PAIRS;
use truesight_core::domain::{Bracket, HeroId, HeroProfile};
use truesight_core::math::{MatchupCell, MatchupMatrix};
use truesight_core::ports::{PortError, StatsRepository};

/// Full 127-hero repository backed by the OpenDota / STRATZ meta snapshot and
/// player `127255741` personal comfort history.
#[derive(Debug, Clone, Default)]
pub struct SeededMetaRepository {
    pub custom_personal_pool: Vec<(HeroId, u32, u32)>,
    pub use_demo_comfort_defaults: bool,
}

impl SeededMetaRepository {
    pub fn new() -> Self {
        Self {
            custom_personal_pool: Vec::new(),
            use_demo_comfort_defaults: false,
        }
    }

    pub fn with_demo_comfort() -> Self {
        Self {
            custom_personal_pool: Vec::new(),
            use_demo_comfort_defaults: true,
        }
    }
}

fn bracket_index(bracket: Bracket) -> usize {
    match bracket {
        Bracket::HeraldGuardian => 0,
        Bracket::CrusaderArchon => 1,
        Bracket::LegendAncient => 2,
        Bracket::DivineImmortal => 3,
        _ => 3,
    }
}

impl StatsRepository for SeededMetaRepository {
    type Error = PortError;

    fn load_matrix(&self, bracket: Bracket) -> Result<MatchupMatrix, Self::Error> {
        let mut m = MatchupMatrix::new();
        let b_idx = bracket_index(bracket);

        // 1. Load all 127 official Dota 2 heroes from the OpenDota snapshot
        for row in OPENDOTA_HERO_SNAPSHOT {
            let base_wr = row.bracket_winrates[b_idx];
            let mut role_winrates = [0.50_f32; 5];
            for (r_idx, offset) in row.role_offsets.iter().copied().enumerate() {
                role_winrates[r_idx] = (base_wr + offset).clamp(0.35, 0.65);
            }

            m.upsert_hero(HeroProfile {
                id: HeroId(row.id),
                slug: row.slug.to_string(),
                display_name: row.name.to_string(),
                primary_roles: row.primary_roles.to_vec(),
                role_winrates,
                personal_matches: row.personal_matches,
                personal_wins: row.personal_wins,
            })
            .map_err(|e| PortError(e.to_string()))?;
        }

        // 2. Apply custom personal pool overrides when provided
        for &(hero_id, matches, wins) in &self.custom_personal_pool {
            if let Some(existing) = m.hero(hero_id).cloned() {
                m.upsert_hero(HeroProfile {
                    personal_matches: matches,
                    personal_wins: wins,
                    ..existing
                })
                .map_err(|e| PortError(e.to_string()))?;
            }
        }

        // 3. Load all 16,002 strictly antisymmetric counter & zero-mean symmetric synergy pairs
        for &(a, b, c_bp, s_bp, vs_m) in OPENDOTA_CALIBRATED_PAIRS {
            m.set_calibrated_cell(
                HeroId(a),
                HeroId(b),
                MatchupCell {
                    counter_delta: (c_bp as f32) / 10000.0,
                    synergy_delta: (s_bp as f32) / 10000.0,
                    vs_matches: vs_m,
                    with_matches: 400,
                },
            )
            .map_err(|e| PortError(e.to_string()))?;
        }

        Ok(m)
    }
}
