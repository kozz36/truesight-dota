use crate::domain::{AttackType, DraftPick, DraftState, HeroId, HeroProfile, PairStats, Role};
use crate::math::{
    MatchupMatrix, ModelWeights, bayesian_winrate, evaluate_draft_5v5, expected_head_to_head_wr,
    recommend_heroes,
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
    assert!(
        large > 0.58,
        "600/1000 games should retain strong signal: {large}"
    );
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

    assert!(
        am_vs_storm > 0.05,
        "AM vs Storm should be positive (green): {am_vs_storm}"
    );
    assert!(
        am_vs_axe < -0.05,
        "AM vs Axe should be negative (red): {am_vs_axe}"
    );
    assert!(
        am_with_mag > 0.04,
        "AM + Magnus should have positive synergy: {am_with_mag}"
    );
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
            primary_roles: vec![
                Role::Carry,
                Role::Mid,
                Role::Offlane,
                Role::SoftSupport,
                Role::HardSupport,
            ],
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
            primary_roles: vec![
                Role::Carry,
                Role::Mid,
                Role::Offlane,
                Role::SoftSupport,
                Role::HardSupport,
            ],
            role_winrates: [0.50; 5],
            personal_matches: 0,
            personal_wins: 0,
            attack_type: if is_melee {
                AttackType::Melee
            } else {
                AttackType::Ranged
            },
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

#[test]
fn personal_comfort_v3_penalizes_stale_heroes_rewards_active_volume_and_shifts_projected_win() {
    use crate::domain::AttackType;

    let mut m = MatchupMatrix::new();
    // Hero 1: Stale 7/8 hero unplayed for 10 months (decay = 0.015 -> g_eff = 0.12 < 0.15)
    m.upsert_hero(HeroProfile {
        id: HeroId(1),
        slug: "stale_am".into(),
        display_name: "Stale AM".into(),
        primary_roles: vec![Role::Carry],
        role_winrates: [0.51; 5],
        personal_matches: 8,
        personal_wins: 7,
        attack_type: AttackType::Melee,
        has_disable: false,
        has_initiation: false,
        trend_delta: 0.0,
        comfort_decay: 0.015,
    })
    .unwrap();

    // Hero 2: Active practiced hero (43 games, 24 blended wins, decay = 0.12 -> g_eff = 5.16)
    m.upsert_hero(HeroProfile {
        id: HeroId(2),
        slug: "active_db".into(),
        display_name: "Active DB".into(),
        primary_roles: vec![Role::Carry],
        role_winrates: [0.51; 5],
        personal_matches: 43,
        personal_wins: 24,
        attack_type: AttackType::Melee,
        has_disable: false,
        has_initiation: false,
        trend_delta: 0.0,
        comfort_decay: 0.12,
    })
    .unwrap();

    let draft = DraftState {
        target_role: Role::Carry,
        ..DraftState::default()
    };
    let recs = recommend_heroes(&draft, &m, &ModelWeights::default(), 5);
    let stale = recs.iter().find(|r| r.hero_id == HeroId(1)).unwrap();
    let active = recs.iter().find(|r| r.hero_id == HeroId(2)).unwrap();

    assert_eq!(
        stale.comfort_delta, 0.0,
        "Stale hero with g_eff < 0.15 must decay to 0.0 comfort: {}",
        stale.comfort_delta
    );
    assert!(
        active.comfort_delta > 0.015,
        "Actively practiced hero must receive positive comfort + volume mastery: {}",
        active.comfort_delta
    );
    assert!(
        active.projected_win_probability > stale.projected_win_probability,
        "Active comfort hero must have higher projected win% ({}) than stale hero ({})",
        active.projected_win_probability,
        stale.projected_win_probability
    );
}

#[test]
fn domain_ban_toggle_capacity_clear_and_role_bracket_invariants() {
    use crate::domain::{Bracket, DomainError, Faction};

    assert_eq!(Role::ALL.len(), 5);
    for (idx, role) in Role::ALL.iter().enumerate() {
        assert_eq!(role.index(), idx);
        assert!(!role.label().is_empty());
    }
    assert_eq!(Bracket::ALL.len(), 8);
    for (idx, bracket) in Bracket::ALL.iter().enumerate() {
        assert_eq!(bracket.index(), idx);
        assert!(!bracket.label().is_empty());
    }
    assert_eq!(Faction::Radiant.opposite(), Faction::Dire);
    assert_eq!(Faction::Dire.opposite(), Faction::Radiant);

    let mut draft = DraftState {
        bracket: Bracket::Divine,
        ally_faction: Faction::Dire,
        ..DraftState::default()
    };
    draft
        .set_ally(
            0,
            Some(DraftPick {
                hero_id: HeroId(1),
                role: Some(Role::Carry),
            }),
        )
        .unwrap();

    // Already-picked hero cannot be banned
    assert_eq!(
        draft.toggle_ban(HeroId(1)),
        Err(DomainError::DuplicateHero(HeroId(1)))
    );

    // Fill all 16 ban slots
    for i in 0..16_u16 {
        assert_eq!(draft.toggle_ban(HeroId(50 + i)), Ok(true));
    }
    // 17th distinct hero hits capacity error
    assert_eq!(draft.toggle_ban(HeroId(99)), Err(DomainError::BanListFull));

    // Toggling an existing banned hero removes it and frees the slot
    assert_eq!(draft.toggle_ban(HeroId(50)), Ok(false));
    assert_eq!(draft.toggle_ban(HeroId(99)), Ok(true));

    // clear() empties picks and bans while keeping bracket and faction preferences
    draft.clear();
    assert!(draft.allies.iter().all(Option::is_none));
    assert!(draft.enemies.iter().all(Option::is_none));
    assert!(draft.bans.iter().all(Option::is_none));
    assert_eq!(draft.bracket, Bracket::Divine);
    assert_eq!(draft.ally_faction, Faction::Dire);
}

#[test]
fn math_team_swap_complementarity_and_projected_win_parity() {
    // 1. Neutral-comfort 5v5 matrix with strict counter antisymmetry and synergy symmetry
    let mut neutral = MatchupMatrix::new();
    for id in 1..=10_u16 {
        neutral
            .upsert_hero(HeroProfile {
                id: HeroId(id),
                slug: format!("hero_{id}"),
                display_name: format!("Hero {id}"),
                primary_roles: vec![Role::ALL[((id - 1) % 5) as usize]],
                role_winrates: [
                    0.48 + 0.005 * (id as f32),
                    0.51 - 0.003 * (id as f32),
                    0.50,
                    0.49 + 0.002 * (id as f32),
                    0.52 - 0.004 * (id as f32),
                ],
                personal_matches: 0,
                personal_wins: 0,
                attack_type: if id % 2 == 0 {
                    AttackType::Melee
                } else {
                    AttackType::Ranged
                },
                has_disable: id != 1 && id != 6,
                has_initiation: id % 3 == 0,
                trend_delta: ((id as f32) - 5.5) * 0.002,
                comfort_decay: 1.0,
            })
            .unwrap();
    }
    for a in 1..=10_u16 {
        for b in (a + 1)..=10_u16 {
            let wins_vs = 500 + ((a * 7 + b * 11) % 90) as u32 - 45;
            let wins_with = 500 + ((a * 5 + b * 3) % 70) as u32 - 35;
            neutral
                .set_pair_stats(
                    HeroId(a),
                    HeroId(b),
                    PairStats {
                        with_matches: 1000,
                        with_wins: wins_with,
                        vs_matches: 1000,
                        vs_wins: wins_vs,
                    },
                    50.0,
                )
                .unwrap();
            neutral
                .set_pair_stats(
                    HeroId(b),
                    HeroId(a),
                    PairStats {
                        with_matches: 1000,
                        with_wins: wins_with,
                        vs_matches: 1000,
                        vs_wins: 1000 - wins_vs,
                    },
                    50.0,
                )
                .unwrap();
        }
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

    let weights = ModelWeights::default();
    let eval_a = evaluate_draft_5v5(&draft, &neutral, &weights);
    let mut swapped = draft;
    std::mem::swap(&mut swapped.allies, &mut swapped.enemies);
    let eval_b = evaluate_draft_5v5(&swapped, &neutral, &weights);

    assert!(
        (eval_a.ally_win_probability + eval_b.ally_win_probability - 1.0).abs() < 1e-6,
        "Team-swap complementarity violated: {} + {} != 1.0",
        eval_a.ally_win_probability,
        eval_b.ally_win_probability
    );

    // 2. Projected Win% parity even with active personal comfort and populated draft
    let rich_matrix = sample_matrix();
    let mut rec_draft = DraftState {
        target_role: Role::Carry,
        ..DraftState::default()
    };
    rec_draft
        .set_ally(
            2,
            Some(DraftPick {
                hero_id: HeroId(97),
                role: Some(Role::Offlane),
            }),
        )
        .unwrap();
    rec_draft
        .set_enemy(
            1,
            Some(DraftPick {
                hero_id: HeroId(17),
                role: Some(Role::Mid),
            }),
        )
        .unwrap();

    for rec in recommend_heroes(&rec_draft, &rich_matrix, &weights, 10) {
        let mut simulated = rec_draft;
        simulated.allies[rec_draft.target_role.index()] = Some(DraftPick {
            hero_id: rec.hero_id,
            role: Some(rec_draft.target_role),
        });
        let direct = evaluate_draft_5v5(&simulated, &rich_matrix, &weights);
        assert!(
            (direct.ally_win_probability - rec.projected_win_probability).abs() < 1e-6,
            "Projected Win% parity drift for {:?}: {} vs {}",
            rec.hero_id,
            rec.projected_win_probability,
            direct.ally_win_probability
        );
    }
}

#[test]
fn composition_rules_cover_all_six_structural_cases() {
    use crate::math::evaluate_team_composition;

    let build_roster = |specs: &[(AttackType, bool, bool)]| {
        let mut m = MatchupMatrix::new();
        let mut picks = [None; 5];
        for (idx, &(atk, dis, init)) in specs.iter().enumerate() {
            let id = HeroId((idx + 1) as u16);
            m.upsert_hero(HeroProfile {
                id,
                slug: format!("h_{idx}"),
                display_name: format!("Hero {idx}"),
                primary_roles: vec![Role::ALL[idx]],
                role_winrates: [0.50; 5],
                personal_matches: 0,
                personal_wins: 0,
                attack_type: atk,
                has_disable: dis,
                has_initiation: init,
                trend_delta: 0.0,
                comfort_decay: 1.0,
            })
            .unwrap();
            picks[idx] = Some(DraftPick {
                hero_id: id,
                role: Some(Role::ALL[idx]),
            });
        }
        (m, picks)
    };

    // 1. 5 melee + >=2 disables (-0.018)
    let (m1, p1) = build_roster(&[
        (AttackType::Melee, true, false),
        (AttackType::Melee, true, false),
        (AttackType::Melee, false, false),
        (AttackType::Melee, false, false),
        (AttackType::Melee, false, false),
    ]);
    assert!((evaluate_team_composition(&p1, &m1) - (-0.018)).abs() < 1e-6);

    // 2. 4 melee + 1 ranged + >=2 disables (-0.007)
    let (m2, p2) = build_roster(&[
        (AttackType::Melee, true, false),
        (AttackType::Melee, true, false),
        (AttackType::Melee, false, false),
        (AttackType::Melee, false, false),
        (AttackType::Ranged, false, false),
    ]);
    assert!((evaluate_team_composition(&p2, &m2) - (-0.007)).abs() < 1e-6);

    // 3. 5 ranged + >=2 disables (-0.008)
    let (m3, p3) = build_roster(&[
        (AttackType::Ranged, true, false),
        (AttackType::Ranged, true, false),
        (AttackType::Ranged, false, false),
        (AttackType::Ranged, false, false),
        (AttackType::Ranged, false, false),
    ]);
    assert!((evaluate_team_composition(&p3, &m3) - (-0.008)).abs() < 1e-6);

    // 4. 2 melee + 3 ranged + 0 disables (-0.015)
    let (m4, p4) = build_roster(&[
        (AttackType::Melee, false, false),
        (AttackType::Melee, false, false),
        (AttackType::Ranged, false, false),
        (AttackType::Ranged, false, false),
        (AttackType::Ranged, false, false),
    ]);
    assert!((evaluate_team_composition(&p4, &m4) - (-0.015)).abs() < 1e-6);

    // 5. 2 melee + 3 ranged + 1 disable + 1 init (-0.005)
    let (m5, p5) = build_roster(&[
        (AttackType::Melee, true, true),
        (AttackType::Melee, false, false),
        (AttackType::Ranged, false, false),
        (AttackType::Ranged, false, false),
        (AttackType::Ranged, false, false),
    ]);
    assert!((evaluate_team_composition(&p5, &m5) - (-0.005)).abs() < 1e-6);

    // 6. Balanced: 2 melee + 3 ranged + 2 disables + 1 init (+0.005)
    let (m6, p6) = build_roster(&[
        (AttackType::Melee, true, true),
        (AttackType::Melee, false, false),
        (AttackType::Ranged, true, false),
        (AttackType::Ranged, false, false),
        (AttackType::Ranged, false, false),
    ]);
    assert!((evaluate_team_composition(&p6, &m6) - 0.005).abs() < 1e-6);
}
