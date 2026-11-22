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
        assert_eq!(
            Bracket::parse_label(&bracket.label().to_ascii_uppercase()),
            Some(*bracket)
        );
        let tier_base = ((idx + 1) * 10) as u8;
        assert_eq!(Bracket::from_rank_tier(tier_base), Some(*bracket));
        assert_eq!(Bracket::from_rank_tier(tier_base + 5), Some(*bracket));
    }
    assert_eq!(Bracket::from_rank_tier(0), None);
    assert_eq!(Bracket::from_rank_tier(9), None);
    assert_eq!(Bracket::from_rank_tier(90), None);
    assert_eq!(Bracket::parse_label("auto"), None);
    assert_eq!(Bracket::parse_label("unknown"), None);
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

// ---------------------------------------------------------------------------
// Item-build recommender & counter-trait table tests (#67)
// ---------------------------------------------------------------------------

struct MockItemRepo {
    catalog: Vec<crate::domain::ItemCatalogEntry>,
    stats: Vec<crate::domain::HeroItemStat>,
}

impl crate::ports::ItemMetaRepository for MockItemRepo {
    fn item_catalog(
        &self,
        item_id: crate::domain::ItemId,
    ) -> Option<crate::domain::ItemCatalogEntry> {
        self.catalog.iter().find(|e| e.id == item_id).cloned()
    }

    fn hero_item_stats(
        &self,
        _hero_id: HeroId,
        _role: Role,
        _bracket: crate::domain::Bracket,
    ) -> Result<Vec<crate::domain::HeroItemStat>, crate::ports::PortError> {
        Ok(self.stats.clone())
    }
}

fn catalog_entry(
    id: u16,
    name: &str,
    cost: u32,
    components: &[u16],
    is_recipe: bool,
    is_consumable_drop: bool,
) -> crate::domain::ItemCatalogEntry {
    crate::domain::ItemCatalogEntry {
        id: crate::domain::ItemId(id),
        slug: name.to_lowercase().replace(' ', "_"),
        display_name: name.to_string(),
        cost,
        components: components
            .iter()
            .copied()
            .map(crate::domain::ItemId)
            .collect(),
        is_recipe,
        is_consumable_drop,
    }
}

fn item_stat(
    id: u16,
    phase: crate::domain::ItemPhase,
    avg_time_mins: f32,
    match_count: u32,
    win_count: u32,
) -> crate::domain::HeroItemStat {
    crate::domain::HeroItemStat {
        item_id: crate::domain::ItemId(id),
        phase,
        avg_time_mins,
        match_count,
        win_count,
    }
}

#[test]
fn counter_trait_table_is_consistent_and_patch_versioned() {
    use crate::domain::{
        COUNTER_TRAIT_DATA_TOML, COUNTER_TRAIT_TABLE_PATCH, COUNTER_TRAIT_TABLE_REVIEWED_AT,
        CounterTrait, ItemId, all_referenced_counter_item_ids, counter_items_for_trait,
        counter_trait_table_patch, counter_trait_table_reviewed_at, hero_counter_traits,
    };

    assert_eq!(COUNTER_TRAIT_TABLE_PATCH, "7.40");
    assert_eq!(COUNTER_TRAIT_TABLE_REVIEWED_AT, "2026-11-21");
    assert_eq!(counter_trait_table_patch(), COUNTER_TRAIT_TABLE_PATCH);
    assert_eq!(
        counter_trait_table_reviewed_at(),
        COUNTER_TRAIT_TABLE_REVIEWED_AT
    );
    assert!(COUNTER_TRAIT_DATA_TOML.contains("patch = \"7.40\""));
    assert!(COUNTER_TRAIT_DATA_TOML.contains("reviewed_at = \"2026-11-21\""));

    let all_traits = [
        CounterTrait::Evasion,
        CounterTrait::HeavyMagicDisable,
        CounterTrait::HighHealingRegen,
        CounterTrait::EtherealGhostSave,
        CounterTrait::CriticalPassives,
        CounterTrait::IllusionsSummons,
        CounterTrait::Invisibility,
        CounterTrait::HighPhysicalBurst,
    ];

    let all_referenced = all_referenced_counter_item_ids();
    for (idx, a) in all_referenced.iter().enumerate() {
        for b in &all_referenced[idx + 1..] {
            assert_ne!(
                a, b,
                "duplicate ItemId {a:?} in all_referenced_counter_item_ids"
            );
        }
    }

    for trait_kind in all_traits {
        assert!(!trait_kind.label().is_empty());
        let items = counter_items_for_trait(trait_kind);
        assert!(
            !items.is_empty(),
            "trait {trait_kind:?} must map to at least one counter item"
        );
        for id in items {
            assert!(
                all_referenced.contains(id),
                "counter item {id:?} missing from all_referenced_counter_item_ids"
            );
        }
    }

    let pa_traits = hero_counter_traits(HeroId(44));
    assert!(pa_traits.contains(&CounterTrait::Evasion));
    assert!(pa_traits.contains(&CounterTrait::CriticalPassives));
    assert!(counter_items_for_trait(CounterTrait::Evasion).contains(&ItemId(135)));
    assert!(counter_items_for_trait(CounterTrait::CriticalPassives).contains(&ItemId(249)));
}

#[test]
fn recommend_item_build_is_deterministic_and_excludes_drops_and_recipes() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(11, "Quelling Blade", 100, &[], false, false),
            catalog_entry(63, "Power Treads", 1400, &[29, 25], false, false),
            catalog_entry(147, "Manta Style", 4600, &[170, 20], false, false),
            catalog_entry(4205, "Great Healing Lotus", 0, &[], false, true),
            catalog_entry(999, "Manta Recipe", 500, &[], true, false),
        ],
        stats: vec![
            item_stat(11, ItemPhase::Starting, 0.0, 900, 480),
            item_stat(4205, ItemPhase::Early, 6.0, 1200, 700),
            item_stat(999, ItemPhase::Core, 19.0, 1100, 620),
            item_stat(63, ItemPhase::Early, 8.0, 800, 430),
            item_stat(147, ItemPhase::Core, 20.0, 750, 420),
        ],
    };

    let enemies = [None; 5];
    let b1 = recommend_item_build(
        &repo,
        HeroId(1),
        Role::Carry,
        Bracket::Legend,
        &enemies,
        &[],
        300,
    )
    .expect("build should succeed");
    let b2 = recommend_item_build(
        &repo,
        HeroId(1),
        Role::Carry,
        Bracket::Legend,
        &enemies,
        &[],
        300,
    )
    .expect("build should succeed");

    assert_eq!(b1, b2);
    assert_eq!(b1.starting_items, vec![ItemId(11)]);
    assert!(
        b1.steps
            .iter()
            .all(|s| s.item_id != ItemId(4205) && s.item_id != ItemId(999))
    );
    assert_eq!(
        b1.steps.iter().map(|s| s.item_id).collect::<Vec<_>>(),
        vec![ItemId(63), ItemId(147)]
    );
    assert_eq!(b1.next_unowned_index, Some(0));
}

#[test]
fn recommend_item_build_marks_owned_and_advances_next_unowned_index() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(63, "Power Treads", 1400, &[], false, false),
            catalog_entry(147, "Manta Style", 4600, &[], false, false),
            catalog_entry(139, "Butterfly", 4975, &[], false, false),
        ],
        stats: vec![
            item_stat(63, ItemPhase::Early, 8.0, 800, 430),
            item_stat(147, ItemPhase::Core, 20.0, 750, 420),
            item_stat(139, ItemPhase::Late, 31.0, 600, 350),
        ],
    };

    let enemies = [None; 5];
    let b_one = recommend_item_build(
        &repo,
        HeroId(1),
        Role::Carry,
        Bracket::Ancient,
        &enemies,
        &[ItemId(63)],
        900,
    )
    .unwrap();
    assert!(b_one.steps[0].owned);
    assert!(!b_one.steps[1].owned);
    assert_eq!(b_one.next_unowned_index, Some(1));

    let b_all = recommend_item_build(
        &repo,
        HeroId(1),
        Role::Carry,
        Bracket::Ancient,
        &enemies,
        &[ItemId(63), ItemId(147), ItemId(139)],
        2400,
    )
    .unwrap();
    assert!(b_all.steps.iter().all(|s| s.owned));
    assert_eq!(b_all.next_unowned_index, None);
}

#[test]
fn recommend_item_build_prioritizes_partially_built_items() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(145, "Battle Fury", 4100, &[65, 69], false, false),
            catalog_entry(158, "Mjollnir", 5500, &[166, 55], false, false),
        ],
        stats: vec![
            item_stat(145, ItemPhase::Core, 16.0, 500, 265),
            item_stat(158, ItemPhase::Core, 16.0, 500, 265),
        ],
    };

    let enemies = [None; 5];
    // Player owns Hyperstone (55), a component of Mjollnir (158).
    let build = recommend_item_build(
        &repo,
        HeroId(1),
        Role::Carry,
        Bracket::Divine,
        &enemies,
        &[ItemId(55)],
        700,
    )
    .unwrap();

    assert_eq!(build.steps[0].item_id, ItemId(158));
    assert_eq!(build.steps[0].owned_components, 1);
    assert_eq!(build.steps[0].total_components, 2);
    assert_eq!(build.steps[0].variants[0].item_id, ItemId(145));
    assert!(build.steps[0].score > build.steps[0].variants[0].score);
}

#[test]
fn recommend_item_build_base_meta_vs_enemy_counter_traits() {
    use crate::domain::{Bracket, CounterTrait, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(63, "Power Treads", 1400, &[], false, false),
            catalog_entry(147, "Manta Style", 4600, &[], false, false),
            catalog_entry(139, "Butterfly", 4975, &[], false, false),
            catalog_entry(135, "Monkey King Bar", 4700, &[], false, false),
            catalog_entry(249, "Silver Edge", 5450, &[], false, false),
        ],
        stats: vec![
            item_stat(63, ItemPhase::Early, 8.0, 900, 480),
            item_stat(147, ItemPhase::Core, 20.0, 850, 470),
            item_stat(139, ItemPhase::Late, 30.0, 700, 390),
            item_stat(135, ItemPhase::Situational, 29.0, 220, 112),
            item_stat(249, ItemPhase::Situational, 28.0, 200, 102),
        ],
    };

    let no_counter_enemies = [Some(HeroId(1)), None, None, None, None];
    let base_build = recommend_item_build(
        &repo,
        HeroId(1),
        Role::Carry,
        Bracket::Immortal,
        &no_counter_enemies,
        &[],
        1200,
    )
    .unwrap();
    assert!(base_build.steps.iter().all(|s| s.counter_trait.is_none()));
    assert!(
        base_build.optional_items.is_empty(),
        "optional counter items must stay empty when no enemy activates counter traits: {:?}",
        base_build.optional_items
    );

    // Enemy Phantom Assassin (44) brings Evasion + CriticalPassives + HighPhysicalBurst.
    let pa_enemies = [Some(HeroId(44)), None, None, None, None];
    let counter_build = recommend_item_build(
        &repo,
        HeroId(1),
        Role::Carry,
        Bracket::Immortal,
        &pa_enemies,
        &[],
        1200,
    )
    .unwrap();

    assert_eq!(
        counter_build.steps.len(),
        3,
        "situational counters must not displace Early/Core/Late progression steps"
    );
    let mkb_opt = counter_build
        .optional_items
        .iter()
        .find(|item| item.item_id == ItemId(135))
        .expect("MKB should promote into Optional / Counters against PA on Carry");
    assert_eq!(mkb_opt.counter_trait, Some(CounterTrait::Evasion));
    assert!(mkb_opt.score > 0.5);

    let silver_edge_present = counter_build.optional_items.iter().any(|item| {
        item.item_id == ItemId(249) && item.counter_trait == Some(CounterTrait::CriticalPassives)
    });
    assert!(
        silver_edge_present,
        "Silver Edge should promote into Optional / Counters against PA on Carry"
    );
}

#[test]
fn recommend_item_build_filters_situational_counters_by_role_and_enemy_traits() {
    use crate::domain::{
        Bracket, CounterTrait, ItemId, ItemPhase, all_referenced_counter_item_ids,
    };
    use crate::math::recommend_item_build;

    let mut catalog = vec![
        catalog_entry(180, "Arcane Boots", 1500, &[], false, false),
        catalog_entry(102, "Force Staff", 2200, &[], false, false),
        catalog_entry(108, "Aghanim's Scepter", 4200, &[], false, false),
    ];
    let mut stats = vec![
        item_stat(180, ItemPhase::Early, 7.5, 900, 485),
        item_stat(102, ItemPhase::Core, 17.5, 850, 460),
        item_stat(108, ItemPhase::Late, 30.0, 650, 365),
    ];
    for &cid in all_referenced_counter_item_ids() {
        if catalog.iter().all(|c| c.id != cid) {
            catalog.push(catalog_entry(cid.0, "Counter", 2500, &[], false, false));
        }
        stats.push(item_stat(cid.0, ItemPhase::Situational, 21.0, 220, 114));
    }
    let repo = MockItemRepo { catalog, stats };

    // 1. Soft Support (Pos 4) vs PA + Shadow Shaman (no Invisibility!)
    let pa_shaman = [Some(HeroId(44)), Some(HeroId(27)), None, None, None];
    let sup_build = recommend_item_build(
        &repo,
        HeroId(88),
        Role::SoftSupport,
        Bracket::Ancient,
        &pa_shaman,
        &[],
        600,
    )
    .unwrap();
    assert!(
        sup_build
            .steps
            .iter()
            .all(|s| s.phase != ItemPhase::Situational),
        "primary timed steps must strictly contain Early/Core/Late items"
    );
    for forbidden in [
        30_u16, 40, 43, 90, 112, 135, 145, 158, 160, 225, 242, 249, 250,
    ] {
        assert!(
            sup_build
                .optional_items
                .iter()
                .all(|opt| opt.item_id != ItemId(forbidden)),
            "Pos 4 without invisible enemies must not get item #{forbidden}: {:?}",
            sup_build.optional_items
        );
    }
    assert!(
        sup_build
            .optional_items
            .iter()
            .any(|opt| opt.item_id == ItemId(226)
                && opt.counter_trait == Some(CounterTrait::HeavyMagicDisable)),
        "Pos 4 vs Shadow Shaman should see Lotus Orb in Optional / Counters"
    );

    // 2. Soft Support (Pos 4) vs Riki (Invisibility): detection stays in optional_items and is capped at <= 2.
    let riki_enemy = [Some(HeroId(32)), None, None, None, None];
    let invis_sup = recommend_item_build(
        &repo,
        HeroId(88),
        Role::SoftSupport,
        Bracket::Ancient,
        &riki_enemy,
        &[],
        600,
    )
    .unwrap();
    let invis_opts: Vec<_> = invis_sup
        .optional_items
        .iter()
        .filter(|o| o.counter_trait == Some(CounterTrait::Invisibility))
        .collect();
    assert!(
        !invis_opts.is_empty() && invis_opts.len() <= 2,
        "detection items should be present and capped at 2 in Optional: {invis_opts:?}"
    );
    assert!(
        invis_sup
            .steps
            .iter()
            .all(|s| !matches!(s.item_id.0, 30 | 40 | 43)),
        "Gem/Dust/Sentry must never displace timed Early/Core/Late steps"
    );

    // 3. Carry (Pos 1) vs Riki + Lion: gets Dust, never Gem (30) or Pipe (90).
    let riki_lion = [Some(HeroId(32)), Some(HeroId(26)), None, None, None];
    let carry_build = recommend_item_build(
        &repo,
        HeroId(95),
        Role::Carry,
        Bracket::Ancient,
        &riki_lion,
        &[],
        600,
    )
    .unwrap();
    assert!(
        carry_build
            .optional_items
            .iter()
            .all(|o| !matches!(o.item_id.0, 30 | 90 | 242)),
        "Pos 1 Carry must not be recommended Gem, Pipe, or Crimson Guard"
    );
    assert!(
        carry_build
            .optional_items
            .iter()
            .any(|o| o.item_id == ItemId(40)),
        "Pos 1 Carry vs Riki can see Dust in Optional / Counters"
    );

    // 4. Synced un-roled stats containing detection/counter items in Early/Core must never
    // leak Gem/Dust/Sentry into timed steps or role-ineligible Pipe onto Pos 4 Soft Support.
    let polluted_sync_repo = MockItemRepo {
        catalog: vec![
            catalog_entry(180, "Arcane Boots", 1500, &[], false, false),
            catalog_entry(30, "Gem of True Sight", 900, &[], false, false),
            catalog_entry(90, "Pipe of Insight", 3725, &[], false, false),
            catalog_entry(232, "Aether Lens", 2275, &[], false, false),
        ],
        stats: vec![
            item_stat(180, ItemPhase::Early, 8.0, 1200, 640),
            item_stat(30, ItemPhase::Core, 15.0, 900, 500),
            item_stat(90, ItemPhase::Core, 21.0, 800, 440),
            item_stat(232, ItemPhase::Core, 23.0, 1100, 600),
        ],
    };
    let no_invis_enemies = [Some(HeroId(27)), None, None, None, None];
    let clean_sup = recommend_item_build(
        &polluted_sync_repo,
        HeroId(88),
        Role::SoftSupport,
        Bracket::Ancient,
        &no_invis_enemies,
        &[],
        900,
    )
    .unwrap();
    assert!(
        clean_sup
            .steps
            .iter()
            .all(|s| !matches!(s.item_id.0, 30 | 90)),
        "Gem and role-ineligible Pipe must not leak into timed core steps: {:?}",
        clean_sup.steps
    );
    assert!(
        clean_sup.optional_items.is_empty(),
        "No-invis enemy + ineligible Pipe must leave optional_items empty: {:?}",
        clean_sup.optional_items
    );
}

#[test]
fn recommend_item_build_classifies_same_phase_variants_with_fewer_than_six_candidates() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(160, "Eye of Skadi", 5300, &[], false, false),
            catalog_entry(139, "Butterfly", 4975, &[], false, false),
        ],
        stats: vec![
            item_stat(160, ItemPhase::Late, 32.0, 600, 340),
            item_stat(139, ItemPhase::Late, 31.0, 600, 340),
        ],
    };

    let enemies = [None; 5];
    let build = recommend_item_build(
        &repo,
        HeroId(1),
        Role::Carry,
        Bracket::Immortal,
        &enemies,
        &[],
        1500,
    )
    .expect("build should succeed");

    assert_eq!(
        build.steps.len(),
        1,
        "two same-phase equal-score alternatives must collapse into 1 primary step + 1 variant"
    );
    assert_eq!(build.steps[0].item_id, ItemId(139));
    assert_eq!(build.steps[0].variants.len(), 1);
    assert_eq!(build.steps[0].variants[0].item_id, ItemId(160));
}

#[test]
fn recommend_item_build_preserves_owned_role_ineligible_counters_under_optional() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let enemies_no_traits = [None; 5];

    // 1a. Carry with Gem in Core stats: owned -> Optional (owned = true), never steps; unowned -> excluded.
    let carry_with_core_gem = MockItemRepo {
        catalog: vec![
            catalog_entry(63, "Power Treads", 1400, &[], false, false),
            catalog_entry(147, "Manta Style", 4550, &[], false, false),
            catalog_entry(30, "Gem of True Sight", 900, &[], false, false),
        ],
        stats: vec![
            item_stat(63, ItemPhase::Early, 7.0, 1200, 640),
            item_stat(147, ItemPhase::Core, 20.0, 1100, 610),
            item_stat(30, ItemPhase::Core, 15.0, 900, 500),
        ],
    };
    let carry_owned_core = recommend_item_build(
        &carry_with_core_gem,
        HeroId(95),
        Role::Carry,
        Bracket::Ancient,
        &enemies_no_traits,
        &[ItemId(30)],
        1020,
    )
    .unwrap();
    assert!(
        carry_owned_core
            .steps
            .iter()
            .all(|s| s.item_id != ItemId(30)),
        "owned Gem must not sit in timed steps"
    );
    assert!(
        carry_owned_core
            .optional_items
            .iter()
            .any(|o| o.item_id == ItemId(30) && o.owned),
        "owned Gem on Carry must remain visible under Optional: {:?}",
        carry_owned_core.optional_items
    );
    let carry_unowned_core = recommend_item_build(
        &carry_with_core_gem,
        HeroId(95),
        Role::Carry,
        Bracket::Ancient,
        &enemies_no_traits,
        &[],
        1020,
    )
    .unwrap();
    assert!(
        carry_unowned_core
            .steps
            .iter()
            .all(|s| s.item_id != ItemId(30))
            && carry_unowned_core
                .optional_items
                .iter()
                .all(|o| o.item_id != ItemId(30)),
        "unowned Gem on Carry without invisibility traits must not be recommended"
    );

    // 1b. Carry when Gem is omitted from stats (role-filtered repository): still preserved when owned!
    let carry_omitted_gem = MockItemRepo {
        catalog: carry_with_core_gem.catalog.clone(),
        stats: vec![
            item_stat(63, ItemPhase::Early, 7.0, 1200, 640),
            item_stat(147, ItemPhase::Core, 20.0, 1100, 610),
        ],
    };
    let carry_owned_omitted = recommend_item_build(
        &carry_omitted_gem,
        HeroId(95),
        Role::Carry,
        Bracket::Ancient,
        &enemies_no_traits,
        &[ItemId(30)],
        1020,
    )
    .unwrap();
    assert!(
        carry_owned_omitted
            .steps
            .iter()
            .all(|s| s.item_id != ItemId(30))
            && carry_owned_omitted
                .optional_items
                .iter()
                .any(|o| o.item_id == ItemId(30) && o.owned),
        "owned Gem omitted from stats must still appear in Optional on Carry: {:?}",
        carry_owned_omitted.optional_items
    );

    // 2. Soft Support with BKB (116): owned -> Optional (owned = true), never steps; unowned -> excluded.
    let sup_with_core_bkb = MockItemRepo {
        catalog: vec![
            catalog_entry(180, "Arcane Boots", 1500, &[], false, false),
            catalog_entry(232, "Aether Lens", 2275, &[], false, false),
            catalog_entry(116, "Black King Bar", 4050, &[], false, false),
        ],
        stats: vec![
            item_stat(180, ItemPhase::Early, 7.5, 1200, 640),
            item_stat(232, ItemPhase::Core, 18.0, 1100, 600),
            item_stat(116, ItemPhase::Core, 24.0, 800, 440),
        ],
    };
    let sup_owned_core = recommend_item_build(
        &sup_with_core_bkb,
        HeroId(88),
        Role::SoftSupport,
        Bracket::Ancient,
        &enemies_no_traits,
        &[ItemId(116)],
        1440,
    )
    .unwrap();
    assert!(
        sup_owned_core
            .steps
            .iter()
            .all(|s| s.item_id != ItemId(116)),
        "owned role-ineligible BKB must not appear in timed steps on Pos 4"
    );
    assert!(
        sup_owned_core
            .optional_items
            .iter()
            .any(|o| o.item_id == ItemId(116) && o.owned),
        "owned BKB on Soft Support must stay visible under Optional: {:?}",
        sup_owned_core.optional_items
    );
    let sup_unowned_core = recommend_item_build(
        &sup_with_core_bkb,
        HeroId(88),
        Role::SoftSupport,
        Bracket::Ancient,
        &enemies_no_traits,
        &[],
        1440,
    )
    .unwrap();
    assert!(
        sup_unowned_core
            .steps
            .iter()
            .all(|s| s.item_id != ItemId(116))
            && sup_unowned_core
                .optional_items
                .iter()
                .all(|o| o.item_id != ItemId(116)),
        "unowned role-ineligible BKB on Pos 4 must not be recommended"
    );

    let sup_omitted_bkb = MockItemRepo {
        catalog: sup_with_core_bkb.catalog,
        stats: vec![
            item_stat(180, ItemPhase::Early, 7.5, 1200, 640),
            item_stat(232, ItemPhase::Core, 18.0, 1100, 600),
        ],
    };
    let sup_owned_omitted = recommend_item_build(
        &sup_omitted_bkb,
        HeroId(88),
        Role::SoftSupport,
        Bracket::Ancient,
        &enemies_no_traits,
        &[ItemId(116)],
        1440,
    )
    .unwrap();
    assert!(
        sup_owned_omitted
            .steps
            .iter()
            .all(|s| s.item_id != ItemId(116))
            && sup_owned_omitted
                .optional_items
                .iter()
                .any(|o| o.item_id == ItemId(116) && o.owned),
        "owned BKB omitted from stats must still appear in Optional on Pos 4: {:?}",
        sup_owned_omitted.optional_items
    );
}

#[test]
fn recommend_item_build_suppresses_tiny_sample_noise_and_redundant_subcomponents() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(63, "Power Treads", 1400, &[], false, false),
            catalog_entry(69, "Perseverance", 1400, &[], false, false),
            catalog_entry(145, "Battle Fury", 3900, &[69], false, false),
            catalog_entry(170, "Yasha", 2000, &[], false, false),
            catalog_entry(147, "Manta Style", 4550, &[170], false, false),
            catalog_entry(149, "Crystalys", 2000, &[], false, false),
            catalog_entry(156, "Satanic", 5050, &[], false, false),
            catalog_entry(534, "Witch Blade", 2775, &[], false, false),
        ],
        stats: vec![
            item_stat(63, ItemPhase::Early, 9.0, 598, 258),
            item_stat(69, ItemPhase::Early, 10.5, 387, 195),
            item_stat(145, ItemPhase::Core, 16.0, 359, 165),
            item_stat(170, ItemPhase::Core, 18.0, 379, 175),
            item_stat(147, ItemPhase::Core, 23.5, 293, 152),
            item_stat(149, ItemPhase::Core, 24.0, 80, 48),
            item_stat(156, ItemPhase::Late, 26.0, 7, 7),
            item_stat(534, ItemPhase::Early, 7.5, 1, 1),
        ],
    };

    let enemies = [None; 5];
    let build = recommend_item_build(
        &repo,
        HeroId(41),
        Role::Carry,
        Bracket::Ancient,
        &enemies,
        &[],
        600,
    )
    .unwrap();

    assert!(
        build.steps.iter().all(|s| {
            !matches!(s.item_id.0, 69 | 170 | 156 | 534)
                && s.variants
                    .iter()
                    .all(|v| !matches!(v.item_id.0, 69 | 170 | 156 | 534))
        }),
        "Perseverance (69), bare Yasha (170 under Manta), and low-sample noise (Satanic 7/7, Witch Blade 1/1) must be excluded: {:?}",
        build.steps
    );
    for expected in [63_u16, 145, 147] {
        assert!(
            build.steps.iter().any(|s| s.item_id == ItemId(expected)),
            "high-volume item #{expected} must stay as primary step: {:?}",
            build.steps
        );
    }
    assert!(
        build.steps.iter().any(|s| {
            s.item_id == ItemId(149) || s.variants.iter().any(|v| v.item_id == ItemId(149))
        }),
        "standalone Crystalys (no Daedalus present) must be preserved in steps/variants: {:?}",
        build.steps
    );
}
