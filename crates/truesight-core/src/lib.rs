pub mod domain;
pub mod math;
pub mod ports;

#[cfg(test)]
mod tests {
    use crate::domain::{DraftPick, DraftState, HeroId, HeroProfile, PairStats, Role};
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
}
