use truesight_core::domain::{Bracket, HeroId, HeroProfile, PairStats, Role};
use truesight_core::math::MatchupMatrix;
use truesight_core::ports::{PortError, StatsRepository};

/// Embedded high-MMR (STRATZ/OpenDota baseline) repository with deterministic
/// pairwise counters, synergies, and personal comfort history.
#[derive(Debug, Clone, Default)]
pub struct SeededMetaRepository {
    pub custom_personal_pool: Vec<(HeroId, u32, u32)>,
}

impl SeededMetaRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatsRepository for SeededMetaRepository {
    type Error = PortError;

    fn load_matrix(&self, _bracket: Bracket) -> Result<MatchupMatrix, Self::Error> {
        let mut m = MatchupMatrix::new();

        let catalog: &[(u16, &str, &str, &[Role], [f32; 5], u32, u32)] = &[
            (1, "npc_dota_hero_antimage", "Anti-Mage", &[Role::Carry], [0.518, 0.450, 0.440, 0.410, 0.400], 42, 27),
            (2, "npc_dota_hero_axe", "Axe", &[Role::Offlane], [0.460, 0.485, 0.534, 0.475, 0.455], 30, 18),
            (5, "npc_dota_hero_crystal_maiden", "Crystal Maiden", &[Role::HardSupport, Role::SoftSupport], [0.420, 0.440, 0.450, 0.518, 0.531], 25, 15),
            (6, "npc_dota_hero_drow_ranger", "Drow Ranger", &[Role::Carry], [0.524, 0.485, 0.445, 0.420, 0.415], 18, 11),
            (7, "npc_dota_hero_earthshaker", "Earthshaker", &[Role::SoftSupport, Role::Mid, Role::Offlane], [0.450, 0.508, 0.502, 0.519, 0.485], 35, 21),
            (8, "npc_dota_hero_juggernaut", "Juggernaut", &[Role::Carry], [0.529, 0.475, 0.450, 0.425, 0.420], 55, 34),
            (11, "npc_dota_hero_nevermore", "Shadow Fiend", &[Role::Mid, Role::Carry], [0.522, 0.526, 0.465, 0.430, 0.420], 20, 12),
            (12, "npc_dota_hero_phantom_lancer", "Phantom Lancer", &[Role::Carry], [0.506, 0.460, 0.440, 0.410, 0.405], 14, 8),
            (13, "npc_dota_hero_puck", "Puck", &[Role::Mid], [0.460, 0.514, 0.482, 0.465, 0.445], 12, 7),
            (14, "npc_dota_hero_pudge", "Pudge", &[Role::SoftSupport, Role::Offlane, Role::Mid], [0.475, 0.505, 0.512, 0.518, 0.502], 60, 35),
            (17, "npc_dota_hero_storm_spirit", "Storm Spirit", &[Role::Mid], [0.465, 0.516, 0.455, 0.440, 0.430], 22, 13),
            (20, "npc_dota_hero_vengefulspirit", "Vengeful Spirit", &[Role::HardSupport, Role::SoftSupport, Role::Offlane], [0.485, 0.490, 0.524, 0.528, 0.532], 19, 12),
            (26, "npc_dota_hero_lion", "Lion", &[Role::HardSupport, Role::SoftSupport], [0.420, 0.465, 0.450, 0.508, 0.512], 40, 22),
            (31, "npc_dota_hero_lich", "Lich", &[Role::HardSupport], [0.415, 0.445, 0.450, 0.515, 0.538], 28, 18),
            (41, "npc_dota_hero_faceless_void", "Faceless Void", &[Role::Carry], [0.519, 0.455, 0.470, 0.420, 0.410], 38, 24),
            (44, "npc_dota_hero_phantom_assassin", "Phantom Assassin", &[Role::Carry], [0.509, 0.470, 0.445, 0.420, 0.410], 33, 19),
            (54, "npc_dota_hero_life_stealer", "Lifestealer", &[Role::Carry], [0.527, 0.465, 0.475, 0.430, 0.420], 29, 18),
            (74, "npc_dota_hero_invoker", "Invoker", &[Role::Mid, Role::SoftSupport], [0.465, 0.512, 0.475, 0.504, 0.480], 45, 27),
            (86, "npc_dota_hero_rubick", "Rubick", &[Role::SoftSupport, Role::Mid], [0.430, 0.492, 0.460, 0.511, 0.495], 50, 29),
            (94, "npc_dota_hero_medusa", "Medusa", &[Role::Carry], [0.536, 0.495, 0.460, 0.420, 0.410], 16, 11),
            (97, "npc_dota_hero_magnataur", "Magnus", &[Role::Offlane, Role::Mid], [0.475, 0.514, 0.522, 0.495, 0.480], 24, 15),
            (104, "npc_dota_hero_legion_commander", "Legion Commander", &[Role::Offlane], [0.475, 0.495, 0.528, 0.470, 0.455], 31, 19),
            (106, "npc_dota_hero_ember_spirit", "Ember Spirit", &[Role::Mid], [0.480, 0.515, 0.475, 0.455, 0.440], 27, 16),
            (111, "npc_dota_hero_oracle", "Oracle", &[Role::HardSupport], [0.415, 0.450, 0.445, 0.508, 0.526], 21, 14),
            (120, "npc_dota_hero_pangolier", "Pangolier", &[Role::Mid, Role::Offlane], [0.465, 0.510, 0.505, 0.475, 0.460], 15, 9),
        ];

        for &(id, slug, name, roles, role_winrates, p_matches, p_wins) in catalog {
            m.upsert_hero(HeroProfile {
                id: HeroId(id),
                slug: slug.to_string(),
                display_name: name.to_string(),
                primary_roles: roles.to_vec(),
                role_winrates,
                personal_matches: p_matches,
                personal_wins: p_wins,
            })
            .map_err(|e| PortError(e.to_string()))?;
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

        // High-confidence STRATZ/Dotabuff matchup & synergy pairs:
        // (hero_a, hero_b, vs_wins_out_of_1000, with_wins_out_of_1000)
        let notable_pairs: &[(u16, u16, u32, u32)] = &[
            (1, 94, 635, 455),   // Anti-Mage hard-counters Medusa
            (1, 17, 610, 485),   // Anti-Mage counters Storm Spirit
            (1, 74, 575, 495),   // Anti-Mage counters Invoker
            (1, 2, 415, 535),    // Axe counters Anti-Mage; good synergy together
            (1, 104, 420, 530),  // Legion Commander counters Anti-Mage
            (1, 97, 510, 585),   // Magnus Empower + Reverse Polarity synergy with Anti-Mage
            (1, 26, 535, 545),   // Lion mana drain + AM Mana Void synergy
            (7, 12, 645, 495),   // Earthshaker hard-counters Phantom Lancer (Echo Slam)
            (2, 12, 615, 490),   // Axe hard-counters Phantom Lancer & Drow
            (2, 6, 585, 540),    // Axe Call counters Drow Ranger; good frontline synergy with Drow
            (2, 44, 570, 520),   // Axe Blade Mail counters Phantom Assassin
            (41, 74, 545, 595),  // Faceless Void Chronosphere + Invoker Cataclysm/Sunstrike
            (41, 31, 530, 580),  // Faceless Void + Lich Chain Frost
            (41, 6, 575, 480),   // Faceless Void Time Walk jumps Drow Ranger
            (41, 111, 435, 560), // Oracle False Promise saves allies inside Chronosphere
            (6, 20, 530, 590),   // Drow Ranger + Vengeful Spirit aura synergy
            (8, 5, 535, 585),    // Juggernaut Blade Fury + Crystal Maiden Frostbite lane combo
            (8, 97, 515, 575),   // Juggernaut + Magnus Empower
            (54, 17, 465, 585),  // Lifestealer Infest bomb + Storm Spirit / Puck
            (54, 13, 470, 580),  // Lifestealer Infest + Puck
            (54, 2, 575, 525),   // Lifestealer Feast counters high-HP strength cores
            (104, 17, 585, 540), // Legion Duel locks down Storm Spirit
            (104, 13, 575, 535), // Legion Duel locks down Puck
            (104, 106, 570, 530),// Legion Duel locks down Ember Spirit
            (104, 111, 410, 565),// Oracle False Promise ruins enemy Legion Duel
            (86, 7, 555, 515),   // Rubick steals Fissure/Echo Slam vs Earthshaker
            (86, 97, 560, 520),  // Rubick steals Reverse Polarity vs Magnus
            (11, 5, 550, 565),   // Shadow Fiend + Crystal Maiden aura/setup
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
