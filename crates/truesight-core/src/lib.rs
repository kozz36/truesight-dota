pub mod domain;
pub mod math;
pub mod ports;

#[cfg(test)]
mod tests {
    use crate::domain::{AttackType, DraftPick, DraftState, HeroId, HeroProfile, PairStats, Role};
    use crate::math::{
        MatchupMatrix, ModelWeights, bayesian_winrate, evaluate_draft_5v5,
        expected_head_to_head_wr, recommend_heroes,
    };

    fn sample_matrix() -> MatchupMatrix {
        let mut m = MatchupMatrix::new();
        // Hero 1: Anti-Mage (strong vs Storm Spirit, weak vs Axe)
        m.upsert_hero(HeroProfile {
            id: HeroId(1),
            slug: "npc_dota_hero_antimage".into(),
            display_name: "Anti-Mage".into(),
            primary_roles: vec![Role::Carry],
            role_winrates: [0.515, 0.460, 0.440, 0.410, 0.410],
            personal_matches: 40,
            personal_wins: 26,
            attack_type: AttackType::Melee,
            has_disable: false,
            has_initiation: false,
            trend_delta: 0.0,
            comfort_decay: 1.0,
        })
        .unwrap();

        // Hero 2: Axe (strong vs Anti-Mage)
        m.upsert_hero(HeroProfile {
            id: HeroId(2),
            slug: "npc_dota_hero_axe".into(),
            display_name: "Axe".into(),
            primary_roles: vec![Role::Offlane],
            role_winrates: [0.470, 0.490, 0.535, 0.480, 0.460],
            personal_matches: 15,
            personal_wins: 9,
            attack_type: AttackType::Melee,
            has_disable: true,
            has_initiation: true,
            trend_delta: 0.0,
            comfort_decay: 1.0,
        })
        .unwrap();

        // Hero 17: Storm Spirit (countered by Anti-Mage)
        m.upsert_hero(HeroProfile {
            id: HeroId(17),
            slug: "npc_dota_hero_storm_spirit".into(),
            display_name: "Storm Spirit".into(),
            primary_roles: vec![Role::Mid],
            role_winrates: [0.480, 0.510, 0.460, 0.440, 0.430],
            personal_matches: 0,
            personal_wins: 0,
            attack_type: AttackType::Ranged,
            has_disable: true,
            has_initiation: true,
            trend_delta: 0.0,
            comfort_decay: 1.0,
        })
        .unwrap();

        // Hero 97: Magnus (strong synergy with Anti-Mage)
        m.upsert_hero(HeroProfile {
            id: HeroId(97),
            slug: "npc_dota_hero_magnataur".into(),
            display_name: "Magnus".into(),
            primary_roles: vec![Role::Offlane, Role::Mid],
            role_winrates: [0.480, 0.515, 0.525, 0.495, 0.480],
            personal_matches: 20,
            personal_wins: 12,
            attack_type: AttackType::Melee,
            has_disable: true,
            has_initiation: true,
            trend_delta: 0.0,
            comfort_decay: 1.0,
        })
        .unwrap();

        // Anti-Mage vs Storm Spirit: AM wins 620 / 1000
        m.set_pair_stats(
            HeroId(1),
            HeroId(17),
            PairStats {
                with_matches: 400,
                with_wins: 195,
                vs_matches: 1000,
                vs_wins: 620,
            },
            50.0,
        )
        .unwrap();

        // Anti-Mage vs Axe: AM winsonly 410 / 1000
        m.set_pair_stats(
            HeroId(1),
            HeroId(2),
            PairStats {
                with_matches: 500,
                with_wins: 265,
                vs_matches: 1000,
                vs_wins: 410,
            },
            50.0,
        )
        .unwrap();

        // Anti-Mage + Magnus synergy: 580 / 1000 together
        m.set_pair_stats(
            HeroId(1),
            HeroId(97),
            PairStats {
                with_matches: 1000,
                with_wins: 580,
                vs_matches: 600,
                vs_wins: 300,
            },
            50.0,
        )
        .unwrap();
        m.set_pair_stats(
            HeroId(97),
            HeroId(1),
            PairStats {
                with_matches: 1000,
                with_wins: 580,
                vs_matches: 600,
                vs_wins: 300,
            },
            50.0,
        )
        .unwrap();

        m
    }

    #[test]
    fn bayesian_shrinkage_dampens_small_sample_extremes() {
        let small = bayesian_winrate(2, 2, 0.50, 50.0);
        let large = bayesian_winrate(600, 1000, 0.50, 50.0);
        assert!(small < 0.53, "2/2 games should shrink near 50%: {small}");
        assert!(large > 0.58, "600/1000 games should retain strong signal: {large}");
    }

    #[test]
    fn expected_wr_respects_log_odds_symmetry() {
        let e_ab = expected_head_to_head_wr(0.55, 0.45);
        let e_ba = expected_head_to_head_wr(0.45, 0.55);
        assert!((e_ab + e_ba - 1.0).abs() < 1e-5);
        assert!(e_ab > 0.55);
    }

    #[test]
    fn draft_5v5_populates_green_and_red_counter_and_synergy_cells() {
        let matrix = sample_matrix();
        let weights = ModelWeights::default();
        let mut draft = DraftState::default();

        // Ally 0: Anti-Mage, Ally 2: Magnus
        draft
            .set_ally(
                0,
                Some(DraftPick {
                    hero_id: HeroId(1),
                    role: Some(Role::Carry),
                }),
            )
            .unwrap();
        draft
            .set_ally(
                2,
                Some(DraftPick {
                    hero_id: HeroId(97),
                    role: Some(Role::Offlane),
                }),
            )
            .unwrap();

        // Enemy 1: Storm Spirit (countered by AM -> positive/green), Enemy 2: Axe (counters AM -> negative/red)
        draft
            .set_enemy(
                1,
                Some(DraftPick {
                    hero_id: HeroId(17),
                    role: Some(Role::Mid),
                }),
            )
            .unwrap();
        draft
            .set_enemy(
                2,
                Some(DraftPick {
                    hero_id: HeroId(2),
                    role: Some(Role::Offlane),
                }),
            )
            .unwrap();

        let analysis = evaluate_draft_5v5(&draft, &matrix, &weights);
        let am_vs_storm = analysis.counter_grid[0][1].expect("AM vs Storm cell populated");
        let am_vs_axe = analysis.counter_grid[0][2].expect("AM vs Axe cell populated");
        let am_with_mag = analysis.ally_synergy_grid[0][2].expect("AM + Magnus synergy populated");

        assert!(am_vs_storm > 0.05, "AM vs Storm should be positive (green): {am_vs_storm}");
        assert!(am_vs_axe < -0.05, "AM vs Axe should be negative (red): {am_vs_axe}");
        assert!(am_with_mag > 0.04, "AM + Magnus should have positive synergy: {am_with_mag}");
        assert!(analysis.ally_win_probability > 0.50);
    }

    #[test]
    fn duplicate_hero_or_invalid_slot_is_rejected_without_mutating_state() {
        let mut draft = DraftState::default();
        draft
            .set_ally(
                0,
                Some(DraftPick {
                    hero_id: HeroId(1),
                    role: Some(Role::Carry),
                }),
            )
            .unwrap();
        let snapshot = draft;

        assert!(
            draft
                .set_enemy(
                    0,
                    Some(DraftPick {
                        hero_id: HeroId(1),
                        role: Some(Role::Carry),
                    })
                )
                .is_err()
        );
        assert_eq!(draft, snapshot);

        assert!(draft.set_ally(9, None).is_err());
        assert_eq!(draft, snapshot);
    }

    #[test]
    fn recommend_heroes_ranks_counter_and_synergy_picks_first() {
        let matrix = sample_matrix();
        let weights = ModelWeights::default();
        let mut draft = DraftState {
            target_role: Role::Carry,
            ..DraftState::default()
        };
        // Ally has Magnus (97), Enemy has Storm Spirit (17)
        draft
            .set_ally(
                2,
                Some(DraftPick {
                    hero_id: HeroId(97),
                    role: Some(Role::Offlane),
                }),
            )
            .unwrap();
        draft
            .set_enemy(
                1,
                Some(DraftPick {
                    hero_id: HeroId(17),
                    role: Some(Role::Mid),
                }),
            )
            .unwrap();

        let recs = recommend_heroes(&draft, &matrix, &weights, 5);
        assert!(!recs.is_empty());
        assert_eq!(
            recs[0].hero_id,
            HeroId(1),
            "Anti-Mage should rank #1 with Magnus ally + Storm Spirit enemy"
        );
        assert!(recs[0].net_probability_delta > 0.05);
    }

    #[test]
    fn statistical_engine_v2_applies_trend_comfort_decay_and_composition_balance() {
        use crate::domain::AttackType;

        let mut m = MatchupMatrix::new();
        // 5 Melee carries with 0 disables on Ally side (structural composition flaw)
        for id in 1..=5 {
            m.upsert_hero(HeroProfile {
                id: HeroId(id),
                slug: format!("melee_{id}"),
                display_name: format!("Melee {id}"),
                primary_roles: vec![Role::Carry, Role::Mid, Role::Offlane, Role::SoftSupport, Role::HardSupport],
                role_winrates: [0.50; 5],
                personal_matches: 40,
                personal_wins: 28,
                attack_type: AttackType::Melee,
                has_disable: false,
                has_initiation: false,
                trend_delta: 0.015,
                comfort_decay: 0.50,
            })
            .unwrap();
        }
        // Balanced Enemy side (2 Melee with disable/init + 3 Ranged with disable)
        for id in 6..=10 {
            let is_melee = id <= 7;
            m.upsert_hero(HeroProfile {
                id: HeroId(id),
                slug: format!("bal_{id}"),
                display_name: format!("Balanced {id}"),
                primary_roles: vec![Role::Carry, Role::Mid, Role::Offlane, Role::SoftSupport, Role::HardSupport],
                role_winrates: [0.50; 5],
                personal_matches: 0,
                personal_wins: 0,
                attack_type: if is_melee { AttackType::Melee } else { AttackType::Ranged },
                has_disable: true,
                has_initiation: is_melee,
                trend_delta: 0.0,
                comfort_decay: 1.0,
            })
            .unwrap();
        }

        let mut draft = DraftState::default();
        for slot in 0..5 {
            draft
                .set_ally(
                    slot,
                    Some(DraftPick {
                        hero_id: HeroId((slot + 1) as u16),
                        role: Some(Role::ALL[slot]),
                    }),
                )
                .unwrap();
            draft
                .set_enemy(
                    slot,
                    Some(DraftPick {
                        hero_id: HeroId((slot + 6) as u16),
                        role: Some(Role::ALL[slot]),
                    }),
                )
                .unwrap();
        }

        let analysis = evaluate_draft_5v5(&draft, &m, &ModelWeights::default());
        assert!(
            analysis.total_composition_edge < -0.02,
            "5-Melee 0-Disable lineup must incur a negative composition edge vs balanced lineup: {}",
            analysis.total_composition_edge
        );
        assert!(
            analysis.total_meta_edge > 0.01,
            "Positive 7-day trend_delta (+1.5%) must increase total_meta_edge: {}",
            analysis.total_meta_edge
        );
        assert_eq!(analysis.ally_synergy_norm, 0.0);
        assert_eq!(analysis.enemy_synergy_norm, 0.0);
    }
}
