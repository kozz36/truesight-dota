use crate::snapshot_data::OPENDOTA_HERO_SNAPSHOT;
use crate::snapshot_pairs::OPENDOTA_CALIBRATED_PAIRS;
use truesight_core::domain::{Bracket, HeroId, HeroProfile};
use truesight_core::math::{MatchupCell, MatchupMatrix};
use truesight_core::ports::{PortError, StatsRepository};

/// Full 127-hero repository backed by the OpenDota / STRATZ meta snapshot and
/// neutral default personal comfort history (populated dynamically per linked account).
#[derive(Debug, Clone, Default)]
pub struct SeededMetaRepository {
    pub custom_personal_pool: Vec<(HeroId, u32, u32, f32)>,
    pub custom_hero_meta: Vec<(HeroId, [f32; 8], f32)>,
    pub custom_pair_synergy: Vec<(HeroId, HeroId, f32, u32)>,
    pub use_demo_comfort_defaults: bool,
}

impl SeededMetaRepository {
    pub fn new() -> Self {
        Self {
            custom_personal_pool: Vec::new(),
            custom_hero_meta: Vec::new(),
            custom_pair_synergy: Vec::new(),
            use_demo_comfort_defaults: false,
        }
    }

    pub fn with_demo_comfort() -> Self {
        Self {
            custom_personal_pool: Vec::new(),
            custom_hero_meta: Vec::new(),
            custom_pair_synergy: Vec::new(),
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

        // 1. Load all 127 official Dota 2 heroes from the OpenDota snapshot (with optional live cache overrides)
        for row in OPENDOTA_HERO_SNAPSHOT {
            let override_meta = self
                .custom_hero_meta
                .iter()
                .find(|(hid, _, _)| hid.0 == row.id);
            let bracket_wrs = override_meta
                .map(|(_, bw, _)| *bw)
                .unwrap_or(row.bracket_winrates);
            let trend_delta = override_meta
                .map(|(_, _, td)| *td)
                .unwrap_or(row.trend_delta);

            let base_wr = bracket_wrs[b_idx];
            let mean_wr =
                bracket_wrs.iter().copied().sum::<f32>() / (bracket_wrs.len() as f32);
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
                trend_delta,
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
        for &(hero_id, matches, wins, decay) in &self.custom_personal_pool {
            if let Some(existing) = m.hero(hero_id).cloned() {
                m.upsert_hero(HeroProfile {
                    personal_matches: matches,
                    personal_wins: wins,
                    comfort_decay: decay,
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

        // 4. Apply optional STRATZ empirical `with` synergy overrides symmetrically
        for &(a, b, syn_raw, with_matches) in &self.custom_pair_synergy {
            if a == b {
                continue;
            }
            let a_idx = a.as_index();
            let b_idx = b.as_index();
            if a_idx >= truesight_core::domain::MAX_HEROES
                || b_idx >= truesight_core::domain::MAX_HEROES
            {
                continue;
            }
            let da = hero_bracket_shift[a_idx] - mean_bracket_shift;
            let db = hero_bracket_shift[b_idx] - mean_bracket_shift;
            let s_adj = tau * syn_raw + 0.15 * 0.5 * (da + db);

            let ab_existing = m.cell(a, b);
            let ba_existing = m.cell(b, a);
            m.set_calibrated_cell(
                a,
                b,
                MatchupCell {
                    counter_delta: ab_existing.counter_delta,
                    synergy_delta: s_adj,
                    vs_matches: ab_existing.vs_matches,
                    with_matches,
                },
            )
            .map_err(|e| PortError(e.to_string()))?;
            m.set_calibrated_cell(
                b,
                a,
                MatchupCell {
                    counter_delta: ba_existing.counter_delta,
                    synergy_delta: s_adj,
                    vs_matches: ba_existing.vs_matches,
                    with_matches,
                },
            )
            .map_err(|e| PortError(e.to_string()))?;
        }

        Ok(m)
    }
}
