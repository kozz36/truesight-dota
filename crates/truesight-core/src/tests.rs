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

#[test]
fn recommend_item_build_groups_mutually_exclusive_early_boots_as_variants() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let enemies = [None; 5];

    // 1. High-gap Power Treads (700) vs Phase Boots (120) -> 1 Early boot step with variant.
    let carry_repo = MockItemRepo {
        catalog: vec![
            catalog_entry(63, "Power Treads", 1400, &[], false, false),
            catalog_entry(50, "Phase Boots", 1450, &[], false, false),
            catalog_entry(168, "Desolator", 3500, &[], false, false),
        ],
        stats: vec![
            item_stat(63, ItemPhase::Early, 8.0, 700, 380),
            item_stat(50, ItemPhase::Early, 8.5, 120, 55),
            item_stat(168, ItemPhase::Core, 20.0, 500, 275),
        ],
    };
    let carry_build = recommend_item_build(
        &carry_repo,
        HeroId(32),
        Role::Carry,
        Bracket::Ancient,
        &enemies,
        &[],
        600,
    )
    .unwrap();
    let early_boot_primaries: Vec<_> = carry_build
        .steps
        .iter()
        .filter(|s| matches!(s.item_id.0, 50 | 63 | 180 | 214))
        .collect();
    assert_eq!(
        early_boot_primaries.len(),
        1,
        "mutually exclusive Early boots must collapse into 1 primary step: {:?}",
        carry_build.steps
    );
    assert_eq!(early_boot_primaries[0].item_id, ItemId(63));
    assert!(
        early_boot_primaries[0]
            .variants
            .iter()
            .any(|v| v.item_id == ItemId(50)),
        "Phase Boots must attach as variant of Power Treads: {:?}",
        early_boot_primaries[0].variants
    );

    // 2. Tranquil Boots (Early 7.5m) vs Arcane Boots (Core 13m) -> collapses into 1 primary boot.
    let support_repo = MockItemRepo {
        catalog: vec![
            catalog_entry(180, "Arcane Boots", 1500, &[], false, false),
            catalog_entry(214, "Tranquil Boots", 900, &[], false, false),
            catalog_entry(254, "Glimmer Cape", 2150, &[], false, false),
        ],
        stats: vec![
            item_stat(180, ItemPhase::Core, 13.0, 420, 230),
            item_stat(214, ItemPhase::Early, 7.5, 210, 108),
            item_stat(254, ItemPhase::Core, 20.0, 460, 260),
        ],
    };
    let sup_build = recommend_item_build(
        &support_repo,
        HeroId(5),
        Role::HardSupport,
        Bracket::Ancient,
        &enemies,
        &[],
        720,
    )
    .unwrap();
    assert_eq!(
        sup_build
            .steps
            .iter()
            .filter(|s| matches!(s.item_id.0, 180 | 214))
            .count(),
        1,
        "Tranquil + Arcane must collapse even across Early/Core threshold: {:?}",
        sup_build.steps
    );

    // 3. When player already owns Phase Boots (50), Phase Boots is primary and next_unowned advances.
    let owned_build = recommend_item_build(
        &carry_repo,
        HeroId(32),
        Role::Carry,
        Bracket::Ancient,
        &enemies,
        &[ItemId(50)],
        600,
    )
    .unwrap();
    let owned_boots: Vec<_> = owned_build
        .steps
        .iter()
        .filter(|s| matches!(s.item_id.0, 50 | 63))
        .collect();
    assert_eq!(owned_boots.len(), 1);
    assert_eq!(owned_boots[0].item_id, ItemId(50));
    assert!(owned_boots[0].owned);
    assert!(
        owned_boots[0]
            .variants
            .iter()
            .any(|v| v.item_id == ItemId(63))
    );
    assert_eq!(owned_build.next_unowned_index, Some(1));
    assert_eq!(owned_build.steps[1].item_id, ItemId(168));
}

#[test]
fn recommend_item_build_allocates_four_late_primary_steps() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(75, "Wraith Band", 505, &[], false, false),
            catalog_entry(63, "Power Treads", 1400, &[], false, false),
            catalog_entry(166, "Maelstrom", 2950, &[], false, false),
            catalog_entry(147, "Manta Style", 4550, &[], false, false),
            catalog_entry(158, "Mjollnir", 5500, &[166], false, false),
            catalog_entry(139, "Butterfly", 4975, &[], false, false),
            catalog_entry(160, "Eye of Skadi", 5300, &[], false, false),
            catalog_entry(141, "Daedalus", 5150, &[], false, false),
            catalog_entry(156, "Satanic", 5050, &[], false, false),
        ],
        stats: vec![
            item_stat(75, ItemPhase::Early, 6.0, 620, 320),
            item_stat(63, ItemPhase::Early, 9.0, 980, 520),
            item_stat(166, ItemPhase::Core, 15.0, 880, 480),
            item_stat(147, ItemPhase::Core, 21.0, 840, 470),
            item_stat(158, ItemPhase::Late, 26.0, 760, 440),
            item_stat(139, ItemPhase::Late, 28.5, 690, 405),
            item_stat(160, ItemPhase::Late, 31.0, 620, 370),
            item_stat(141, ItemPhase::Late, 33.5, 580, 350),
            item_stat(156, ItemPhase::Late, 36.0, 510, 310),
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
    .expect("build should succeed");

    let early_count = build
        .steps
        .iter()
        .filter(|s| s.phase == ItemPhase::Early)
        .count();
    let core_count = build
        .steps
        .iter()
        .filter(|s| s.phase == ItemPhase::Core)
        .count();
    let late_count = build
        .steps
        .iter()
        .filter(|s| s.phase == ItemPhase::Late)
        .count();

    assert_eq!(early_count, 2, "expected 2 Early steps: {:?}", build.steps);
    assert_eq!(core_count, 2, "expected 2 Core steps: {:?}", build.steps);
    assert_eq!(late_count, 4, "expected 4 Late steps: {:?}", build.steps);
    assert_eq!(build.steps.len(), 8);
    assert!(
        build
            .steps
            .iter()
            .filter(|s| s.phase == ItemPhase::Late)
            .any(|s| s.variants.iter().any(|v| v.item_id == ItemId(156))),
        "5th Late candidate should attach as variant when 4 Late primaries are filled: {:?}",
        build.steps
    );
}

#[test]
fn recommend_item_build_distinguishes_valuable_stepping_stones_from_basic_stats() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(63, "Power Treads", 1400, &[], false, false),
            catalog_entry(21, "Ogre Axe", 1000, &[], false, false),
            catalog_entry(22, "Blade of Alacrity", 1000, &[], false, false),
            catalog_entry(67, "Oblivion Staff", 1625, &[], false, false),
            catalog_entry(69, "Perseverance", 1400, &[], false, false),
            catalog_entry(170, "Yasha", 2000, &[], false, false),
            catalog_entry(24, "Ultimate Orb", 2800, &[], false, false),
            catalog_entry(147, "Manta Style", 4550, &[170, 20], false, false),
            catalog_entry(160, "Eye of Skadi", 5300, &[24, 60], false, false),
        ],
        stats: vec![
            item_stat(63, ItemPhase::Early, 9.0, 900, 470),
            item_stat(21, ItemPhase::Early, 10.0, 650, 350),
            item_stat(22, ItemPhase::Core, 15.0, 640, 345),
            item_stat(67, ItemPhase::Core, 14.0, 620, 330),
            item_stat(69, ItemPhase::Early, 11.0, 610, 325),
            // Upgradable composite (Yasha 2000g) & high-value Ultimate Orb (2800g) bought >= 4m before Late parents.
            item_stat(170, ItemPhase::Core, 19.0, 720, 405),
            item_stat(24, ItemPhase::Core, 17.0, 560, 315),
            item_stat(147, ItemPhase::Late, 25.5, 680, 400),
            item_stat(160, ItemPhase::Late, 28.0, 540, 330),
        ],
    };

    let enemies = [None; 5];
    let build = recommend_item_build(
        &repo,
        HeroId(94),
        Role::Carry,
        Bracket::Ancient,
        &enemies,
        &[],
        600,
    )
    .expect("build should succeed");

    for forbidden in [21_u16, 22, 67, 69] {
        assert!(
            build.steps.iter().all(|s| {
                s.item_id != ItemId(forbidden)
                    && s.variants.iter().all(|v| v.item_id != ItemId(forbidden))
            }),
            "basic stat/ingredient item #{forbidden} must never appear in steps or variants: {:?}",
            build.steps
        );
    }

    assert!(
        build
            .steps
            .iter()
            .any(|s| s.item_id == ItemId(170) && s.phase == ItemPhase::Core),
        "upgradable Yasha (170) @19m Core must coexist with Late Manta (147) @25.5m: {:?}",
        build.steps
    );
    assert!(
        build
            .steps
            .iter()
            .any(|s| s.item_id == ItemId(24) && s.phase == ItemPhase::Core),
        "high-value Ultimate Orb (24) @17m Core must coexist with Late Skadi (160) @28m: {:?}",
        build.steps
    );
    assert!(
        build.steps.iter().any(|s| s.item_id == ItemId(147))
            && build.steps.iter().any(|s| s.item_id == ItemId(160)),
        "Late parent upgrades must still be present: {:?}",
        build.steps
    );
}

#[test]
fn recommend_item_build_protects_late_upgrade_chains_and_blocks_low_share_phase_filler() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(75, "Wraith Band", 505, &[], false, false),
            catalog_entry(63, "Power Treads", 1400, &[], false, false),
            catalog_entry(174, "Diffusal Blade", 2500, &[], false, false),
            catalog_entry(1107, "Phylactery", 2600, &[], false, false),
            catalog_entry(147, "Manta Style", 4550, &[170], false, false),
            catalog_entry(609, "Aghanim's Shard", 1400, &[], false, false),
            catalog_entry(1097, "Disperser", 6100, &[174], false, false),
            catalog_entry(143, "Skull Basher", 2875, &[], false, false),
            catalog_entry(116, "Black King Bar", 4050, &[21], false, false),
            catalog_entry(108, "Aghanim's Scepter", 4200, &[], false, false),
            catalog_entry(139, "Butterfly", 4975, &[], false, false),
            catalog_entry(208, "Abyssal Blade", 6250, &[143], false, false),
        ],
        stats: vec![
            item_stat(75, ItemPhase::Early, 7.5, 180, 96),
            item_stat(63, ItemPhase::Early, 10.2, 635, 333),
            item_stat(174, ItemPhase::Core, 16.7, 642, 342),
            // Low-share Core filler (~5.1% of Diffusal leader 642) must not steal Core slot 2.
            item_stat(1107, ItemPhase::Core, 16.9, 33, 18),
            item_stat(147, ItemPhase::Late, 25.7, 349, 201),
            item_stat(609, ItemPhase::Late, 25.2, 258, 155),
            item_stat(1097, ItemPhase::Late, 26.7, 126, 85),
            item_stat(143, ItemPhase::Late, 25.1, 68, 44),
            item_stat(116, ItemPhase::Late, 27.0, 33, 22),
            item_stat(108, ItemPhase::Late, 26.6, 28, 19),
            item_stat(139, ItemPhase::Late, 25.3, 19, 11),
            // Protected Late upgrade of Skull Basher (143): mc=18, 72.2% WR.
            item_stat(208, ItemPhase::Late, 26.9, 18, 13),
        ],
    };

    let enemies = [None; 5];
    let build = recommend_item_build(
        &repo,
        HeroId(32),
        Role::Mid,
        Bracket::Ancient,
        &enemies,
        &[],
        300,
    )
    .expect("build should succeed");

    assert!(
        build.steps.iter().all(|s| s.item_id != ItemId(1107)),
        "low-share Core filler Phylactery (33 vs 642) must not be promoted as primary step: {:?}",
        build.steps
    );
    assert_eq!(
        build
            .steps
            .iter()
            .filter(|s| s.phase == ItemPhase::Late)
            .count(),
        4,
        "Riki should populate 4 Late primary steps: {:?}",
        build.steps
    );
    let abyssal_present = build
        .steps
        .iter()
        .any(|s| s.item_id == ItemId(208) || s.variants.iter().any(|v| v.item_id == ItemId(208)));
    assert!(
        abyssal_present,
        "Abyssal Blade (208, upgrade of Skull Basher 143) must be recommended in Late steps or variants: {:?}",
        build.steps
    );
}

#[test]
fn recommend_item_build_keeps_core_capped_at_two_on_sparse_build() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(63, "Power Treads", 1400, &[], false, false),
            catalog_entry(145, "Battle Fury", 3900, &[], false, false),
            catalog_entry(147, "Manta Style", 4550, &[], false, false),
            catalog_entry(116, "Black King Bar", 4050, &[], false, false),
        ],
        stats: vec![
            item_stat(63, ItemPhase::Early, 8.0, 1200, 620),
            item_stat(145, ItemPhase::Core, 12.0, 1100, 610),
            item_stat(147, ItemPhase::Core, 18.2, 950, 520),
            item_stat(116, ItemPhase::Core, 24.5, 820, 445),
        ],
    };

    let enemies = [None; 5];
    let build = recommend_item_build(
        &repo,
        HeroId(1),
        Role::Carry,
        Bracket::Ancient,
        &enemies,
        &[],
        600,
    )
    .expect("build should succeed");

    let core_steps: Vec<_> = build
        .steps
        .iter()
        .filter(|s| s.phase == ItemPhase::Core)
        .collect();
    assert_eq!(
        core_steps.len(),
        2,
        "Core primary steps must stay capped at 2 even when Early/Late are sparse: {:?}",
        build.steps
    );
    assert!(
        core_steps.iter().any(
            |s| s.item_id == ItemId(116) || s.variants.iter().any(|v| v.item_id == ItemId(116))
        ) || build.steps.iter().all(|s| s.item_id != ItemId(116)),
        "3rd Core candidate must never take a 3rd Core primary slot: {:?}",
        build.steps
    );
}

#[test]
fn recommend_item_build_collapses_late_boots_and_surfaces_support_late_utility_and_ghost_scepter() {
    use crate::domain::{
        Bracket, CounterTrait, ItemId, ItemPhase, is_counter_item_eligible_for_role,
    };
    use crate::math::recommend_item_build;

    assert!(
        is_counter_item_eligible_for_role(ItemId(37), Role::SoftSupport)
            && is_counter_item_eligible_for_role(ItemId(37), Role::HardSupport)
            && !is_counter_item_eligible_for_role(ItemId(37), Role::Carry),
        "Ghost Scepter (37) counter eligibility must target supports only"
    );

    let support_catalog = vec![
        catalog_entry(36, "Magic Wand", 460, &[], false, false),
        catalog_entry(180, "Arcane Boots", 1500, &[], false, false),
        catalog_entry(214, "Tranquil Boots", 900, &[], false, false),
        catalog_entry(1, "Blink Dagger", 2250, &[], false, false),
        catalog_entry(254, "Glimmer Cape", 2150, &[], false, false),
        catalog_entry(94, "Headdress", 425, &[], false, false),
        catalog_entry(79, "Mekansm", 1775, &[94], false, false),
        catalog_entry(931, "Boots of Bearing", 4225, &[214], false, false),
        catalog_entry(231, "Guardian Greaves", 4350, &[180, 79], false, false),
        catalog_entry(48, "Boots of Travel", 2500, &[], false, false),
        catalog_entry(108, "Aghanim's Scepter", 4200, &[], false, false),
        catalog_entry(271, "Aghanim's Blessing", 5800, &[108], false, false),
        catalog_entry(100, "Eul's Scepter of Divinity", 2600, &[], false, false),
        catalog_entry(610, "Wind Waker", 6800, &[100, 58], false, false),
        catalog_entry(37, "Ghost Scepter", 1500, &[], false, false),
        catalog_entry(256, "Aeon Disk", 3000, &[], false, false),
        catalog_entry(96, "Scythe of Vyse", 5200, &[], false, false),
        catalog_entry(223, "Meteor Hammer", 2850, &[], false, false),
    ];

    let repo = MockItemRepo {
        catalog: support_catalog.clone(),
        stats: vec![
            item_stat(36, ItemPhase::Early, 6.0, 22000, 11400),
            item_stat(214, ItemPhase::Early, 8.2, 28000, 14500),
            item_stat(180, ItemPhase::Early, 11.4, 9500, 4800),
            item_stat(1, ItemPhase::Core, 23.0, 26000, 14500),
            item_stat(254, ItemPhase::Core, 24.5, 22000, 11800),
            // Late candidates: 3 Late boots, cheap aura Headdress (94), same-phase subcomponent Mekansm (79),
            // Aghs + Blessing upgrade, low-share Meteor Hammer filler, Aeon Disk, Eul's + Wind Waker, Ghost Scepter, Vyse
            item_stat(108, ItemPhase::Late, 37.5, 16000, 10800),
            item_stat(271, ItemPhase::Late, 54.0, 900, 580),
            item_stat(931, ItemPhase::Late, 40.5, 3200, 2200),
            item_stat(231, ItemPhase::Late, 39.5, 2100, 1380),
            item_stat(48, ItemPhase::Late, 49.0, 1000, 670),
            item_stat(94, ItemPhase::Late, 30.0, 5700, 3600),
            item_stat(79, ItemPhase::Late, 34.0, 1200, 780),
            item_stat(256, ItemPhase::Late, 41.5, 3100, 1950),
            item_stat(100, ItemPhase::Late, 31.0, 2600, 1550),
            item_stat(610, ItemPhase::Late, 49.5, 380, 265),
            item_stat(37, ItemPhase::Late, 33.5, 2400, 1350),
            item_stat(96, ItemPhase::Late, 45.0, 950, 620),
            item_stat(223, ItemPhase::Late, 32.0, 500, 380),
        ],
    };

    let pa_enemies = [Some(HeroId(44)), None, None, None, None];
    for role in [Role::SoftSupport, Role::HardSupport] {
        let build = recommend_item_build(
            &repo,
            HeroId(5),
            role,
            Bracket::Ancient,
            &pa_enemies,
            &[],
            1800,
        )
        .expect("support build should succeed");

        let late_steps: Vec<_> = build
            .steps
            .iter()
            .filter(|s| s.phase == ItemPhase::Late)
            .collect();
        assert_eq!(
            late_steps.len(),
            4,
            "{role:?} must fill 4 Late primaries: {late_steps:?}"
        );

        let late_boot_primaries: Vec<_> = late_steps
            .iter()
            .filter(|s| matches!(s.item_id.0, 48 | 220 | 231 | 931))
            .collect();
        assert_eq!(
            late_boot_primaries.len(),
            1,
            "{role:?} must have at most 1 Late boot primary, got: {late_boot_primaries:?}"
        );
        assert!(
            late_boot_primaries[0]
                .variants
                .iter()
                .any(|v| matches!(v.item_id.0, 48 | 231)),
            "{role:?} competing Late boots must attach as variants of {}: {:?}",
            late_boot_primaries[0].item_id.0,
            late_boot_primaries[0].variants
        );

        assert!(
            build.steps.iter().all(|s| {
                s.item_id != ItemId(94) && s.variants.iter().all(|v| v.item_id != ItemId(94))
            }),
            "{role:?} cheap Headdress (94) must be excluded in Late: {:?}",
            build.steps
        );
        assert!(
            build.steps.iter().all(|s| {
                s.item_id != ItemId(79) && s.variants.iter().all(|v| v.item_id != ItemId(79))
            }),
            "{role:?} same-phase Mekansm (79) under Greaves (231) must be suppressed: {:?}",
            build.steps
        );
        assert!(
            late_steps.iter().all(|s| s.item_id != ItemId(271)),
            "{role:?} Aghanim's Blessing (271) must collapse under Aghanim's Scepter (108), not steal a primary Late slot: {late_steps:?}"
        );
        assert!(
            late_steps
                .iter()
                .any(|s| s.item_id == ItemId(108)
                    && s.variants.iter().any(|v| v.item_id == ItemId(271))),
            "{role:?} Aghanim's Blessing (271) should attach as variant of Aghs (108): {late_steps:?}"
        );
        assert!(
            late_steps.iter().all(|s| s.item_id != ItemId(223)),
            "{role:?} low-share Meteor Hammer (223) must not block high-volume support Late items: {late_steps:?}"
        );

        let ghost_step = late_steps
            .iter()
            .find(|s| s.item_id == ItemId(37))
            .expect("Ghost Scepter (37) must surface in support Late against PA");
        assert_eq!(
            ghost_step.counter_trait,
            Some(CounterTrait::HighPhysicalBurst)
        );
        assert!(
            build
                .steps
                .iter()
                .any(|s| s.item_id == ItemId(610)
                    || s.variants.iter().any(|v| v.item_id == ItemId(610))),
            "{role:?} Wind Waker (610) upgrade of Eul's (100) must be preserved: {:?}",
            build.steps
        );
    }

    // When Ghost Scepter (37) is only a Situational candidate against PA, it appears in Optional on Pos 5, never Carry.
    let sparse_support_repo = MockItemRepo {
        catalog: support_catalog,
        stats: vec![
            item_stat(214, ItemPhase::Early, 8.2, 28000, 14500),
            item_stat(254, ItemPhase::Core, 24.5, 22000, 11800),
            item_stat(108, ItemPhase::Late, 37.5, 16000, 10800),
            item_stat(37, ItemPhase::Situational, 32.0, 220, 114),
        ],
    };
    let pos5_opt = recommend_item_build(
        &sparse_support_repo,
        HeroId(5),
        Role::HardSupport,
        Bracket::Ancient,
        &pa_enemies,
        &[],
        1500,
    )
    .unwrap();
    assert!(
        pos5_opt
            .optional_items
            .iter()
            .any(|o| o.item_id == ItemId(37)
                && o.counter_trait == Some(CounterTrait::HighPhysicalBurst)),
        "Ghost Scepter (37) should promote into Optional against PA on HardSupport: {:?}",
        pos5_opt.optional_items
    );
    let carry_opt = recommend_item_build(
        &sparse_support_repo,
        HeroId(41),
        Role::Carry,
        Bracket::Ancient,
        &pa_enemies,
        &[],
        1500,
    )
    .unwrap();
    assert!(
        carry_opt
            .optional_items
            .iter()
            .all(|o| o.item_id != ItemId(37)),
        "Ghost Scepter (37) must stay excluded from Optional on Carry: {:?}",
        carry_opt.optional_items
    );
}

#[test]
fn recommend_item_build_pairs_higher_scoring_blessing_under_scepter_across_inventory_states() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(214, "Tranquil Boots", 900, &[], false, false),
            catalog_entry(
                108,
                "Aghanim's Scepter",
                4200,
                &[60, 23, 21, 22],
                false,
                false,
            ),
            catalog_entry(271, "Aghanim's Blessing", 5800, &[108], false, false),
            catalog_entry(256, "Aeon Disk", 3000, &[], false, false),
        ],
        stats: vec![
            item_stat(214, ItemPhase::Early, 8.0, 1800, 940),
            // Exact PR #86 review reproduction: Blessing (222/300 -> ~1.042) outscores Scepter (520/1000 -> ~0.933)
            item_stat(108, ItemPhase::Late, 32.0, 1000, 520),
            item_stat(271, ItemPhase::Late, 48.0, 300, 222),
            item_stat(256, ItemPhase::Late, 36.0, 900, 500),
        ],
    };

    let enemies = [None; 5];

    // 1. Before purchase (`owned_items = &[]`): higher-scoring Blessing MUST still attach under Scepter,
    // using a single Late slot and keeping purchase order (Scepter primary -> Blessing variant).
    let unowned_build = recommend_item_build(
        &repo,
        HeroId(5),
        Role::HardSupport,
        Bracket::Ancient,
        &enemies,
        &[],
        1200,
    )
    .expect("unowned build should succeed");

    let unowned_late: Vec<_> = unowned_build
        .steps
        .iter()
        .filter(|s| s.phase == ItemPhase::Late)
        .collect();
    assert!(
        unowned_late.iter().all(|s| s.item_id != ItemId(271)),
        "Aghanim's Blessing (271) must never become a separate Late primary even when outscoring Scepter: {unowned_late:?}"
    );
    let scepter_unowned = unowned_late
        .iter()
        .find(|s| s.item_id == ItemId(108))
        .expect("Aghanim's Scepter (108) must remain primary Late step");
    assert!(!scepter_unowned.owned);
    assert_eq!(
        scepter_unowned.variants.first().map(|v| v.item_id),
        Some(ItemId(271)),
        "Blessing (271) must sit immediately beneath Scepter (108) at variants[0]: {:?}",
        scepter_unowned.variants
    );
    assert_eq!(
        unowned_late.len(),
        2,
        "Scepter+Blessing pair plus Aeon Disk must occupy only 2 Late slots: {unowned_late:?}"
    );

    // 2. Live inventory reports Tranquil Boots + Aghanim's Scepter owned:
    // Scepter remains primary & OWNED, Blessing stays grouped beneath it at variants[0],
    // and `next_unowned_index` advances past owned Scepter to Aeon Disk (256).
    let owned_build = recommend_item_build(
        &repo,
        HeroId(5),
        Role::HardSupport,
        Bracket::Ancient,
        &enemies,
        &[ItemId(214), ItemId(108)],
        2100,
    )
    .expect("owned Scepter build should succeed");

    let owned_late: Vec<_> = owned_build
        .steps
        .iter()
        .filter(|s| s.phase == ItemPhase::Late)
        .collect();
    assert!(
        owned_late.iter().all(|s| s.item_id != ItemId(271)),
        "Blessing (271) must stay under Scepter (108) when Scepter is owned: {owned_late:?}"
    );
    let scepter_owned = owned_late
        .iter()
        .find(|s| s.item_id == ItemId(108))
        .expect("Owned Scepter (108) must stay primary Late step");
    assert!(scepter_owned.owned);
    assert_eq!(
        scepter_owned.variants.first().map(|v| v.item_id),
        Some(ItemId(271)),
        "Blessing (271) upgrade must remain at variants[0] when Scepter is owned: {:?}",
        scepter_owned.variants
    );
    assert_eq!(
        owned_build
            .next_unowned_index
            .map(|idx| owned_build.steps[idx].item_id),
        Some(ItemId(256)),
        "next_unowned_index must advance to next unowned primary step (Aeon Disk): {:?}",
        owned_build.steps
    );
}

#[test]
fn recommend_item_build_defers_blessing_when_tight_window_competitor_absorbs_scepter() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(214, "Tranquil Boots", 900, &[], false, false),
            catalog_entry(
                108,
                "Aghanim's Scepter",
                4200,
                &[60, 23, 21, 22],
                false,
                false,
            ),
            catalog_entry(271, "Aghanim's Blessing", 5800, &[108], false, false),
            catalog_entry(256, "Aeon Disk", 3000, &[], false, false),
        ],
        stats: vec![
            item_stat(214, ItemPhase::Early, 8.0, 1800, 940),
            // Slightly higher-scoring Aeon Disk at 32.5m clusters tight-window Scepter at 32.0m as a variant.
            item_stat(256, ItemPhase::Late, 32.5, 1000, 550),
            item_stat(108, ItemPhase::Late, 32.0, 1000, 520),
            item_stat(271, ItemPhase::Late, 48.0, 300, 222),
        ],
    };

    let enemies = [None; 5];

    // 1. No owned items: Scepter is absorbed into Aeon Disk's competing slot as a variant,
    // so Blessing must NOT become a separate Late primary step.
    let unowned_build = recommend_item_build(
        &repo,
        HeroId(5),
        Role::HardSupport,
        Bracket::Ancient,
        &enemies,
        &[],
        1200,
    )
    .expect("unowned build with tight-window competitor should succeed");

    let unowned_late: Vec<_> = unowned_build
        .steps
        .iter()
        .filter(|s| s.phase == ItemPhase::Late)
        .collect();
    assert_eq!(
        unowned_late.len(),
        1,
        "absorbed Scepter + deferred Blessing must not spawn a standalone Blessing primary: {unowned_late:?}"
    );
    assert_eq!(unowned_late[0].item_id, ItemId(256));
    assert!(
        unowned_late[0]
            .variants
            .iter()
            .any(|v| v.item_id == ItemId(108)),
        "tight-window Scepter should sit under Aeon Disk variants: {:?}",
        unowned_late[0].variants
    );
    assert!(
        unowned_build.steps.iter().all(|s| s.item_id != ItemId(271)),
        "Blessing (271) must not appear as a primary step when Scepter is absorbed: {:?}",
        unowned_build.steps
    );

    // 2. Owned Scepter promotes Scepter into its own owned Late primary and keeps Blessing grouped under it.
    let owned_build = recommend_item_build(
        &repo,
        HeroId(5),
        Role::HardSupport,
        Bracket::Ancient,
        &enemies,
        &[ItemId(214), ItemId(108)],
        2100,
    )
    .expect("owned Scepter build should succeed");

    let owned_late: Vec<_> = owned_build
        .steps
        .iter()
        .filter(|s| s.phase == ItemPhase::Late)
        .collect();
    assert!(
        owned_late.iter().all(|s| s.item_id != ItemId(271)),
        "Blessing (271) must stay under owned Scepter rather than a standalone primary: {owned_late:?}"
    );
    let owned_scepter = owned_late
        .iter()
        .find(|s| s.item_id == ItemId(108))
        .expect("owned Scepter (108) must be retained as primary");
    assert!(owned_scepter.owned);
    assert_eq!(
        owned_scepter.variants.first().map(|v| v.item_id),
        Some(ItemId(271)),
        "Blessing (271) must attach at variants[0] when owned Scepter is primary: {:?}",
        owned_scepter.variants
    );
}

#[test]
fn recommend_item_build_marks_upgraded_prerequisites_and_late_boots_satisfied_without_resurrecting_basics()
 {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::{is_item_owned_or_upgraded, recommend_item_build};

    // Dawnbreaker scenario: Echo Sabre (252, Core, components &[21, 3, 57]) -> Harpoon (939, Late, &[252, 236])
    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(50, "Phase Boots", 1500, &[29], false, false),
            catalog_entry(48, "Boots of Travel", 2500, &[29], false, false),
            catalog_entry(21, "Ogre Axe", 1000, &[], false, false),
            catalog_entry(252, "Echo Sabre", 2700, &[21, 3, 57], false, false),
            catalog_entry(116, "Black King Bar", 4050, &[], false, false),
            catalog_entry(939, "Harpoon", 4700, &[252, 236], false, false),
            catalog_entry(112, "Assault Cuirass", 5125, &[], false, false),
            catalog_entry(180, "Arcane Boots", 1400, &[29], false, false),
            catalog_entry(79, "Mekansm", 1775, &[], false, false),
            catalog_entry(231, "Guardian Greaves", 5050, &[180, 79], false, false),
            // Cyclic test entries to verify cycle-safe traversal termination
            catalog_entry(7001, "Cycle A", 1000, &[7002], false, false),
            catalog_entry(7002, "Cycle B", 1000, &[7001], false, false),
        ],
        stats: vec![
            item_stat(50, ItemPhase::Early, 7.0, 2000, 1080),
            item_stat(21, ItemPhase::Early, 10.0, 900, 460),
            item_stat(252, ItemPhase::Core, 14.0, 2200, 1210),
            item_stat(116, ItemPhase::Core, 22.0, 2000, 1100),
            item_stat(939, ItemPhase::Late, 29.0, 1400, 820),
            item_stat(112, ItemPhase::Late, 36.0, 1000, 560),
        ],
    };

    let enemies = [None; 5];
    // Hero upgraded Echo Sabre into Harpoon (so 252 is no longer literally in inventory),
    // and upgraded Phase Boots later into Boots of Travel (48).
    let owned = [ItemId(48), ItemId(116), ItemId(939)];
    let build = recommend_item_build(
        &repo,
        HeroId(135),
        Role::Offlane,
        Bracket::Ancient,
        &enemies,
        &owned,
        2000,
    )
    .expect("Dawnbreaker build with Harpoon should succeed");

    // Excluded basic subcomponent Ogre Axe (21) must NOT be resurrected when owning Harpoon.
    assert!(
        build.steps.iter().all(|s| s.item_id != ItemId(21)),
        "basic subcomponent Ogre Axe (21) must stay excluded when Harpoon is owned: {:?}",
        build.steps
    );

    let echo = build
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(252))
        .expect("Echo Sabre step should be present");
    assert!(
        echo.owned,
        "Echo Sabre (252) must be marked satisfied/owned once Harpoon (939) is owned: {echo:?}"
    );
    let harpoon = build
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(939))
        .expect("Harpoon step should be present");
    assert!(harpoon.owned);

    let phase = build
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(50))
        .expect("Phase Boots step should be present");
    assert!(
        phase.owned,
        "early Phase Boots (50) must be satisfied when Boots of Travel (48) is owned: {phase:?}"
    );

    assert_eq!(
        build.next_unowned_index.map(|i| build.steps[i].item_id),
        Some(ItemId(112)),
        "NEXT must advance past upgraded Echo Sabre / Harpoon directly to Assault Cuirass (112): {:?}",
        build.steps
    );

    // Transitive recipe satisfaction & cycle safety checks
    assert!(is_item_owned_or_upgraded(&repo, ItemId(21), &[ItemId(939)]));
    assert!(is_item_owned_or_upgraded(
        &repo,
        ItemId(180),
        &[ItemId(231)]
    ));
    assert!(!is_item_owned_or_upgraded(
        &repo,
        ItemId(79),
        &[ItemId(7001)]
    ));
}

#[test]
fn recommend_item_build_skips_sold_or_skipped_earlier_phase_items_for_next_pointer() {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::recommend_item_build;

    let repo = MockItemRepo {
        catalog: vec![
            catalog_entry(36, "Magic Wand", 450, &[], false, false),
            catalog_entry(50, "Phase Boots", 1500, &[], false, false),
            catalog_entry(252, "Echo Sabre", 2700, &[21, 3, 57], false, false),
            catalog_entry(116, "Black King Bar", 4050, &[], false, false),
            catalog_entry(939, "Harpoon", 4700, &[252, 236], false, false),
            catalog_entry(112, "Assault Cuirass", 5125, &[], false, false),
        ],
        stats: vec![
            item_stat(36, ItemPhase::Early, 4.0, 2400, 1280),
            item_stat(50, ItemPhase::Early, 7.0, 2100, 1140),
            item_stat(252, ItemPhase::Core, 14.0, 1900, 1020),
            item_stat(116, ItemPhase::Core, 22.0, 2000, 1100),
            item_stat(939, ItemPhase::Late, 31.0, 1300, 740),
            item_stat(112, ItemPhase::Late, 38.0, 1000, 560),
        ],
    };

    let enemies = [None; 5];
    // Late game where Magic Wand (Early) & Echo Sabre (Core) were sold without upgrading to Harpoon,
    // while Assault Cuirass (Late) is already owned: NEXT must advance to Harpoon (Late), NOT Echo Sabre!
    let late_sold = recommend_item_build(
        &repo,
        HeroId(135),
        Role::Offlane,
        Bracket::Ancient,
        &enemies,
        &[ItemId(50), ItemId(116), ItemId(112)],
        2400,
    )
    .expect("late-game build should succeed");

    let echo = late_sold
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(252))
        .expect("Echo Sabre step present");
    assert!(!echo.owned);
    assert_eq!(
        late_sold
            .next_unowned_index
            .map(|i| late_sold.steps[i].item_id),
        Some(ItemId(939)),
        "NEXT must skip sold/skipped Early & Core items once a Late step is owned and point to Harpoon (939): {:?}",
        late_sold.steps
    );

    // When all Late steps (Assault Cuirass + Harpoon) are owned, NEXT must be None even if Magic Wand (Early) was sold.
    let late_done = recommend_item_build(
        &repo,
        HeroId(135),
        Role::Offlane,
        Bracket::Ancient,
        &enemies,
        &[ItemId(50), ItemId(116), ItemId(112), ItemId(939)],
        2800,
    )
    .expect("completed late-game build should succeed");
    assert_eq!(
        late_done.next_unowned_index, None,
        "when all Late steps are owned, sold Early items must not trigger NEXT: {:?}",
        late_done.steps
    );
}

#[test]
fn recommend_item_build_keeps_manta_eligible_after_out_of_order_abyssal_and_supports_early_aghs_brown_boots_turbo()
 {
    use crate::domain::{Bracket, ItemId, ItemPhase};
    use crate::math::{is_item_owned_or_upgraded, recommend_item_build};

    let enemies = [None; 5];

    // 1. Out-of-order Anti-Mage: Yasha (170) + Abyssal Blade (208, Late) bought at 26:00 (1560s).
    // Skull Basher (143) must be satisfied while Manta Style (147) remains NEXT instead of Skadi (160).
    let am_repo = MockItemRepo {
        catalog: vec![
            catalog_entry(63, "Power Treads", 1400, &[29, 25, 17], false, false),
            catalog_entry(36, "Magic Wand", 450, &[], false, false),
            catalog_entry(145, "Battle Fury", 4100, &[65, 69], false, false),
            catalog_entry(170, "Yasha", 2100, &[], false, false),
            catalog_entry(147, "Manta Style", 4650, &[170, 24], false, false),
            catalog_entry(143, "Skull Basher", 2875, &[21], false, false),
            catalog_entry(160, "Eye of Skadi", 5300, &[24], false, false),
            catalog_entry(208, "Abyssal Blade", 6150, &[143, 236], false, false),
        ],
        stats: vec![
            item_stat(63, ItemPhase::Early, 6.5, 1400, 740),
            item_stat(36, ItemPhase::Early, 5.5, 950, 500),
            item_stat(145, ItemPhase::Core, 14.5, 1350, 735),
            item_stat(147, ItemPhase::Core, 21.5, 1300, 725),
            item_stat(143, ItemPhase::Core, 25.0, 680, 380),
            item_stat(160, ItemPhase::Late, 28.5, 780, 445),
            item_stat(208, ItemPhase::Late, 37.0, 620, 365),
        ],
    };
    let am_owned = [ItemId(63), ItemId(145), ItemId(170), ItemId(208)];
    let am_build = recommend_item_build(
        &am_repo,
        HeroId(1),
        Role::Carry,
        Bracket::Legend,
        &enemies,
        &am_owned,
        1560,
    )
    .expect("Anti-Mage out-of-order build should succeed");
    assert!(is_item_owned_or_upgraded(&am_repo, ItemId(143), &am_owned));
    let manta = am_build
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(147))
        .expect("Manta Style step must be present");
    assert_eq!(manta.owned_components, 1);
    assert_eq!(
        am_build
            .next_unowned_index
            .map(|i| am_build.steps[i].item_id),
        Some(ItemId(147)),
        "Manta Style (147) with Yasha in inventory must remain NEXT after early Abyssal Blade (208): {:?}",
        am_build.steps
    );

    // 2. Witch Doctor early Aghanim's Scepter (108, sole Late item) at 22:00 (1320s):
    // NEXT must stay on Glimmer Cape (254) instead of returning None.
    let wd_repo = MockItemRepo {
        catalog: vec![
            catalog_entry(214, "Tranquil Boots", 900, &[29], false, false),
            catalog_entry(36, "Magic Wand", 450, &[], false, false),
            catalog_entry(254, "Glimmer Cape", 2150, &[], false, false),
            catalog_entry(102, "Force Staff", 2200, &[], false, false),
            catalog_entry(108, "Aghanim's Scepter", 4200, &[], false, false),
        ],
        stats: vec![
            item_stat(214, ItemPhase::Early, 7.5, 1120, 590),
            item_stat(36, ItemPhase::Early, 6.5, 1050, 550),
            item_stat(254, ItemPhase::Core, 17.5, 1020, 555),
            item_stat(102, ItemPhase::Core, 22.0, 940, 510),
            item_stat(108, ItemPhase::Late, 31.0, 540, 305),
        ],
    };
    let wd_build = recommend_item_build(
        &wd_repo,
        HeroId(30),
        Role::HardSupport,
        Bracket::Legend,
        &enemies,
        &[ItemId(214), ItemId(108)],
        1320,
    )
    .expect("Witch Doctor early Aghs build should succeed");
    assert_eq!(
        wd_build
            .next_unowned_index
            .map(|i| wd_build.steps[i].item_id),
        Some(ItemId(254)),
        "sole Late item (Aghs) bought early must not hide NEXT from pending Core Glimmer Cape: {:?}",
        wd_build.steps
    );

    // 3. Brown Boots (29) alone in mid-game must not pin NEXT to Phase Boots when skipping toward Boots of Travel,
    // whereas owning a specific Phase component (Blades of Attack, id 2) keeps Phase Boots active as NEXT.
    let boots_repo = MockItemRepo {
        catalog: vec![
            catalog_entry(29, "Boots of Speed", 500, &[], false, false),
            catalog_entry(2, "Blades of Attack", 450, &[], false, false),
            catalog_entry(4, "Chainmail", 550, &[], false, false),
            catalog_entry(50, "Phase Boots", 1450, &[29, 4, 2], false, false),
            catalog_entry(1, "Blink Dagger", 2250, &[], false, false),
            catalog_entry(48, "Boots of Travel", 2500, &[29], false, false),
        ],
        stats: vec![
            item_stat(50, ItemPhase::Early, 7.0, 1800, 960),
            item_stat(1, ItemPhase::Core, 14.0, 1700, 920),
            item_stat(48, ItemPhase::Late, 25.0, 900, 510),
        ],
    };
    let brown_boots_skip = recommend_item_build(
        &boots_repo,
        HeroId(2),
        Role::Offlane,
        Bracket::Legend,
        &enemies,
        &[ItemId(29), ItemId(1)],
        1440,
    )
    .expect("brown boots skip build should succeed");
    assert_eq!(
        brown_boots_skip
            .next_unowned_index
            .map(|i| brown_boots_skip.steps[i].item_id),
        Some(ItemId(48)),
        "generic Boots of Speed (29) past Early window with Core owned must advance NEXT toward Boots of Travel: {:?}",
        brown_boots_skip.steps
    );
    let specific_comp_keep = recommend_item_build(
        &boots_repo,
        HeroId(2),
        Role::Offlane,
        Bracket::Legend,
        &enemies,
        &[ItemId(29), ItemId(2), ItemId(1)],
        1440,
    )
    .expect("specific boot component build should succeed");
    assert_eq!(
        specific_comp_keep
            .next_unowned_index
            .map(|i| specific_comp_keep.steps[i].item_id),
        Some(ItemId(50)),
        "holding specific Phase Boots component (Blades of Attack 2) must keep Phase Boots as NEXT: {:?}",
        specific_comp_keep.steps
    );

    // 4. Turbo-paced 5-slotted inventory at 15:00 (900s): owning BKB (Core) + AC (Late) + Heart (Late)
    // skips sold Magic Wand (Early) & Echo Sabre (Core) purely via inventory saturation.
    let turbo_repo = MockItemRepo {
        catalog: vec![
            catalog_entry(36, "Magic Wand", 450, &[], false, false),
            catalog_entry(252, "Echo Sabre", 2700, &[21, 3, 57], false, false),
            catalog_entry(116, "Black King Bar", 4050, &[], false, false),
            catalog_entry(112, "Assault Cuirass", 5125, &[], false, false),
            catalog_entry(114, "Heart of Tarrasque", 5100, &[], false, false),
            catalog_entry(939, "Harpoon", 4700, &[252, 236], false, false),
        ],
        stats: vec![
            item_stat(36, ItemPhase::Early, 4.0, 2000, 1080),
            item_stat(252, ItemPhase::Core, 14.0, 1800, 970),
            item_stat(116, ItemPhase::Core, 22.0, 1900, 1040),
            item_stat(112, ItemPhase::Late, 32.0, 1200, 680),
            item_stat(114, ItemPhase::Late, 35.0, 1100, 620),
            item_stat(939, ItemPhase::Late, 38.0, 1000, 560),
        ],
    };
    let turbo_build = recommend_item_build(
        &turbo_repo,
        HeroId(135),
        Role::Offlane,
        Bracket::Legend,
        &enemies,
        &[ItemId(116), ItemId(112), ItemId(114)],
        900,
    )
    .expect("Turbo-paced build should succeed");
    assert_eq!(
        turbo_build
            .next_unowned_index
            .map(|i| turbo_build.steps[i].item_id),
        Some(ItemId(939)),
        "Turbo late inventory (2+ Late items at 15:00) must skip sold Wand & Echo Sabre and point NEXT to Harpoon: {:?}",
        turbo_build.steps
    );
}
