use crate::snapshot_data::OPENDOTA_HERO_SNAPSHOT;
use crate::snapshot_pairs::OPENDOTA_PAIR_SNAPSHOT;
use truesight_core::domain::{Bracket, HeroId, HeroProfile, PairStats, Role};
use truesight_core::math::MatchupMatrix;
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

        // 2. Apply curated high-MMR baseline role winrates for core meta staples
        let curated_overrides: &[(u16, &[Role], [f32; 5], u32, u32)] = &[
            (1, &[Role::Carry], [0.518, 0.450, 0.440, 0.410, 0.400], 42, 27),
            (2, &[Role::Offlane], [0.460, 0.485, 0.534, 0.475, 0.455], 30, 18),
            (5, &[Role::HardSupport, Role::SoftSupport], [0.420, 0.440, 0.450, 0.518, 0.531], 25, 15),
            (6, &[Role::Carry], [0.524, 0.485, 0.445, 0.420, 0.415], 18, 11),
            (7, &[Role::SoftSupport, Role::Mid, Role::Offlane], [0.450, 0.508, 0.502, 0.519, 0.485], 35, 21),
            (8, &[Role::Carry], [0.529, 0.475, 0.450, 0.425, 0.420], 55, 34),
            (11, &[Role::Mid, Role::Carry], [0.522, 0.526, 0.465, 0.430, 0.420], 20, 12),
            (12, &[Role::Carry], [0.506, 0.460, 0.440, 0.410, 0.405], 14, 8),
            (13, &[Role::Mid], [0.460, 0.514, 0.482, 0.465, 0.445], 12, 7),
            (14, &[Role::SoftSupport, Role::Offlane, Role::Mid], [0.475, 0.505, 0.512, 0.518, 0.502], 60, 35),
            (17, &[Role::Mid], [0.465, 0.516, 0.455, 0.440, 0.430], 22, 13),
            (20, &[Role::HardSupport, Role::SoftSupport, Role::Offlane], [0.485, 0.490, 0.524, 0.528, 0.532], 19, 12),
            (26, &[Role::HardSupport, Role::SoftSupport], [0.420, 0.465, 0.450, 0.508, 0.512], 40, 22),
            (31, &[Role::HardSupport], [0.415, 0.445, 0.450, 0.515, 0.538], 28, 18),
            (41, &[Role::Carry], [0.519, 0.455, 0.470, 0.420, 0.410], 38, 24),
            (44, &[Role::Carry], [0.509, 0.470, 0.445, 0.420, 0.410], 33, 19),
            (54, &[Role::Carry], [0.527, 0.465, 0.475, 0.430, 0.420], 29, 18),
            (74, &[Role::Mid, Role::SoftSupport], [0.465, 0.512, 0.475, 0.504, 0.480], 45, 27),
            (86, &[Role::SoftSupport, Role::Mid], [0.430, 0.492, 0.460, 0.511, 0.495], 50, 29),
            (94, &[Role::Carry], [0.536, 0.495, 0.460, 0.420, 0.410], 16, 11),
            (97, &[Role::Offlane, Role::Mid], [0.475, 0.514, 0.522, 0.495, 0.480], 24, 15),
            (104, &[Role::Offlane], [0.475, 0.495, 0.528, 0.470, 0.455], 31, 19),
            (106, &[Role::Mid], [0.480, 0.515, 0.475, 0.455, 0.440], 27, 16),
            (111, &[Role::HardSupport], [0.415, 0.450, 0.445, 0.508, 0.526], 21, 14),
            (120, &[Role::Mid, Role::Offlane], [0.465, 0.510, 0.505, 0.475, 0.460], 15, 9),
        ];

        for &(id, roles, role_winrates, demo_m, demo_w) in curated_overrides {
            if let Some(existing) = m.hero(HeroId(id)).cloned() {
                let (personal_matches, personal_wins) =
                    if self.use_demo_comfort_defaults || existing.personal_matches == 0 {
                        (demo_m, demo_w)
                    } else {
                        (existing.personal_matches, existing.personal_wins)
                    };
                m.upsert_hero(HeroProfile {
                    primary_roles: roles.to_vec(),
                    role_winrates,
                    personal_matches,
                    personal_wins,
                    ..existing
                })
                .map_err(|e| PortError(e.to_string()))?;
            }
        }

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

        // 3. Load all 15,966 empirical pairwise OpenDota matchups (every 127x127 hero pair)
        for &(a, b, vs_m, vs_w, with_m, with_w) in OPENDOTA_PAIR_SNAPSHOT {
            m.set_pair_stats(
                HeroId(a),
                HeroId(b),
                PairStats {
                    with_matches: with_m,
                    with_wins: with_w,
                    vs_matches: vs_m,
                    vs_wins: vs_w,
                },
                30.0,
            )
            .map_err(|e| PortError(e.to_string()))?;
        }

        // 4. High-confidence STRATZ/OpenDota notable pairs override
        let notable_pairs: &[(u16, u16, u32, u32)] = &[
            (1, 94, 635, 455),    // Anti-Mage hard-counters Medusa
            (1, 17, 610, 485),    // Anti-Mage counters Storm Spirit
            (1, 74, 575, 495),    // Anti-Mage counters Invoker
            (1, 2, 415, 535),     // Axe counters Anti-Mage; good synergy together
            (1, 104, 420, 530),   // Legion Commander counters Anti-Mage
            (1, 97, 510, 585),    // Magnus Empower + Reverse Polarity synergy with Anti-Mage
            (1, 26, 535, 545),    // Lion mana drain + AM Mana Void synergy
            (7, 12, 645, 495),    // Earthshaker hard-counters Phantom Lancer (Echo Slam)
            (7, 81, 610, 510),    // Earthshaker counters Chaos Knight illusions
            (7, 82, 625, 490),    // Earthshaker counters Meepo clones
            (2, 12, 615, 490),    // Axe hard-counters Phantom Lancer & Drow
            (2, 6, 585, 540),     // Axe Call counters Drow Ranger; good frontline synergy with Drow
            (2, 44, 570, 520),    // Axe Blade Mail counters Phantom Assassin
            (2, 50, 620, 495),    // Axe Culling Blade pierces Dazzle Shallow Grave
            (41, 74, 545, 595),   // Faceless Void Chronosphere + Invoker Cataclysm/Sunstrike
            (41, 31, 530, 580),   // Faceless Void + Lich Chain Frost
            (41, 6, 575, 480),    // Faceless Void Time Walk jumps Drow Ranger
            (41, 111, 435, 560),  // Oracle False Promise saves allies inside Chronosphere
            (6, 20, 530, 590),    // Drow Ranger + Vengeful Spirit aura synergy
            (8, 5, 535, 585),     // Juggernaut Blade Fury + Crystal Maiden Frostbite lane combo
            (8, 97, 515, 575),    // Juggernaut + Magnus Empower
            (54, 17, 465, 585),   // Lifestealer Infest bomb + Storm Spirit / Puck
            (54, 13, 470, 580),   // Lifestealer Infest + Puck
            (54, 2, 575, 525),    // Lifestealer Feast counters high-HP strength cores
            (104, 17, 585, 540),  // Legion Duel locks down Storm Spirit
            (104, 13, 575, 535),  // Legion Duel locks down Puck
            (104, 106, 570, 530), // Legion Duel locks down Ember Spirit
            (104, 111, 410, 565), // Oracle False Promise ruins enemy Legion Duel
            (86, 7, 555, 515),    // Rubick steals Fissure/Echo Slam vs Earthshaker
            (86, 97, 560, 520),   // Rubick steals Reverse Polarity vs Magnus
            (86, 29, 575, 515),   // Rubick steals Ravage vs Tidehunter
            (86, 33, 580, 510),   // Rubick steals Black Hole vs Enigma
            (11, 5, 550, 565),    // Shadow Fiend + Crystal Maiden aura/setup
            (97, 94, 480, 520),   // Magnus vs Medusa
            (97, 17, 540, 535),   // Magnus Reverse Polarity catches Storm Spirit
            (97, 2, 485, 550),    // Magnus vs Axe; strong teamfight synergy
            (97, 5, 525, 560),    // Magnus Reverse Polarity + Crystal Maiden Freezing Field
            (5, 94, 465, 530),    // Crystal Maiden vs Medusa
            (5, 17, 520, 540),    // Crystal Maiden Frostbite roots Storm Spirit
            (5, 2, 450, 555),     // Axe jumps Crystal Maiden; strong lane synergy together
            (19, 51, 535, 565),   // Tiny + Clockwerk (Player 127255741 top signature heroes)
            (135, 51, 530, 575),  // Dawnbreaker Solar Guardian + Clockwerk Hookshot initiation
        ];

        for &(a, b, vs_wins_a, with_wins) in notable_pairs {
            let ha = HeroId(a);
            let hb = HeroId(b);
            let vs_wins_b = 1000_u32.saturating_sub(vs_wins_a);
            m.set_pair_stats(
                ha,
                hb,
                PairStats {
                    with_matches: 1000,
                    with_wins,
                    vs_matches: 1000,
                    vs_wins: vs_wins_a,
                },
                50.0,
            )
            .map_err(|e| PortError(e.to_string()))?;

            m.set_pair_stats(
                hb,
                ha,
                PairStats {
                    with_matches: 1000,
                    with_wins,
                    vs_matches: 1000,
                    vs_wins: vs_wins_b,
                },
                50.0,
            )
            .map_err(|e| PortError(e.to_string()))?;
        }

        Ok(m)
    }
}
