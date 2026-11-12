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
    bracket.index()
}

impl StatsRepository for SeededMetaRepository {
    type Error = PortError;

    fn load_matrix(&self, bracket: Bracket) -> Result<MatchupMatrix, Self::Error> {
        let mut m = MatchupMatrix::new();
        let b_idx = bracket_index(bracket);

        // Medal execution sharpness factor (Legend = 1.00 reference; Herald..Immortal = 0.86..1.12)
        const BRACKET_EXECUTION_SCALE: [f32; 8] =
            [0.86, 0.90, 0.94, 0.97, 1.00, 1.03, 1.07, 1.12];
        let tau = BRACKET_EXECUTION_SCALE[b_idx];

        let mut hero_bracket_shift = [0.0_f32; truesight_core::domain::MAX_HEROES];
        let mut shift_sum = 0.0_f32;
        let mut shift_cnt = 0_u32;

        // 1. Load all 127 official Dota 2 heroes from the OpenDota snapshot
        for row in OPENDOTA_HERO_SNAPSHOT {
            let base_wr = row.bracket_winrates[b_idx];
            let mean_wr = row.bracket_winrates.iter().copied().sum::<f32>()
                / (row.bracket_winrates.len() as f32);
            let shift = base_wr - mean_wr;
            if (row.id as usize) < truesight_core::domain::MAX_HEROES {
                hero_bracket_shift[row.id as usize] = shift;
                shift_sum += shift;
                shift_cnt += 1;
            }

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
                attack_type: row.attack_type,
                has_disable: row.has_disable,
                has_initiation: row.has_initiation,
                trend_delta: row.trend_delta,
                comfort_decay: row.comfort_decay,
            })
            .map_err(|e| PortError(e.to_string()))?;
        }

        let mean_bracket_shift = if shift_cnt > 0 {
            shift_sum / (shift_cnt as f32)
        } else {
            0.0
        };

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

        // 3. Load all 16,002 strictly antisymmetric counter & zero-mean symmetric synergy pairs,
        // modulated by medal execution sharpness (`tau`) and per-hero medal scaling (`d_a`, `d_b`).
        for &(a, b, c_bp, s_bp, vs_m) in OPENDOTA_CALIBRATED_PAIRS {
            let (counter_delta, synergy_delta) = if a == b {
                (0.0, 0.0)
            } else {
                let c0 = (c_bp as f32) / 10000.0;
                let s0 = (s_bp as f32) / 10000.0;
                let da = hero_bracket_shift[a as usize] - mean_bracket_shift;
                let db = hero_bracket_shift[b as usize] - mean_bracket_shift;
                // Strictly antisymmetric: c(a,b) = -c(b,a)
                let c_adj = tau * c0 + 0.22 * (da - db);
                // Strictly symmetric & zero-mean: s(a,b) = s(b,a)
                let s_adj = tau * s0 + 0.15 * 0.5 * (da + db);
                (c_adj, s_adj)
            };

            m.set_calibrated_cell(
                HeroId(a),
                HeroId(b),
                MatchupCell {
                    counter_delta,
                    synergy_delta,
                    vs_matches: vs_m,
                    with_matches: 400,
                },
            )
            .map_err(|e| PortError(e.to_string()))?;
        }

        Ok(m)
    }
}
