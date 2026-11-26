use crate::gsi::{BoundedGsiListener, parse_gsi_json_events};
use crate::meta_repo::SeededMetaRepository;
use std::io::Write;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;
use truesight_core::domain::{Bracket, DraftPick, DraftState, HeroId, Role, Side};
use truesight_core::math::{ModelWeights, recommend_heroes};
use truesight_core::ports::{DraftEvent, DraftEventSource, StatsRepository};

#[test]
fn seeded_meta_repository_loads_heroes_and_matchup_deltas() {
    let repo = SeededMetaRepository::new();
    let matrix = repo.load_matrix(Bracket::Immortal).unwrap();

    // Anti-Mage (1) vs Medusa (94) should have strong positive counter delta
    let cell = matrix.cell(HeroId(1), HeroId(94));
    assert!(
        cell.counter_delta > 0.04,
        "Anti-Mage should counter Medusa: {}",
        cell.counter_delta
    );

    // Earthshaker (7) vs Phantom Lancer (12)
    let es_pl = matrix.cell(HeroId(7), HeroId(12));
    assert!(es_pl.counter_delta > 0.03);

    // Strict antisymmetry & symmetry across pairs
    let medusa_am = matrix.cell(HeroId(94), HeroId(1));
    assert!(
        (cell.counter_delta + medusa_am.counter_delta).abs() < 1e-6,
        "Counter matrix must be strictly antisymmetric: {} vs {}",
        cell.counter_delta,
        medusa_am.counter_delta
    );
    assert!(
        (cell.synergy_delta - medusa_am.synergy_delta).abs() < 1e-6,
        "Synergy matrix must be strictly symmetric: {} vs {}",
        cell.synergy_delta,
        medusa_am.synergy_delta
    );

    // Recommend Carry when enemy picked Medusa (94) and Storm Spirit (17)
    let mut draft = DraftState {
        target_role: Role::Carry,
        ..DraftState::default()
    };
    draft
        .set_enemy(
            0,
            Some(DraftPick {
                hero_id: HeroId(94),
                role: Some(Role::Carry),
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

    let recs = recommend_heroes(&draft, &matrix, &ModelWeights::default(), 10);
    // Verify role purity: Spirit Breaker (71), Leshrac (52), Mirana (9), Kunkka (23) must not appear in Pos 1 Carry
    for r in &recs {
        assert!(
            ![HeroId(71), HeroId(52), HeroId(9), HeroId(23)].contains(&r.hero_id),
            "Non-Pos1 hero {:?} leaked into Carry recommendations",
            r.hero_id
        );
    }

    // Verify exact projection identity: picking #1 recommendation into A1 yields exact Projected Win%
    let top_pick = &recs[0];
    let mut picked_draft = draft;
    picked_draft
        .set_ally(
            0,
            Some(DraftPick {
                hero_id: top_pick.hero_id,
                role: Some(Role::Carry),
            }),
        )
        .unwrap();
    let eval_after =
        truesight_core::math::evaluate_draft_5v5(&picked_draft, &matrix, &ModelWeights::default());
    assert!(
        (eval_after.ally_win_probability - top_pick.projected_win_probability).abs() < 1e-6,
        "Header win% ({}) must exactly match Projected Win% ({})",
        eval_after.ally_win_probability,
        top_pick.projected_win_probability
    );
    assert!(
        (eval_after.ally_counter_avg[0].unwrap() - top_pick.counter_delta_sum).abs() < 1e-6,
        "Row Avg Counter must exactly match vs Picked Enemies"
    );
    assert_eq!(
        matrix.heroes().count(),
        127,
        "All 127 official OpenDota heroes must be loaded"
    );
    // Verify arbitrary non-curated heroes (e.g. Slardar=28 vs Silencer=75, Enigma=33 vs Necrophos=36)
    // have populated non-zero empirical counter & synergy deltas from the 15,966-pair table.
    let slardar_vs_silencer = matrix.cell(HeroId(28), HeroId(75));
    assert!(
        slardar_vs_silencer.vs_matches > 0 && slardar_vs_silencer.counter_delta.abs() > 0.0001,
        "Slardar vs Silencer must have empirical matchup data: {:?}",
        slardar_vs_silencer
    );
    let fv_with_enigma = matrix.cell(HeroId(41), HeroId(33));
    assert!(
        fv_with_enigma.with_matches > 0 && fv_with_enigma.synergy_delta.abs() > 0.0001,
        "Faceless Void + Enigma must have empirical synergy data: {:?}",
        fv_with_enigma
    );
}

#[test]
fn gsi_parser_extracts_game_state_hero_and_custom_pick() {
    let sample = r#"{
        "map": {
            "game_state": "DOTA_GAMERULES_STATE_HERO_SELECTION",
            "clock_time": -45
        },
        "hero": {
            "name": "npc_dota_hero_antimage"
        },
        "truesight_pick": {
            "side": "enemy",
            "slot": 1,
            "hero_id": 17
        }
    }"#;

    let events = parse_gsi_json_events(sample);
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        DraftEvent::GameRulesStateChanged {
            state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
            game_time_secs: -45,
        }
    );
    assert_eq!(
        events[1],
        DraftEvent::LocalPlayerHeroDetected {
            hero_slug: "npc_dota_hero_antimage".into(),
        }
    );
    assert_eq!(
        events[2],
        DraftEvent::PickDetected {
            side: Side::Enemies,
            slot: 1,
            hero_id: HeroId(17),
            role: Some(Role::Mid),
        }
    );

    // Replay / spectator GSI payload with nested team2/team3 player0..player9 blocks
    let replay_payload = r#"{
        "player": {
            "team2": {
                "player0": { "steamid": "76561198000000001", "team_name": "radiant" },
                "player1": { "steamid": "76561198071488061", "team_name": "radiant" }
            },
            "team3": {
                "player5": { "steamid": "76561198000000002", "team_name": "dire" }
            }
        },
        "hero": {
            "team2": {
                "player0": { "name": "npc_dota_hero_spectre" },
                "player1": { "name": "npc_dota_hero_night_stalker" }
            },
            "team3": {
                "player5": { "name": "npc_dota_hero_slark" }
            }
        }
    }"#;
    let replay_events = parse_gsi_json_events(replay_payload);
    assert!(
        !replay_events
            .iter()
            .any(|e| matches!(e, DraftEvent::PlayerSteamIdDetected { .. })),
        "Replay GSI must not overwrite account_id from player0: {replay_events:?}"
    );
    assert!(
        replay_events.contains(&DraftEvent::PlayerHeroMapped {
            steam32: 111_222_333,
            faction: truesight_core::domain::Faction::Radiant,
            hero_slug: "npc_dota_hero_night_stalker".into(),
        }),
        "Replay GSI must map player1 Steam32 + faction + hero: {replay_events:?}"
    );
    assert!(
        replay_events.contains(&DraftEvent::PlayerHeroMapped {
            steam32: 39_734_274,
            faction: truesight_core::domain::Faction::Dire,
            hero_slug: "npc_dota_hero_slark".into(),
        }),
        "Replay GSI must emit Dire PlayerHeroMapped: {replay_events:?}"
    );
}

#[test]
fn bounded_gsi_listener_accepts_http_post_and_emits_event() {
    let mut listener = BoundedGsiListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr();

    let body = r#"{"map":{"game_state":"DOTA_GAMERULES_STATE_HERO_SELECTION","clock_time":-30}}"#;
    let req = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );

    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();

    thread::sleep(Duration::from_millis(60));
    let events = listener.poll_events().unwrap();
    assert_eq!(
        events,
        vec![DraftEvent::GameRulesStateChanged {
            state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
            game_time_secs: -30,
        }]
    );
}

#[test]
fn eight_individual_brackets_dynamically_update_meta_counter_and_synergy_edges() {
    assert_eq!(
        Bracket::ALL.len(),
        8,
        "All 8 individual Dota 2 medals (Herald..Immortal) must be exposed"
    );
    assert_eq!(Bracket::Herald.label(), "Herald");
    assert_eq!(Bracket::Guardian.label(), "Guardian");
    assert_eq!(Bracket::Crusader.label(), "Crusader");
    assert_eq!(Bracket::Archon.label(), "Archon");
    assert_eq!(Bracket::Legend.label(), "Legend");
    assert_eq!(Bracket::Ancient.label(), "Ancient");
    assert_eq!(Bracket::Divine.label(), "Divine");
    assert_eq!(Bracket::Immortal.label(), "Immortal");

    let repo = SeededMetaRepository::new();
    let weights = ModelWeights::default();

    // Build the exact 5v5 draft from user simulation:
    // Allies: Faceless Void (41), Invoker (74), Magnus (97), Spirit Breaker (71), Crystal Maiden (5)
    // Enemies: Medusa (94), Storm Spirit (17), Axe (2), Shadow Demon (79), Abaddon (102)
    let mut base_draft = DraftState::default();
    for (slot, (hid, role)) in [
        (HeroId(41), Role::Carry),
        (HeroId(74), Role::Mid),
        (HeroId(97), Role::Offlane),
        (HeroId(71), Role::SoftSupport),
        (HeroId(5), Role::HardSupport),
    ]
    .into_iter()
    .enumerate()
    {
        base_draft
            .set_ally(
                slot,
                Some(DraftPick {
                    hero_id: hid,
                    role: Some(role),
                }),
            )
            .unwrap();
    }
    for (slot, (hid, role)) in [
        (HeroId(94), Role::Carry),
        (HeroId(17), Role::Mid),
        (HeroId(2), Role::Offlane),
        (HeroId(79), Role::SoftSupport),
        (HeroId(102), Role::HardSupport),
    ]
    .into_iter()
    .enumerate()
    {
        base_draft
            .set_enemy(
                slot,
                Some(DraftPick {
                    hero_id: hid,
                    role: Some(role),
                }),
            )
            .unwrap();
    }

    let m_herald = repo.load_matrix(Bracket::Herald).unwrap();
    let m_legend = repo.load_matrix(Bracket::Legend).unwrap();
    let m_divine = repo.load_matrix(Bracket::Divine).unwrap();
    let m_immortal = repo.load_matrix(Bracket::Immortal).unwrap();

    let eval_herald = truesight_core::math::evaluate_draft_5v5(&base_draft, &m_herald, &weights);
    let eval_legend = truesight_core::math::evaluate_draft_5v5(&base_draft, &m_legend, &weights);
    let eval_divine = truesight_core::math::evaluate_draft_5v5(&base_draft, &m_divine, &weights);
    let eval_immortal =
        truesight_core::math::evaluate_draft_5v5(&base_draft, &m_immortal, &weights);

    // Adjacent high-MMR medals (Divine -> Immortal) must progress smoothly without >3% cliff jumps
    assert!(
        (eval_divine.ally_win_probability - eval_immortal.ally_win_probability).abs() < 0.03,
        "Divine ({}) and Immortal ({}) must not have an artificial cliff jump from tiny pro samples",
        eval_divine.ally_win_probability,
        eval_immortal.ally_win_probability
    );

    // Meta edge, Counter edge, Synergy edge, and Win Probability must all change across brackets
    assert!(
        (eval_herald.total_meta_edge - eval_immortal.total_meta_edge).abs() > 0.001,
        "Meta edge must differ between Herald and Immortal"
    );
    assert!(
        (eval_herald.total_counter_edge - eval_immortal.total_counter_edge).abs() > 0.001,
        "Counter edge must differ between Herald ({}) and Immortal ({})",
        eval_herald.total_counter_edge,
        eval_immortal.total_counter_edge
    );
    assert!(
        (eval_herald.total_synergy_edge - eval_immortal.total_synergy_edge).abs() > 0.0002,
        "Synergy edge must differ between Herald ({}) and Immortal ({})",
        eval_herald.total_synergy_edge,
        eval_immortal.total_synergy_edge
    );
    assert!(
        (eval_legend.ally_win_probability - eval_immortal.ally_win_probability).abs() > 0.001,
        "Overall win probability must differ between Legend and Immortal"
    );

    // Verify strict antisymmetry and symmetry in all brackets
    for bracket in Bracket::ALL {
        let mat = repo.load_matrix(bracket).unwrap();
        let ab = mat.cell(HeroId(41), HeroId(94));
        let ba = mat.cell(HeroId(94), HeroId(41));
        assert!(
            (ab.counter_delta + ba.counter_delta).abs() < 1e-6,
            "Antisymmetry violated in {:?}",
            bracket
        );
        assert!(
            (ab.synergy_delta - ba.synergy_delta).abs() < 1e-6,
            "Symmetry violated in {:?}",
            bracket
        );
    }
}

#[test]
fn local_cache_manager_enforces_24h_meta_ttl_1h_player_ttl_and_applies_cached_comfort() {
    use crate::sync::{LocalCacheManager, META_TTL_SECS, PLAYER_TTL_SECS};

    assert_eq!(
        META_TTL_SECS, 86_400,
        "Meta DB TTL must be 24 hours (86400s)"
    );
    assert_eq!(
        PLAYER_TTL_SECS, 3_600,
        "Player comfort TTL must be 1 hour (3600s)"
    );

    let tmp_dir = std::env::temp_dir().join(format!("truesight_cache_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let mgr = LocalCacheManager::with_dir(tmp_dir.clone(), 100_000_001);

    let now = 1_794_500_000_u64;
    assert!(mgr.is_meta_stale(now));
    assert!(mgr.is_player_stale(now));

    let sample_meta_json = r#"[
        {"id": 1, "1_pick": 1000, "1_win": 600, "2_pick": 1000, "2_win": 600, "3_pick": 1000, "3_win": 600, "4_pick": 1000, "4_win": 600, "5_pick": 1000, "5_win": 600, "6_pick": 1000, "6_win": 600, "7_pick": 1000, "7_win": 600, "pub_pick_trend": [1000,1000,1000,1000,1000,1000,1000], "pub_win_trend": [500,500,500,500,580,580,580]}
    ]"#;
    let sample_player_json = r#"[
        {"hero_id": 1, "last_played": 1794499000, "games": 50, "win": 40},
        {"hero_id": 8, "last_played": 1700000000, "games": 30, "win": 20}
    ]"#;
    mgr.write_cached_files(Some(sample_meta_json), Some(sample_player_json), now)
        .unwrap();
    assert!(
        mgr.is_meta_stale(now + 3600),
        "Meta cache without item_constants.json must remain stale"
    );
    let sample_item_constants =
        r#"{"blink": {"id": 1, "dname": "Blink Dagger", "cost": 2250, "components": null}}"#;
    mgr.publish_staged_cache_with_items(
        Some(sample_meta_json),
        None,
        Some(sample_item_constants),
        None,
        Some(sample_player_json),
        None,
        None,
        now,
    )
    .unwrap();

    assert!(
        !mgr.is_meta_stale(now + 3600),
        "Meta cache at +1h must be fresh (<24h)"
    );
    assert!(
        mgr.is_meta_stale(now + 86_401),
        "Meta cache at +24h1s must be stale (>24h)"
    );
    assert!(
        !mgr.is_player_stale(now + 1800),
        "Player cache at +30m must be fresh (<1h)"
    );
    assert!(
        mgr.is_player_stale(now + 3601),
        "Player cache at +60m1s must be stale (>1h)"
    );

    let repo = mgr.build_repository_at(now);
    let mat = repo.load_matrix(Bracket::Legend).unwrap();
    let am = mat.hero(HeroId(1)).unwrap();
    assert_eq!(am.personal_matches, 50);
    assert_eq!(am.personal_wins, 40);
    assert!(
        am.comfort_decay > 0.98,
        "Recently played Anti-Mage must have ~1.0 decay"
    );
    assert!(
        am.role_winrates[0] > 0.59,
        "Cached heroStats 60% winrate must override snapshot: {}",
        am.role_winrates[0]
    );
    assert!(
        am.trend_delta > 0.01,
        "Cached 7-day trend jump must populate trend_delta: {}",
        am.trend_delta
    );

    let jugg = mat.hero(HeroId(8)).unwrap();
    assert_eq!(jugg.personal_matches, 30);
    assert!(
        jugg.comfort_decay < 0.50,
        "Stale Juggernaut games must decay: {}",
        jugg.comfort_decay
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn default_seeded_repository_has_zero_personal_comfort_for_open_source_neutrality() {
    let repo = SeededMetaRepository::new();
    let mat = repo.load_matrix(Bracket::Legend).unwrap();
    assert_eq!(mat.heroes().count(), 127);
    for hero in mat.heroes() {
        assert_eq!(
            hero.personal_matches, 0,
            "Public snapshot hero {} must have 0 personal_matches",
            hero.display_name
        );
        assert_eq!(
            hero.personal_wins, 0,
            "Public snapshot hero {} must have 0 personal_wins",
            hero.display_name
        );
        assert_eq!(
            hero.comfort_delta(),
            0.0,
            "Public snapshot hero {} must have 0.0 comfort_delta",
            hero.display_name
        );
    }
}

#[test]
fn comfort_v3_snapshot_demotes_inactive_antimage_and_juggernaut_below_active_pool() {
    use crate::sync::parse_player_comfort_with_matches_json;

    // Synthetic Comfort v3 fixture: Tiny & Hoodwink actively played & winning;
    // Dawnbreaker & Lifestealer moderately practiced & winning;
    // Anti-Mage unplayed for ~302 days; Juggernaut 2W-5L in recent matches.
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let stale_ts = now_ts - 302 * 86_400;
    let recent_ts = now_ts - 3 * 86_400;

    let heroes_json = format!(
        r#"[
            {{"hero_id": 19, "last_played": {recent_ts}, "games": 70, "win": 44}},
            {{"hero_id": 123, "last_played": {recent_ts}, "games": 65, "win": 41}},
            {{"hero_id": 135, "last_played": {recent_ts}, "games": 20, "win": 13}},
            {{"hero_id": 54, "last_played": {recent_ts}, "games": 20, "win": 13}},
            {{"hero_id": 1, "last_played": {stale_ts}, "games": 8, "win": 6}},
            {{"hero_id": 8, "last_played": {recent_ts}, "games": 18, "win": 8}}
        ]"#
    );

    let mut match_objs = Vec::new();
    for k in 0..45_u64 {
        let st = now_ts - k * 2 * 86_400;
        let rad_win = if k < 32 { "true" } else { "false" };
        match_objs.push(format!(
            r#"{{"hero_id": 19, "start_time": {st}, "player_slot": 0, "radiant_win": {rad_win}}}"#
        ));
        match_objs.push(format!(
            r#"{{"hero_id": 123, "start_time": {st}, "player_slot": 0, "radiant_win": {rad_win}}}"#
        ));
    }
    for k in 0..16_u64 {
        let st = now_ts - k * 3 * 86_400;
        let rad_win = if k < 12 { "true" } else { "false" };
        match_objs.push(format!(
            r#"{{"hero_id": 135, "start_time": {st}, "player_slot": 0, "radiant_win": {rad_win}}}"#
        ));
        match_objs.push(format!(
            r#"{{"hero_id": 54, "start_time": {st}, "player_slot": 0, "radiant_win": {rad_win}}}"#
        ));
    }
    for k in 0..7_u64 {
        let st = now_ts - k * 4 * 86_400;
        let rad_win = if k < 2 { "true" } else { "false" };
        match_objs.push(format!(
            r#"{{"hero_id": 8, "start_time": {st}, "player_slot": 0, "radiant_win": {rad_win}}}"#
        ));
    }
    let matches_json = format!("[{}]", match_objs.join(","));

    let mut repo = SeededMetaRepository::new();
    repo.custom_personal_pool =
        parse_player_comfort_with_matches_json(&heroes_json, Some(&matches_json));
    let mat = repo.load_matrix(Bracket::Legend).unwrap();

    let tiny = mat.hero(HeroId(19)).unwrap().comfort_delta();
    let hoodwink = mat.hero(HeroId(123)).unwrap().comfort_delta();
    let dawnbreaker = mat.hero(HeroId(135)).unwrap().comfort_delta();
    let lifestealer = mat.hero(HeroId(54)).unwrap().comfort_delta();
    let antimage = mat.hero(HeroId(1)).unwrap().comfort_delta();
    let juggernaut = mat.hero(HeroId(8)).unwrap().comfort_delta();

    assert!(
        tiny >= 0.04 && hoodwink >= 0.04,
        "Active high-volume comfort heroes (Tiny={tiny}, Hoodwink={hoodwink}) must be >= +4.0%"
    );
    assert!(
        dawnbreaker > 0.015 && lifestealer > 0.015,
        "Active practiced heroes (Dawnbreaker={dawnbreaker}, Lifestealer={lifestealer}) must exceed +1.5%"
    );
    assert_eq!(
        antimage, 0.0,
        "Anti-Mage (unplayed for 10 months / 302d) must decay to 0.0 comfort: {antimage}"
    );
    assert!(
        juggernaut <= 0.0,
        "Juggernaut (2W-5L in recent matches) must not have positive comfort: {juggernaut}"
    );
}

#[test]
fn gsi_parser_extracts_radiant_and_dire_faction_and_preserves_50_50_neutrality() {
    use truesight_core::domain::Faction;
    use truesight_core::math::{ModelWeights, evaluate_draft_5v5};

    let radiant_payload = r#"{
        "player": {
            "steamid": "76561198087521469",
            "team_name": "radiant"
        },
        "map": {
            "game_state": "DOTA_GAMERULES_STATE_HERO_SELECTION",
            "clock_time": -20
        }
    }"#;
    let dire_payload = r#"{
        "player": {
            "team_name": "dire"
        }
    }"#;

    let rad_events = parse_gsi_json_events(radiant_payload);
    assert!(
        rad_events.contains(&DraftEvent::FactionDetected {
            faction: Faction::Radiant
        }),
        "GSI parser must emit FactionDetected::Radiant for player.team_name=radiant: {rad_events:?}"
    );

    let dire_events = parse_gsi_json_events(dire_payload);
    assert!(
        dire_events.contains(&DraftEvent::FactionDetected {
            faction: Faction::Dire
        }),
        "GSI parser must emit FactionDetected::Dire for player.team_name=dire: {dire_events:?}"
    );

    let repo = SeededMetaRepository::new();
    let matrix = repo.load_matrix(Bracket::Legend).unwrap();
    let weights = ModelWeights::default();

    let mut draft_rad = DraftState {
        ally_faction: Faction::Radiant,
        ..DraftState::default()
    };
    draft_rad
        .set_ally(
            0,
            Some(DraftPick {
                hero_id: HeroId(1),
                role: Some(Role::Carry),
            }),
        )
        .unwrap();
    draft_rad
        .set_enemy(
            0,
            Some(DraftPick {
                hero_id: HeroId(94),
                role: Some(Role::Carry),
            }),
        )
        .unwrap();

    let mut draft_dire = draft_rad;
    draft_dire.ally_faction = Faction::Dire;

    let eval_rad = evaluate_draft_5v5(&draft_rad, &matrix, &weights);
    let eval_dire = evaluate_draft_5v5(&draft_dire, &matrix, &weights);
    assert!(
        (eval_rad.ally_win_probability - eval_dire.ally_win_probability).abs() < 1e-6,
        "Changing ally_faction must preserve strict 50-50 faction-neutral win probability"
    );
}

#[test]
fn vision_capture_adapter_recognizes_topbar_10_slots_and_maps_radiant_dire_sides() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter,
        compute_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::Faction;

    struct MockFrameSource {
        frames: std::collections::VecDeque<Option<RgbFrame>>,
    }
    impl ScreenFrameSource for MockFrameSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.frames.pop_front().flatten())
        }
    }

    fn paint_hero_into_frame(
        frame: &mut [u8],
        frame_w: usize,
        rect: crate::vision::TopBarSlotRect,
        hero_rgb: &[u8],
        src_w: usize,
        src_h: usize,
    ) {
        for dy in 0..rect.height {
            let sy = (dy * src_h) / rect.height;
            for dx in 0..rect.width {
                let sx = (dx * src_w) / rect.width;
                let src_idx = (sy * src_w + sx) * 3;
                let dst_idx = ((rect.y + dy) * frame_w + (rect.x + dx)) * 3;
                frame[dst_idx..dst_idx + 3].copy_from_slice(&hero_rgb[src_idx..src_idx + 3]);
            }
        }
    }

    let atlas = parse_hero_atlas().expect("embedded hero atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");

    // Frame 1: 1920x1080 with Radiant slots 0 (Anti-Mage=1), 2 (Magnus=97) and Dire slot 0 (Medusa=94), rest dark
    let (w1, h1) = (1920usize, 1080usize);
    let mut rgb1 = vec![0u8; w1 * h1 * 3];
    let (rad_r1, dire_r1) = compute_topbar_slot_rects(w1, h1);
    paint_hero_into_frame(
        &mut rgb1,
        w1,
        rad_r1[0],
        &atlas.heroes[&HeroId(1)],
        atlas.hero_width,
        atlas.hero_height,
    );
    paint_hero_into_frame(
        &mut rgb1,
        w1,
        rad_r1[2],
        &atlas.heroes[&HeroId(97)],
        atlas.hero_width,
        atlas.hero_height,
    );
    paint_hero_into_frame(
        &mut rgb1,
        w1,
        dire_r1[0],
        &atlas.heroes[&HeroId(94)],
        atlas.hero_width,
        atlas.hero_height,
    );

    // Frame 2: 2560x1440 scaled frame adding Dire slot 1 (Storm Spirit=17) while local player is Dire
    let (w2, h2) = (2560usize, 1440usize);
    let mut rgb2 = vec![0u8; w2 * h2 * 3];
    let (rad_r2, dire_r2) = compute_topbar_slot_rects(w2, h2);
    paint_hero_into_frame(
        &mut rgb2,
        w2,
        rad_r2[0],
        &atlas.heroes[&HeroId(1)],
        atlas.hero_width,
        atlas.hero_height,
    );
    paint_hero_into_frame(
        &mut rgb2,
        w2,
        rad_r2[2],
        &atlas.heroes[&HeroId(97)],
        atlas.hero_width,
        atlas.hero_height,
    );
    paint_hero_into_frame(
        &mut rgb2,
        w2,
        dire_r2[0],
        &atlas.heroes[&HeroId(94)],
        atlas.hero_width,
        atlas.hero_height,
    );
    paint_hero_into_frame(
        &mut rgb2,
        w2,
        dire_r2[1],
        &atlas.heroes[&HeroId(17)],
        atlas.hero_width,
        atlas.hero_height,
    );

    let source = MockFrameSource {
        frames: std::collections::VecDeque::from([
            Some(RgbFrame {
                width: w1,
                height: h1,
                source_height: None,
                capture_strip_rect: None,
                rgb: rgb1,
            }),
            Some(RgbFrame {
                width: w2,
                height: h2,
                source_height: None,
                capture_strip_rect: None,
                rgb: rgb2,
            }),
        ]),
    };

    let mut adapter = VisionCaptureAdapter::new(source, catalog);
    adapter.set_ally_faction(Faction::Radiant);
    let evs1 = adapter.poll_events().unwrap();
    assert_eq!(
        evs1.len(),
        3,
        "Frame 1 must detect 3 picked heroes and ignore 7 dark slots"
    );
    assert!(evs1.contains(&DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 0,
        hero_id: HeroId(1),
        role: Some(Role::Carry),
    }));
    assert!(evs1.contains(&DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 2,
        hero_id: HeroId(97),
        role: Some(Role::Offlane),
    }));
    assert!(evs1.contains(&DraftEvent::PickDetected {
        side: Side::Enemies,
        slot: 0,
        hero_id: HeroId(94),
        role: Some(Role::Carry),
    }));

    // Switch ally faction to Dire: Dire slots become Side::Allies, Radiant slots become Side::Enemies
    adapter.set_ally_faction(Faction::Dire);
    let evs2 = adapter.poll_events().unwrap();
    assert!(evs2.contains(&DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 1,
        hero_id: HeroId(17),
        role: Some(Role::Mid),
    }));
    assert!(evs2.contains(&DraftEvent::PickDetected {
        side: Side::Enemies,
        slot: 0,
        hero_id: HeroId(1),
        role: Some(Role::Carry),
    }));
}

#[test]
fn steam32_resolver_supports_explicit_override_gsi_steam64_and_loginusers_vdf() {
    use crate::sync::{
        LocalCacheManager, normalize_steam32_id, parse_loginusers_vdf, resolve_steam32_account_id,
    };

    let sample_vdf = r#"
    "users"
    {
        "76561198000000001"
        {
            "AccountName" "alt_account"
            "MostRecent" "0"
        }
        "76561198061460213"
        {
            "AccountName" "primary_player"
            "MostRecent" "1"
        }
    }
    "#;

    assert_eq!(
        normalize_steam32_id(76_561_198_061_460_213),
        Some(101_194_485)
    );
    assert_eq!(normalize_steam32_id(101_194_485), Some(101_194_485));
    assert_eq!(parse_loginusers_vdf(sample_vdf), Some(101_194_485));

    // Tier 1 (Explicit override) > Tier 2 (GSI Steam64) > Tier 3 (Steam loginusers.vdf)
    assert_eq!(
        resolve_steam32_account_id(Some(222_333_444), Some(111_222_333), Some(sample_vdf)),
        Some(222_333_444)
    );
    assert_eq!(
        resolve_steam32_account_id(None, Some(76_561_198_071_488_061), Some(sample_vdf)),
        Some(111_222_333)
    );
    assert_eq!(
        resolve_steam32_account_id(None, None, Some(sample_vdf)),
        Some(101_194_485)
    );
    assert_eq!(resolve_steam32_account_id(None, None, None), None);

    // GSI payload with player.steamid emits PlayerSteamIdDetected
    let gsi_payload = r#"{
        "player": {
            "steamid": "76561198071488061",
            "team_name": "radiant"
        }
    }"#;
    let events = parse_gsi_json_events(gsi_payload);
    assert!(
        events.contains(&DraftEvent::PlayerSteamIdDetected {
            steam32: 111_222_333
        }),
        "GSI parser must emit PlayerSteamIdDetected with converted Steam32: {events:?}"
    );

    // Per-account cache isolation under cache_dir/players/{account_id}/
    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_account_isolation_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mgr_a = LocalCacheManager::with_dir(tmp_dir.clone(), 100_000_001);
    let mgr_b = LocalCacheManager::with_dir(tmp_dir.clone(), 100_000_002);
    let now = 1_794_500_000_u64;
    mgr_a
        .write_cached_files(
            None,
            Some(r#"[{"hero_id": 1, "last_played": 1794499000, "games": 20, "win": 16}]"#),
            now,
        )
        .unwrap();
    mgr_b
        .write_cached_files(
            None,
            Some(r#"[{"hero_id": 1, "last_played": 1794499000, "games": 20, "win": 4}]"#),
            now,
        )
        .unwrap();

    let mat_a = mgr_a
        .build_repository()
        .load_matrix(Bracket::Legend)
        .unwrap();
    let mat_b = mgr_b
        .build_repository()
        .load_matrix(Bracket::Legend)
        .unwrap();
    assert!(
        mat_a.hero(HeroId(1)).unwrap().comfort_delta()
            > mat_b.hero(HeroId(1)).unwrap().comfort_delta(),
        "Per-account cache directories must isolate player comfort pools"
    );
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn stratz_synergy_override_updates_matchup_matrix_while_preserving_symmetry() {
    use crate::sync::parse_stratz_with_synergy_json;

    let stratz_json = r#"[
        {"heroId1": 1, "heroId2": 97, "synergy": 4.2, "matchCount": 12500}
    ]"#;
    let parsed = parse_stratz_with_synergy_json(stratz_json);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].0, HeroId(1));
    assert_eq!(parsed[0].1, HeroId(97));

    let mut repo = SeededMetaRepository::new();
    repo.custom_pair_synergy = parsed;

    let mat = repo.load_matrix(Bracket::Immortal).unwrap();
    let ab = mat.cell(HeroId(1), HeroId(97));
    let ba = mat.cell(HeroId(97), HeroId(1));
    assert!(
        ab.synergy_delta > 0.035 && ab.with_matches == 12500,
        "STRATZ empirical synergy override must update pair synergy & with_matches: {ab:?}"
    );
    assert!(
        (ab.synergy_delta - ba.synergy_delta).abs() < 1e-6,
        "STRATZ synergy override must preserve strict synergy symmetry"
    );
    assert!(
        (ab.counter_delta + ba.counter_delta).abs() < 1e-6,
        "STRATZ synergy override must preserve strict counter antisymmetry"
    );
}

#[test]
fn vision_catalog_matches_pickphase_upper_crop_and_rejects_flat_medal_slots() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter,
        compute_ingame_topbar_slot_rects, compute_pickphase_topbar_slot_rects, parse_hero_atlas,
    };
    use std::collections::VecDeque;

    struct QueueSource(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.pop_front())
        }
    }

    let atlas = parse_hero_atlas().expect("embedded hero atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");

    let frame_w = 2560_usize;
    let frame_h = 96_usize;
    let virtual_h = 1440_usize;

    let (rad_pick, dire_pick) = compute_pickphase_topbar_slot_rects(frame_w, virtual_h);
    let (_rad_game, _dire_game) = compute_ingame_topbar_slot_rects(frame_w, virtual_h);

    let mut pick_rgb = vec![18_u8; frame_w * frame_h * 3];

    // Paint flat gray slots + small silver badge in center columns for R0 and D0
    for rect in [rad_pick[0], dire_pick[0]] {
        for py in rect.y..rect.y + rect.height {
            for px in rect.x..rect.x + rect.width {
                let idx = (py * frame_w + px) * 3;
                pick_rgb[idx] = 68;
                pick_rgb[idx + 1] = 72;
                pick_rgb[idx + 2] = 78;
            }
        }
        for py in rect.y + 4..rect.y + 14 {
            for px in rect.x + rect.width / 2 - 10..rect.x + rect.width / 2 + 10 {
                let idx = (py * frame_w + px) * 3;
                pick_rgb[idx] = 190;
                pick_rgb[idx + 1] = 195;
                pick_rgb[idx + 2] = 200;
            }
        }
    }

    let paint_upper = |buf: &mut [u8], rect: crate::vision::TopBarSlotRect, hid: u16| {
        let hero_rgb = &atlas.heroes[&HeroId(hid)];
        let src_x0 = 4_usize;
        let src_w = atlas.hero_width - 8;
        let src_y0 = 2_usize;
        let src_h = 22_usize;
        for dy in 0..rect.height {
            let sy = src_y0 + (dy * src_h) / rect.height;
            for dx in 0..rect.width {
                let sx = src_x0 + (dx * src_w) / rect.width;
                let src_idx = (sy * atlas.hero_width + sx) * 3;
                let dst_idx = ((rect.y + dy) * frame_w + (rect.x + dx)) * 3;
                buf[dst_idx..dst_idx + 3].copy_from_slice(&hero_rgb[src_idx..src_idx + 3]);
            }
        }
    };

    let expected_rad = [(1, 60_u16), (2, 27), (3, 64), (4, 128)];
    let expected_dire = [(1, 47_u16), (2, 100), (3, 38), (4, 56)];
    for &(slot, hid) in &expected_rad {
        paint_upper(&mut pick_rgb, rad_pick[slot], hid);
    }
    for &(slot, hid) in &expected_dire {
        paint_upper(&mut pick_rgb, dire_pick[slot], hid);
    }

    let source = QueueSource(VecDeque::from([RgbFrame {
        width: frame_w,
        height: frame_h,
        source_height: None,
        capture_strip_rect: None,
        rgb: pick_rgb,
    }]));
    let mut adapter = VisionCaptureAdapter::new(source, catalog);
    let events = adapter.poll_events().unwrap();
    assert_eq!(
        events.len(),
        8,
        "Expected 8 picks and 0 empty medal slots: {events:?}"
    );

    for &(slot, hid) in &expected_rad {
        assert!(
            events.iter().any(|ev| matches!(
                ev,
                DraftEvent::PickDetected {
                    side: Side::Allies,
                    slot: s,
                    hero_id,
                    ..
                } if *s == slot && *hero_id == HeroId(hid)
            )),
            "Missing Radiant pick slot {slot} hero {hid} in {events:?}"
        );
    }
    for &(slot, hid) in &expected_dire {
        assert!(
            events.iter().any(|ev| matches!(
                ev,
                DraftEvent::PickDetected {
                    side: Side::Enemies,
                    slot: s,
                    hero_id,
                    ..
                } if *s == slot && *hero_id == HeroId(hid)
            )),
            "Missing Dire pick slot {slot} hero {hid} in {events:?}"
        );
    }
}

#[test]
fn vision_guard_rejects_non_dota_wm_class_single_slot_noise_and_dead_hero_overwrites() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter,
        compute_ingame_topbar_slot_rects, is_dota2_xprop_output, parse_hero_atlas,
    };
    use std::collections::VecDeque;

    // 1. Strict WM_CLASS window validation (rejects browser/terminal titled "Dota 2" and hidden windows)
    assert!(!is_dota2_xprop_output(
        "WM_CLASS(STRING) = \"navigator\", \"Firefox\"\nWM_NAME(STRING) = \"Dota 2\"\n_NET_WM_STATE(ATOM) = _NET_WM_STATE_FOCUSED\n"
    ));
    assert!(!is_dota2_xprop_output(
        "WM_CLASS(STRING) = \"dota2\", \"dota2\"\n_NET_WM_STATE(ATOM) = _NET_WM_STATE_HIDDEN\n"
    ));
    assert!(is_dota2_xprop_output(
        "WM_CLASS(STRING) = \"dota2\", \"dota2\"\n_NET_WM_STATE(ATOM) = _NET_WM_STATE_FULLSCREEN\n"
    ));

    struct QueueSource(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.pop_front())
        }
    }

    let atlas = parse_hero_atlas().expect("embedded hero atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    let frame_w = 2560_usize;
    let frame_h = 96_usize;
    let virtual_h = 1440_usize;
    let (rad_game, _) = compute_ingame_topbar_slot_rects(frame_w, virtual_h);

    let paint_ingame =
        |buf: &mut [u8], rect: crate::vision::TopBarSlotRect, hid: u16, chroma_scale: f32| {
            let hero_rgb = &atlas.heroes[&HeroId(hid)];
            let src_x0 = 4_usize;
            let src_w = atlas.hero_width - 8;
            let src_y0 = 2_usize;
            let src_h = atlas.hero_height - 4;
            for dy in 0..rect.height {
                let sy = src_y0 + (dy * src_h) / rect.height;
                for dx in 0..rect.width {
                    let sx = src_x0 + (dx * src_w) / rect.width;
                    let src_idx = (sy * atlas.hero_width + sx) * 3;
                    let r = f32::from(hero_rgb[src_idx]);
                    let g = f32::from(hero_rgb[src_idx + 1]);
                    let b = f32::from(hero_rgb[src_idx + 2]);
                    let luma = (r + g + b) / 3.0;
                    let dst_idx = ((rect.y + dy) * frame_w + (rect.x + dx)) * 3;
                    buf[dst_idx] = (luma + (r - luma) * chroma_scale).clamp(0.0, 255.0) as u8;
                    buf[dst_idx + 1] = (luma + (g - luma) * chroma_scale).clamp(0.0, 255.0) as u8;
                    buf[dst_idx + 2] = (luma + (b - luma) * chroma_scale).clamp(0.0, 255.0) as u8;
                }
            }
        };

    // 2. Single-slot noise frame (menu banner / partial splash) must be ignored (< 2 slots quorum)
    let mut single_slot_rgb = vec![14_u8; frame_w * frame_h * 3];
    paint_ingame(&mut single_slot_rgb, rad_game[0], 67, 1.0);

    // 3. Valid multi-slot frame locks Spectre (67) in slot 0 and Snapfire (128) in slot 4
    let mut valid_rgb = vec![14_u8; frame_w * frame_h * 3];
    paint_ingame(&mut valid_rgb, rad_game[0], 67, 1.0);
    paint_ingame(&mut valid_rgb, rad_game[4], 128, 1.0);

    // 4. Dead-hero grayscale/desaturated overwrite attempt in slot 4 (Silencer=75 at low chroma)
    let mut dead_overwrite_rgb = valid_rgb.clone();
    paint_ingame(&mut dead_overwrite_rgb, rad_game[4], 75, 0.15);

    // 5 & 6. Genuine full-color swap in already-filled slot 4 (75 at full color) requires 2 consecutive frames
    let mut swap_rgb = valid_rgb.clone();
    paint_ingame(&mut swap_rgb, rad_game[4], 75, 1.0);

    let source = QueueSource(VecDeque::from([
        RgbFrame {
            width: frame_w,
            height: frame_h,
            source_height: None,
            capture_strip_rect: None,
            rgb: single_slot_rgb,
        },
        RgbFrame {
            width: frame_w,
            height: frame_h,
            source_height: None,
            capture_strip_rect: None,
            rgb: valid_rgb,
        },
        RgbFrame {
            width: frame_w,
            height: frame_h,
            source_height: None,
            capture_strip_rect: None,
            rgb: dead_overwrite_rgb,
        },
        RgbFrame {
            width: frame_w,
            height: frame_h,
            source_height: None,
            capture_strip_rect: None,
            rgb: swap_rgb.clone(),
        },
        RgbFrame {
            width: frame_w,
            height: frame_h,
            source_height: None,
            capture_strip_rect: None,
            rgb: swap_rgb,
        },
    ]));

    let mut adapter = VisionCaptureAdapter::new(source, catalog);

    let ev_single = adapter.poll_events().unwrap();
    assert!(
        ev_single.is_empty(),
        "Single matched slot (< 2 quorum) must emit 0 events, got {ev_single:?}"
    );

    let ev_valid = adapter.poll_events().unwrap();
    assert_eq!(
        ev_valid.len(),
        2,
        "Valid 2-slot frame must emit both heroes: {ev_valid:?}"
    );

    let ev_dead = adapter.poll_events().unwrap();
    assert!(
        ev_dead.is_empty(),
        "Desaturated/dead portrait must not overwrite locked Snapfire(128): {ev_dead:?}"
    );

    let ev_swap_1 = adapter.poll_events().unwrap();
    assert!(
        ev_swap_1.is_empty(),
        "First frame of full-color overwrite must require 2-frame confirmation: {ev_swap_1:?}"
    );

    let ev_swap_2 = adapter.poll_events().unwrap();
    assert_eq!(
        ev_swap_2,
        vec![DraftEvent::PickDetected {
            side: Side::Allies,
            slot: 4,
            hero_id: HeroId(75),
            role: Some(Role::HardSupport),
        }],
        "Second consecutive full-color frame must confirm slot overwrite to HeroId(75)"
    );
}

#[test]
fn gsi_replay_payload_maps_player_steamid_to_faction_and_rejects_out_of_range_picks() {
    use truesight_core::domain::Faction;

    // Replay / spectator multi-player payload (`player0..player9` + matching `hero.playerK`)
    let replay_payload = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_PRE_GAME", "clock_time": -42 },
        "player": {
            "player0": { "steamid": "76561198000000001", "team_name": "radiant" },
            "player7": { "steamid": "76561198087521469", "team_name": "team3" }
        },
        "hero": {
            "player0": { "name": "npc_dota_hero_antimage" },
            "player7": { "name": "npc_dota_hero_night_stalker" }
        }
    }"#;
    let events = parse_gsi_json_events(replay_payload);
    assert!(events.contains(&DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_PRE_GAME".into(),
        game_time_secs: -42,
    }));
    assert!(events.contains(&DraftEvent::PlayerHeroMapped {
        steam32: 127_255_741,
        faction: Faction::Dire,
        hero_slug: "npc_dota_hero_night_stalker".into(),
    }));

    // Out-of-bounds custom pick slot/hero and malformed JSON must not emit bogus picks or panic
    let bad_pick = r#"{"truesight_pick":{"side":"enemy","slot":9,"hero_id":999}}"#;
    assert!(parse_gsi_json_events(bad_pick).is_empty());
    assert!(parse_gsi_json_events("{broken json without braces").is_empty());
}

#[test]
fn sync_stratz_synergy_parser_and_steam_id_normalizer_handle_edge_cases() {
    use crate::sync::{STEAM64_OFFSET, normalize_steam32_id, parse_stratz_with_synergy_json};

    assert_eq!(normalize_steam32_id(0), None);
    assert_eq!(normalize_steam32_id(127_255_741), Some(127_255_741));
    assert_eq!(
        normalize_steam32_id(76_561_198_087_521_469),
        Some(127_255_741)
    );
    assert_eq!(
        normalize_steam32_id(STEAM64_OFFSET + u64::from(u32::MAX) + 1),
        None
    );

    let json = r#"{
        "data": {
            "with": [
                {"heroId1": 1, "heroId2": 97, "matchCount": 1200, "synergy": 42.0},
                {"heroId1": 2, "heroId2": 2, "matchCount": 500, "synergy": 0.05},
                {"heroId1": 0, "heroId2": 5, "matchCount": 500, "synergy": 0.05},
                {"heroId1": 14, "heroId2": 27, "matches": 450, "wins": 270}
            ]
        }
    }"#;
    let rows = parse_stratz_with_synergy_json(json);
    assert_eq!(
        rows.len(),
        2,
        "self-pair and zero-id entries must be skipped"
    );
    assert_eq!(rows[0].0, HeroId(1));
    assert_eq!(rows[0].1, HeroId(97));
    assert!(
        (rows[0].2 - 0.15).abs() < 1e-6,
        "synergy must clamp to +0.15"
    );
    assert_eq!(rows[1].0, HeroId(14));
    assert_eq!(rows[1].1, HeroId(27));
    assert!(rows[1].2 > 0.05 && rows[1].2 <= 0.15);
}

#[test]
fn comfort_v3_decays_against_wall_clock_now_unix_and_handles_edge_timestamps() {
    use crate::sync::parse_player_comfort_with_matches_at_json;

    let now_unix = 1_800_000_000_u64;
    let stale_ts = now_unix - 300 * 86_400;

    // 1 match played 300 days ago (tau = 75d => expected decay = exp(-4) ~= 0.0183156, not 1.0)
    let one_stale_hero =
        format!(r#"[{{"hero_id": 1, "last_played": {stale_ts}, "games": 1, "win": 1}}]"#);
    let one_stale_match = format!(
        r#"[{{"hero_id": 1, "start_time": {stale_ts}, "player_slot": 0, "radiant_win": true}}]"#
    );

    let with_matches = parse_player_comfort_with_matches_at_json(
        &one_stale_hero,
        Some(&one_stale_match),
        now_unix,
    );
    let without_matches =
        parse_player_comfort_with_matches_at_json(&one_stale_hero, None, now_unix);
    let expected_exp_neg4 = (-4.0_f32).exp();
    assert_eq!(with_matches.len(), 1);
    assert!(
        (with_matches[0].3 - expected_exp_neg4).abs() < 1e-4,
        "1 match 300d ago must decay to exp(-4)={expected_exp_neg4}, got {}",
        with_matches[0].3
    );
    assert!(
        (without_matches[0].3 - expected_exp_neg4).abs() < 1e-4,
        "last_played fallback 300d ago must decay to exp(-4)={expected_exp_neg4}, got {}",
        without_matches[0].3
    );

    // Frozen history evaluated across advancing wall-clock timestamps strictly decays to 0.0
    let t0 = 1_750_000_000_u64;
    let frozen_heroes =
        format!(r#"[{{"hero_id": 19, "last_played": {t0}, "games": 6, "win": 4}}]"#);
    let mut frozen_match_objs = Vec::new();
    for k in 0..5_u64 {
        let st = t0 - k * 2 * 86_400;
        let rad_win = if k < 3 { "true" } else { "false" };
        frozen_match_objs.push(format!(
            r#"{{"hero_id": 19, "start_time": {st}, "player_slot": 0, "radiant_win": {rad_win}}}"#
        ));
    }
    let frozen_matches = format!("[{}]", frozen_match_objs.join(","));

    let mut deltas = Vec::new();
    let mut decays = Vec::new();
    for eval_ts in [t0, t0 + 60 * 86_400, t0 + 300 * 86_400] {
        let parsed = parse_player_comfort_with_matches_at_json(
            &frozen_heroes,
            Some(&frozen_matches),
            eval_ts,
        );
        decays.push(parsed[0].3);
        let mut repo = SeededMetaRepository::new();
        repo.custom_personal_pool = parsed;
        let mat = repo.load_matrix(Bracket::Legend).unwrap();
        deltas.push(mat.hero(HeroId(19)).unwrap().comfort_delta());
    }
    assert!(
        decays[0] > decays[1] && decays[1] > decays[2],
        "decays={decays:?}"
    );
    assert!(
        deltas[0] > deltas[1] && deltas[1] > deltas[2],
        "deltas={deltas:?}"
    );
    assert_eq!(
        deltas[2], 0.0,
        "after 300d without matches comfort_delta must decay to 0.0"
    );

    // Edge cases: future timestamp clamped, missing last_played -> 0.0, games == 0 -> 0.0,
    // and high-volume old career hero with 0 recent matches does not revive g_eff >= 0.15.
    let future_ts = now_unix + 7 * 86_400;
    let edge_heroes = format!(
        r#"[
            {{"hero_id": 2, "last_played": {future_ts}, "games": 2, "win": 2}},
            {{"hero_id": 3, "last_played": 0, "games": 20, "win": 15}},
            {{"hero_id": 4, "last_played": 0, "games": 0, "win": 0}},
            {{"hero_id": 5, "last_played": {stale_ts}, "games": 80, "win": 60}}
        ]"#
    );
    let edge_matches = format!(
        r#"[{{"hero_id": 2, "start_time": {future_ts}, "player_slot": 0, "radiant_win": true}}]"#
    );
    let edge_parsed =
        parse_player_comfort_with_matches_at_json(&edge_heroes, Some(&edge_matches), now_unix);
    let by_id = |id: u16| {
        edge_parsed
            .iter()
            .find(|r| r.0 == HeroId(id))
            .copied()
            .unwrap()
    };

    assert!(
        (by_id(2).3 - 0.5).abs() < 1e-4,
        "1 future match out of 2 games clamps age_days to 0.0 => decay 0.5"
    );
    assert_eq!(
        by_id(3).3,
        0.0,
        "missing last_played with 0 recent matches must have 0.0 decay"
    );
    assert_eq!(
        by_id(4),
        (HeroId(4), 0, 0, 0.0),
        "zero games must emit neutral 0.0"
    );
    assert!(
        (by_id(5).1 as f32) * by_id(5).3 < 0.15,
        "stale 80-game hero with 0 recent matches must not revive g_eff >= 0.15 (decay={})",
        by_id(5).3
    );

    // Neutrality check when no personal matches/heroes exist
    assert!(parse_player_comfort_with_matches_at_json("[]", Some("[]"), now_unix).is_empty());
}

#[test]
fn vision_reset_clears_detector_state_drains_stale_queue_and_repopulates_stationary_draft() {
    use crate::vision::{
        BoundedVisionWorker, HeroVisionCatalog, RgbFrame, ScreenFrameSource, TopBarSlotRect,
        VisionCaptureAdapter, compute_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::Faction;

    struct StationaryFrameSource {
        frame: RgbFrame,
    }
    impl ScreenFrameSource for StationaryFrameSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(Some(self.frame.clone()))
        }
    }

    fn paint_slot(
        frame: &mut [u8],
        frame_w: usize,
        rect: TopBarSlotRect,
        hero_rgb: &[u8],
        src_w: usize,
        src_h: usize,
    ) {
        for dy in 0..rect.height {
            let sy = (dy * src_h) / rect.height;
            for dx in 0..rect.width {
                let sx = (dx * src_w) / rect.width;
                let src_idx = (sy * src_w + sx) * 3;
                let dst_idx = ((rect.y + dy) * frame_w + (rect.x + dx)) * 3;
                frame[dst_idx..dst_idx + 3].copy_from_slice(&hero_rgb[src_idx..src_idx + 3]);
            }
        }
    }

    let atlas = parse_hero_atlas().expect("embedded hero atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    let (w, h) = (1920usize, 1080usize);
    let mut rgb = vec![0u8; w * h * 3];
    let (rad_r, dire_r) = compute_topbar_slot_rects(w, h);
    let rad_heroes = [1_u16, 13, 97, 79, 5];
    let dire_heroes = [94_u16, 17, 2, 100, 30];
    for (slot, hid) in rad_heroes.iter().enumerate() {
        paint_slot(
            &mut rgb,
            w,
            rad_r[slot],
            &atlas.heroes[&HeroId(*hid)],
            atlas.hero_width,
            atlas.hero_height,
        );
    }
    for (slot, hid) in dire_heroes.iter().enumerate() {
        paint_slot(
            &mut rgb,
            w,
            dire_r[slot],
            &atlas.heroes[&HeroId(*hid)],
            atlas.hero_width,
            atlas.hero_height,
        );
    }
    let stationary = RgbFrame {
        width: w,
        height: h,
        source_height: None,
        capture_strip_rect: None,
        rgb,
    };

    let mut adapter = VisionCaptureAdapter::new(
        StationaryFrameSource {
            frame: stationary.clone(),
        },
        catalog,
    );
    adapter.set_ally_faction(Faction::Radiant);
    let first = adapter.poll_events().unwrap();
    assert_eq!(first.len(), 10);
    assert!(adapter.poll_events().unwrap().is_empty());

    adapter.reset_state();
    let after_reset = adapter.poll_events().unwrap();
    assert_eq!(
        after_reset.len(),
        10,
        "reset_state() must re-emit stationary 10-hero picks once"
    );
    assert!(
        adapter.poll_events().unwrap().is_empty(),
        "stationary frame must not trigger repeated event storms after reset"
    );

    // Worker-level reset drains queued events and wakes the loop to re-emit stationary picks
    let mut worker =
        BoundedVisionWorker::spawn_with_source(StationaryFrameSource { frame: stationary })
            .expect("worker must spawn");

    let mut initial_seen = Vec::new();
    for _ in 0..40 {
        initial_seen.extend(worker.poll_events().unwrap());
        if initial_seen.len() >= 10 {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(initial_seen.len(), 10);

    worker.reset();
    assert!(
        worker.poll_events().unwrap().is_empty(),
        "worker.reset() must clear any buffered pre-reset events"
    );

    let mut repopulated = Vec::new();
    for _ in 0..40 {
        repopulated.extend(worker.poll_events().unwrap());
        if repopulated.len() >= 10 {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        repopulated.len(),
        10,
        "worker must re-emit stationary 10-hero picks after reset()"
    );
}

#[test]
fn gsi_listener_enforces_loopback_bind_and_rejects_non_loopback_addresses() {
    use crate::gsi::validate_gsi_bind_addr;

    for bad in ["0.0.0.0:0", "[::]:0", "192.168.1.50:4001"] {
        let err = match BoundedGsiListener::bind(bad) {
            Ok(_) => panic!("expected bind failure for non-loopback address {bad}"),
            Err(e) => e,
        };
        assert!(
            err.to_string().to_ascii_lowercase().contains("loopback"),
            "error for {bad} must explain loopback policy: {err}"
        );
    }

    let listener = BoundedGsiListener::bind("127.0.0.1:0").expect("IPv4 loopback must bind");
    assert!(listener.local_addr().ip().is_loopback());

    let parsed_v6 = validate_gsi_bind_addr("[::1]:4001").expect("IPv6 loopback must validate");
    assert!(parsed_v6.ip().is_loopback());
    if let Err(err) = BoundedGsiListener::bind("[::1]:0") {
        assert!(
            !err.to_string().to_ascii_lowercase().contains("loopback"),
            "IPv6 loopback [::1] must not be blocked by loopback guard: {err}"
        );
    }
}

#[test]
fn gsi_structural_json_parser_handles_escapes_duplicates_and_truncation() {
    // 1. Escaped quotes & decoy keys inside unrelated string values must be ignored
    let decoy_payload = r#"{
        "provider": {
            "name": "fake \"game_state\": \"DOTA_GAMERULES_STATE_POST_GAME\" \"hero_id\": 99"
        },
        "hero": {
            "alias": "quoted \"npc_dota_hero_pudge\"}",
            "name": "npc_dota_hero_antimage"
        },
        "map": {
            "clock_time": 15,
            "game_state": "DOTA_GAMERULES_STATE_HERO_SELECTION"
        }
    }"#;
    let decoy_events = parse_gsi_json_events(decoy_payload);
    assert_eq!(
        decoy_events,
        vec![
            DraftEvent::GameRulesStateChanged {
                state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
                game_time_secs: 15,
            },
            DraftEvent::LocalPlayerHeroDetected {
                hero_slug: "npc_dota_hero_antimage".into(),
            },
        ]
    );

    // 2. Reordered and duplicate keys: last key wins
    let dup_payload = r#"{"truesight_pick":{"hero_id":1,"slot":4,"slot":2,"side":"enemy"}}"#;
    assert_eq!(
        parse_gsi_json_events(dup_payload),
        vec![DraftEvent::PickDetected {
            side: Side::Enemies,
            slot: 2,
            hero_id: HeroId(1),
            role: Some(Role::Offlane),
        }]
    );

    // 3. Truncated / trailing-garbage JSON must emit zero events without panicking
    assert!(
        parse_gsi_json_events(r#"{"map":{"game_state":"DOTA_GAMERULES_STATE_HERO_SELECTION""#)
            .is_empty(),
        "Truncated JSON without closing braces must be rejected"
    );
    assert!(
        parse_gsi_json_events(
            r#"{"map":{"game_state":"DOTA_GAMERULES_STATE_HERO_SELECTION"}} trailing"#
        )
        .is_empty(),
        "Trailing non-whitespace garbage after root JSON object must be rejected"
    );
}

#[test]
fn gsi_http_framing_rejects_bad_method_missing_length_truncation_and_oversized_body() {
    use std::io::Read;
    use std::net::Shutdown;

    let mut listener = BoundedGsiListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr();

    let send_raw = |raw: &[u8]| -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        stream.write_all(raw).unwrap();
        stream.flush().unwrap();
        let _ = stream.shutdown(Shutdown::Write);
        let mut resp = String::new();
        let _ = stream.read_to_string(&mut resp);
        resp
    };

    let resp_405 = send_raw(b"GET / HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}");
    assert!(
        resp_405.starts_with("HTTP/1.1 405"),
        "Expected 405 for GET, got: {resp_405}"
    );

    let body_ok = r#"{"map":{"game_state":"DOTA_GAMERULES_STATE_HERO_SELECTION","clock_time":1}}"#;
    let resp_411 =
        send_raw(format!("POST / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n{body_ok}").as_bytes());
    assert!(
        resp_411.starts_with("HTTP/1.1 411"),
        "Expected 411 without Content-Length, got: {resp_411}"
    );

    let resp_400_trunc =
        send_raw(format!("POST / HTTP/1.1\r\nContent-Length: 200\r\n\r\n{body_ok}").as_bytes());
    assert!(
        resp_400_trunc.starts_with("HTTP/1.1 400"),
        "Expected 400 for truncated body < Content-Length, got: {resp_400_trunc}"
    );

    let resp_400_json = send_raw(b"POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\n{\"map\":bad");
    assert!(
        resp_400_json.starts_with("HTTP/1.1 400"),
        "Expected 400 for malformed JSON body, got: {resp_400_json}"
    );

    let resp_413 = send_raw(b"POST / HTTP/1.1\r\nContent-Length: 70000\r\n\r\n");
    assert!(
        resp_413.starts_with("HTTP/1.1 413"),
        "Expected 413 for oversized Content-Length > 64 KiB, got: {resp_413}"
    );

    thread::sleep(Duration::from_millis(60));
    assert!(
        listener.poll_events().unwrap().is_empty(),
        "Rejected HTTP requests must never enqueue events"
    );
}

#[test]
fn event_queues_coalesce_under_saturation_preserve_resets_and_recover_stationary_vision() {
    use crate::gsi::{BoundedDraftQueue, GSI_QUEUE_CAPACITY};
    use crate::vision::{
        BoundedVisionWorker, RgbFrame, ScreenFrameSource, compute_topbar_slot_rects,
        parse_hero_atlas,
    };
    use truesight_core::domain::Faction;

    // 1. GSI queue saturation: coalesces redundant updates and never drops critical reset/faction/steamid
    let q = BoundedDraftQueue::new(GSI_QUEUE_CAPACITY);
    assert!(q.push_for_epoch(
        0,
        DraftEvent::GameRulesStateChanged {
            state_name: "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD".into(),
            game_time_secs: -90,
        }
    ));
    assert!(q.push_for_epoch(
        0,
        DraftEvent::FactionDetected {
            faction: Faction::Radiant
        }
    ));
    assert!(q.push_for_epoch(
        0,
        DraftEvent::FactionDetected {
            faction: Faction::Dire
        }
    ));
    assert!(q.push_for_epoch(
        0,
        DraftEvent::PlayerSteamIdDetected {
            steam32: 127_255_741
        }
    ));
    for round in 0..5_u16 {
        for slot in 0..5_usize {
            let _ = q.push_for_epoch(
                0,
                DraftEvent::PickDetected {
                    side: Side::Allies,
                    slot,
                    hero_id: HeroId(10 + round * 10 + slot as u16),
                    role: Role::ALL.get(slot).copied(),
                },
            );
        }
        let _ = q.push_for_epoch(
            0,
            DraftEvent::GameRulesStateChanged {
                state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
                game_time_secs: i32::from(round) * 60,
            },
        );
    }
    for k in 0..12_u64 {
        let _ = q.push_for_epoch(
            0,
            DraftEvent::PlayerHeroMapped {
                steam32: 5000 + k,
                faction: Faction::Radiant,
                hero_slug: "npc_dota_hero_axe".into(),
            },
        );
    }

    let drained = q.drain();
    let q_stats = q.stats();
    assert!(
        q_stats.enqueued >= 45 && q_stats.coalesced > 0 && q_stats.dropped > 0,
        "Expected coalescing and dropped stats: {q_stats:?}"
    );
    assert!(drained.iter().any(|e| matches!(
        e, DraftEvent::GameRulesStateChanged { state_name, .. } if state_name == "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD"
    )));
    assert!(drained.contains(&DraftEvent::FactionDetected {
        faction: Faction::Dire
    }));
    assert!(!drained.contains(&DraftEvent::FactionDetected {
        faction: Faction::Radiant
    }));
    assert!(drained.contains(&DraftEvent::PlayerSteamIdDetected {
        steam32: 127_255_741
    }));
    let (epoch, should_resync) = q.begin_poll_cycle();
    assert!(should_resync);
    assert_eq!(
        q.stats().recovered_resyncs,
        0,
        "recovered_resyncs must not increment until recovery events actually arrive"
    );
    assert!(q.push_for_epoch(
        epoch,
        DraftEvent::PickDetected {
            side: Side::Allies,
            slot: 0,
            hero_id: HeroId(1),
            role: Some(Role::Carry),
        }
    ));
    assert_eq!(q.stats().recovered_resyncs, 1);

    // Listener diagnostics check
    let mut listener = BoundedGsiListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr();
    let body = r#"{"truesight_pick":{"side":"ally","slot":0,"hero_id":1}}"#;
    let mut s = TcpStream::connect(addr).unwrap();
    s.write_all(
        format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(50));
    let _ = listener.poll_events().unwrap();
    assert!(listener.queue_stats().enqueued >= 1);

    // 2. Vision saturation triggers overflow coalescing then automatic stationary resync recovery
    struct StationarySource(RgbFrame);
    impl ScreenFrameSource for StationarySource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(Some(self.0.clone()))
        }
    }
    let atlas = parse_hero_atlas().unwrap();
    let (w, h) = (1920usize, 1080usize);
    let mut rgb = vec![0u8; w * h * 3];
    let (rad_r, dire_r) = compute_topbar_slot_rects(w, h);
    for (rects, heroes) in [
        (&rad_r, &[1_u16, 13, 97, 79, 5]),
        (&dire_r, &[94_u16, 17, 2, 100, 30]),
    ] {
        for (slot, hid) in heroes.iter().enumerate() {
            let (rect, hero_rgb) = (rects[slot], &atlas.heroes[&HeroId(*hid)]);
            for dy in 0..rect.height {
                let sy = (dy * atlas.hero_height) / rect.height;
                for dx in 0..rect.width {
                    let src_idx =
                        (sy * atlas.hero_width + (dx * atlas.hero_width) / rect.width) * 3;
                    let dst_idx = ((rect.y + dy) * w + (rect.x + dx)) * 3;
                    rgb[dst_idx..dst_idx + 3].copy_from_slice(&hero_rgb[src_idx..src_idx + 3]);
                }
            }
        }
    }
    let mut worker = BoundedVisionWorker::spawn_with_capacity(
        StationarySource(RgbFrame {
            width: w,
            height: h,
            source_height: None,
            capture_strip_rect: None,
            rgb,
        }),
        8,
    )
    .unwrap();
    let mut first_batch = Vec::new();
    for _ in 0..40 {
        first_batch.extend(worker.poll_events().unwrap());
        if first_batch.len() >= 8 {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(first_batch.len(), 8);
    assert!(worker.queue_stats().dropped >= 2);

    let mut resynced = Vec::new();
    for _ in 0..40 {
        resynced.extend(worker.poll_events().unwrap());
        if worker.queue_stats().recovered_resyncs >= 1 && !resynced.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        worker.queue_stats().recovered_resyncs >= 1,
        "Expected automatic overflow resync: {:?}",
        worker.queue_stats()
    );
    assert!(
        !resynced.is_empty(),
        "Expected recovered vision picks after saturation"
    );
}

#[test]
fn gsi_parser_extracts_draft_bans_and_coalesces_duplicate_ban_events() {
    let payload = r#"{
        "draft": {
            "team2": {"ban0_id": 1, "ban1_id": 2, "ban2_id": 0},
            "team3": {"ban0_id": 94, "ban1_id": 160, "ban2_id": -5}
        },
        "truesight_ban": {"hero_id": 13}
    }"#;
    let events = crate::gsi::parse_gsi_json_events(payload);
    assert_eq!(
        events,
        vec![
            DraftEvent::BanDetected { hero_id: HeroId(1) },
            DraftEvent::BanDetected { hero_id: HeroId(2) },
            DraftEvent::BanDetected {
                hero_id: HeroId(94)
            },
            DraftEvent::BanDetected {
                hero_id: HeroId(13)
            },
        ]
    );

    let queue = crate::gsi::BoundedDraftQueue::new(4);
    assert!(queue.push_for_epoch(
        0,
        DraftEvent::BanDetected {
            hero_id: HeroId(94)
        }
    ));
    assert!(queue.push_for_epoch(
        0,
        DraftEvent::BanDetected {
            hero_id: HeroId(94)
        }
    ));
    assert_eq!(
        queue.drain(),
        vec![DraftEvent::BanDetected {
            hero_id: HeroId(94)
        }]
    );
}

#[test]
fn vision_rejects_empty_pickphase_slots_and_dark_gradient_bars_that_matched_dark_heroes() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter,
        compute_ingame_topbar_slot_rects, compute_pickphase_topbar_slot_rects,
        compute_topbar_slot_rects, parse_hero_atlas,
    };
    use std::collections::VecDeque;

    struct QueueSource(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.pop_front())
        }
    }

    let atlas = parse_hero_atlas().expect("embedded hero atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");

    // 1. Dark-slate / teal UI gradient frame (resembles match-start Hero Selection / client chrome
    // that previously false-matched Shadow Fiend(11), Terrorblade(109), Largo(155), Abaddon(102))
    let (w1, h1) = (1429_usize, 389_usize);
    let vh1 = (w1 * 9) / 16;
    let mut dark_gradient = vec![0_u8; w1 * h1 * 3];
    for y in 0..h1 {
        let t = (y as f32) / (h1 as f32);
        for x in 0..w1 {
            let xt = ((x % 165) as f32) / 165.0;
            let idx = (y * w1 + x) * 3;
            dark_gradient[idx] = (14.0 + xt * 8.0 + t * 6.0) as u8;
            dark_gradient[idx + 1] = (24.0 + xt * 10.0 + t * 8.0) as u8;
            dark_gradient[idx + 2] = (34.0 + xt * 12.0 + t * 10.0) as u8;
        }
    }

    // 2. Empty PickPhase slots painted at 1280x720 with realistic dark hero-card slate/cyan-tinted
    // borders (mimicking unpicked PickPhase cards where outer_sat ~ 16-24 and outer_var ~ 8-12)
    let (w2, h2) = (1280_usize, 720_usize);
    let mut empty_pick = vec![15_u8; w2 * h2 * 3];
    let (rad_pick, dire_pick) = compute_pickphase_topbar_slot_rects(w2, h2);
    for rect in rad_pick.iter().chain(dire_pick.iter()) {
        for dy in 0..rect.height {
            for dx in 0..rect.width {
                let border = dy < 3 || dy + 3 >= rect.height || dx < 4 || dx + 4 >= rect.width;
                let idx = ((rect.y + dy) * w2 + (rect.x + dx)) * 3;
                if border {
                    empty_pick[idx] = 32;
                    empty_pick[idx + 1] = 48;
                    empty_pick[idx + 2] = 66;
                } else {
                    empty_pick[idx] = 18;
                    empty_pick[idx + 1] = 28;
                    empty_pick[idx + 2] = 42;
                }
            }
        }
    }

    // 3. Grayscale high-contrast text/button boxes across all 10 SyntheticExact & InGame slots
    // (passes high outer_var, fails saturation gate once broken `&&` logic is fixed)
    let (w3, h3) = (1920_usize, 1080_usize);
    let mut gray_ui = vec![0_u8; w3 * h3 * 3];
    let (rad_syn, dire_syn) = compute_topbar_slot_rects(w3, h3);
    for rect in rad_syn.iter().chain(dire_syn.iter()) {
        for dy in 0..rect.height {
            let v = if (dy / 4) % 2 == 0 { 52 } else { 20 };
            for dx in 0..rect.width {
                let idx = ((rect.y + dy) * w3 + (rect.x + dx)) * 3;
                gray_ui[idx] = v;
                gray_ui[idx + 1] = v;
                gray_ui[idx + 2] = v;
            }
        }
    }

    // 4. Positive control at 1280x720 InGame with Queen of Pain (39, lowest outer_var=8..10) + Axe (2)
    let (w4, h4) = (1280_usize, 720_usize);
    let mut valid_ingame_1280 = vec![12_u8; w4 * h4 * 3];
    let (rad_game, dire_game) = compute_ingame_topbar_slot_rects(w4, h4);
    for (rect, hid) in [(rad_game[0], 39_u16), (dire_game[0], 2_u16)] {
        let hero_rgb = &atlas.heroes[&HeroId(hid)];
        let (sx0, sw0, sy0, sh0) = (
            4_usize,
            atlas.hero_width - 8,
            2_usize,
            atlas.hero_height - 4,
        );
        for dy in 0..rect.height {
            let sy = sy0 + (dy * sh0) / rect.height;
            for dx in 0..rect.width {
                let sx = sx0 + (dx * sw0) / rect.width;
                let si = (sy * atlas.hero_width + sx) * 3;
                let di = ((rect.y + dy) * w4 + (rect.x + dx)) * 3;
                valid_ingame_1280[di..di + 3].copy_from_slice(&hero_rgb[si..si + 3]);
            }
        }
    }
    let _ = vh1;

    let source = QueueSource(VecDeque::from([
        RgbFrame {
            width: w1,
            height: h1,
            source_height: None,
            capture_strip_rect: None,
            rgb: dark_gradient,
        },
        RgbFrame {
            width: w2,
            height: h2,
            source_height: None,
            capture_strip_rect: None,
            rgb: empty_pick,
        },
        RgbFrame {
            width: w3,
            height: h3,
            source_height: None,
            capture_strip_rect: None,
            rgb: gray_ui,
        },
        RgbFrame {
            width: w4,
            height: h4,
            source_height: None,
            capture_strip_rect: None,
            rgb: valid_ingame_1280,
        },
    ]));
    let mut adapter = VisionCaptureAdapter::new(source, catalog);

    let ev1 = adapter.poll_events().unwrap();
    assert!(
        ev1.is_empty(),
        "Dark gradient top-bar must emit 0 picks, got {ev1:?}"
    );

    let ev2 = adapter.poll_events().unwrap();
    assert!(
        ev2.is_empty(),
        "Empty PickPhase slate slots must emit 0 picks, got {ev2:?}"
    );

    let ev3 = adapter.poll_events().unwrap();
    assert!(
        ev3.is_empty(),
        "Grayscale high-contrast UI boxes must emit 0 picks, got {ev3:?}"
    );

    let ev4 = adapter.poll_events().unwrap();
    assert_eq!(
        ev4.len(),
        2,
        "Valid InGame 1280x720 QoP + Axe frame must still detect both: {ev4:?}"
    );
}

#[test]
fn vision_x11_import_args_include_silent_flag_to_prevent_xbell() {
    use crate::vision::build_x11_import_args;

    let args = build_x11_import_args("0x620013d");
    assert!(
        args.contains(&"-silent"),
        "ImageMagick import args must pass -silent to suppress XBell: {args:?}"
    );
    assert_eq!(
        args,
        [
            "-silent",
            "-window",
            "0x620013d",
            "-depth",
            "8",
            "-crop",
            "100%x8%+0+0",
            "ppm:-"
        ]
    );
}

#[test]
fn alt_portrait_atlas_loads_verified_variants_and_rejects_malformed_headers() {
    use crate::hero_alt_atlas::{parse_hero_alt_atlas, parse_hero_alt_atlas_from_slice};

    let alt_atlas = parse_hero_alt_atlas().expect("embedded alt portrait atlas must load");
    assert_eq!(alt_atlas.width, 64);
    assert_eq!(alt_atlas.height, 36);
    assert_eq!(alt_atlas.entries.len(), 49);

    let count_for = |id: u16| -> usize {
        alt_atlas
            .entries
            .iter()
            .filter(|(hero_id, _)| *hero_id == HeroId(id))
            .count()
    };
    assert_eq!(count_for(14), 3, "Pudge must include alt1, alt2, persona1");
    assert_eq!(
        count_for(44),
        2,
        "Phantom Assassin must include alt1 and persona1"
    );
    assert_eq!(
        count_for(5),
        2,
        "Crystal Maiden must include alt1 and persona1"
    );
    assert_eq!(count_for(91), 1, "Io/Wisp must include Companion Cube alt");

    assert!(parse_hero_alt_atlas_from_slice(b"NOPE").is_none());
    let mut bad_magic = crate::hero_alt_atlas::HERO_ALT_ATLAS_BYTES[..10].to_vec();
    bad_magic[0..4].copy_from_slice(b"BAD!");
    assert!(parse_hero_alt_atlas_from_slice(&bad_magic).is_none());

    let truncated = &crate::hero_alt_atlas::HERO_ALT_ATLAS_BYTES
        [..crate::hero_alt_atlas::HERO_ALT_ATLAS_BYTES.len() - 1];
    assert!(parse_hero_alt_atlas_from_slice(truncated).is_none());
}

#[test]
fn vision_catalog_matches_all_arcana_and_persona_portraits_across_layouts() {
    use crate::hero_alt_atlas::parse_hero_alt_atlas;
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, TopBarSlotRect, VisionCaptureAdapter,
        compute_ingame_topbar_slot_rects, compute_pickphase_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::Faction;

    struct SingleFrameSource(Option<RgbFrame>);
    impl ScreenFrameSource for SingleFrameSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.take())
        }
    }

    fn paint_rgb(
        frame: &mut [u8],
        frame_w: usize,
        rect: TopBarSlotRect,
        src_rgb: &[u8],
        src_w: usize,
        src_h: usize,
    ) {
        for dy in 0..rect.height {
            let sy = (dy * src_h) / rect.height;
            for dx in 0..rect.width {
                let sx = (dx * src_w) / rect.width;
                let s = (sy * src_w + sx) * 3;
                let d = ((rect.y + dy) * frame_w + (rect.x + dx)) * 3;
                frame[d..d + 3].copy_from_slice(&src_rgb[s..s + 3]);
            }
        }
    }

    let base_atlas = parse_hero_atlas().expect("embedded base atlas must load");
    let alt_atlas = parse_hero_alt_atlas().expect("embedded alt atlas must load");
    assert_eq!(
        base_atlas.heroes.len(),
        127,
        "UI base hero atlas must stay strictly at the 127 canonical base portraits"
    );

    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");

    // 1. Every single extracted Arcana/Persona variant must resolve to its base HeroId.
    let exact_rect = TopBarSlotRect {
        x: 0,
        y: 0,
        width: alt_atlas.width,
        height: alt_atlas.height,
    };
    for (idx, (expected_id, rgb)) in alt_atlas.entries.iter().enumerate() {
        let matched =
            catalog.match_portrait_crop(rgb, alt_atlas.width, alt_atlas.height, exact_rect);
        assert_eq!(
            matched,
            Some(*expected_id),
            "Alt/Persona variant idx {idx} for {expected_id:?} was not recognized"
        );
    }

    // 2. End-to-end PickPhase & InGame 10-slot recognition for Persona & Arcana portraits,
    //    preserving dark/grayscale negative-control rejection on empty slots.
    let pudge_persona = &alt_atlas
        .entries
        .iter()
        .rfind(|(id, _)| *id == HeroId(14))
        .expect("Pudge persona1 must exist")
        .1;
    let wisp_arcana = &alt_atlas
        .entries
        .iter()
        .find(|(id, _)| *id == HeroId(91))
        .expect("Wisp alt must exist")
        .1;
    let pa_persona = &alt_atlas
        .entries
        .iter()
        .rfind(|(id, _)| *id == HeroId(44))
        .expect("PA persona1 must exist")
        .1;

    let (w, h) = (1920usize, 1080usize);
    for (is_pickphase, rects) in [
        (true, compute_pickphase_topbar_slot_rects(w, h)),
        (false, compute_ingame_topbar_slot_rects(w, h)),
    ] {
        let (rad_r, dire_r) = rects;
        let mut frame_rgb = vec![0u8; w * h * 3];
        paint_rgb(
            &mut frame_rgb,
            w,
            rad_r[0],
            pudge_persona,
            alt_atlas.width,
            alt_atlas.height,
        );
        paint_rgb(
            &mut frame_rgb,
            w,
            rad_r[1],
            wisp_arcana,
            alt_atlas.width,
            alt_atlas.height,
        );
        paint_rgb(
            &mut frame_rgb,
            w,
            dire_r[0],
            pa_persona,
            alt_atlas.width,
            alt_atlas.height,
        );
        // Leave rad_r[2..5] and dire_r[1..5] as dark empty/grayscale UI negative controls.

        let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
        let source = SingleFrameSource(Some(RgbFrame {
            width: w,
            height: h,
            source_height: None,
            capture_strip_rect: None,
            rgb: frame_rgb,
        }));
        let mut adapter = VisionCaptureAdapter::new(source, catalog);
        adapter.set_ally_faction(Faction::Radiant);
        let events = adapter.poll_events().expect("poll_events must succeed");

        assert!(
            events.contains(&DraftEvent::PickDetected {
                side: Side::Allies,
                slot: 0,
                hero_id: HeroId(14),
                role: Some(Role::Carry),
            }),
            "layout pickphase={is_pickphase}: expected Pudge Persona -> HeroId(14), got {events:?}"
        );
        assert!(
            events.contains(&DraftEvent::PickDetected {
                side: Side::Allies,
                slot: 1,
                hero_id: HeroId(91),
                role: Some(Role::Mid),
            }),
            "layout pickphase={is_pickphase}: expected Wisp Arcana -> HeroId(91), got {events:?}"
        );
        assert!(
            events.contains(&DraftEvent::PickDetected {
                side: Side::Enemies,
                slot: 0,
                hero_id: HeroId(44),
                role: Some(Role::Carry),
            }),
            "layout pickphase={is_pickphase}: expected PA Persona -> HeroId(44), got {events:?}"
        );
        assert_eq!(
            events.len(),
            3,
            "layout pickphase={is_pickphase}: negative-control empty slots must produce 0 extra picks, got {events:?}"
        );
    }
}

#[test]
fn vision_pickphase_and_ingame_adapter_detect_all_49_arcana_and_persona_variants() {
    use crate::hero_alt_atlas::parse_hero_alt_atlas;
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, TopBarSlotRect, VisionCaptureAdapter,
        VisionCaptureMode, compute_ingame_topbar_slot_rects, compute_pickphase_topbar_slot_rects,
        parse_hero_atlas,
    };
    use truesight_core::domain::Faction;

    struct SingleFrameSource(Option<RgbFrame>);
    impl ScreenFrameSource for SingleFrameSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.take())
        }
    }

    fn paint_rgb_crop(
        frame: &mut [u8],
        frame_w: usize,
        rect: TopBarSlotRect,
        src_rgb: &[u8],
        src_w: usize,
        crop: TopBarSlotRect,
    ) {
        for dy in 0..rect.height {
            let sy = crop.y + (dy * crop.height) / rect.height;
            for dx in 0..rect.width {
                let sx = crop.x + (dx * crop.width) / rect.width;
                let s = (sy * src_w + sx) * 3;
                let d = ((rect.y + dy) * frame_w + (rect.x + dx)) * 3;
                frame[d..d + 3].copy_from_slice(&src_rgb[s..s + 3]);
            }
        }
    }

    let base_atlas = parse_hero_atlas().expect("base atlas must load");
    let alt_atlas = parse_hero_alt_atlas().expect("alt atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    // Companion heroes with no entries in hero_alt_atlas.bin so slot 0 never collides via dedup.
    let magnus = &base_atlas.heroes[&HeroId(97)];
    let medusa = &base_atlas.heroes[&HeroId(94)];
    let (w, h) = (1920usize, 1080usize);
    let full_src = TopBarSlotRect {
        x: 0,
        y: 0,
        width: alt_atlas.width,
        height: alt_atlas.height,
    };
    let upper_src = TopBarSlotRect {
        x: 4,
        y: 2,
        width: alt_atlas.width - 8,
        height: 22,
    };

    let mut failures = Vec::new();
    for (idx, (expected_id, rgb)) in alt_atlas.entries.iter().enumerate() {
        let cases = [
            (
                w,
                160usize,
                true,
                full_src,
                compute_pickphase_topbar_slot_rects(w, h),
            ),
            (
                w,
                160usize,
                true,
                upper_src,
                compute_pickphase_topbar_slot_rects(w, h),
            ),
            (
                2560usize,
                160usize,
                true,
                upper_src,
                compute_pickphase_topbar_slot_rects(2560, 1440),
            ),
            (
                w,
                160usize,
                false,
                full_src,
                compute_ingame_topbar_slot_rects(w, h),
            ),
        ];
        for (fw, fh, is_pickphase, crop, (rad_r, dire_r)) in cases {
            let mut frame_rgb = vec![0u8; fw * fh * 3];
            paint_rgb_crop(&mut frame_rgb, fw, rad_r[0], rgb, alt_atlas.width, crop);
            paint_rgb_crop(
                &mut frame_rgb,
                fw,
                rad_r[1],
                magnus,
                base_atlas.hero_width,
                full_src,
            );
            paint_rgb_crop(
                &mut frame_rgb,
                fw,
                dire_r[0],
                medusa,
                base_atlas.hero_width,
                full_src,
            );

            let mut adapter = VisionCaptureAdapter::new(
                SingleFrameSource(Some(RgbFrame {
                    width: fw,
                    height: fh,
                    source_height: None,
                    capture_strip_rect: None,
                    rgb: frame_rgb,
                })),
                catalog.clone(),
            );
            adapter.set_ally_faction(Faction::Radiant);
            adapter.set_capture_mode(if is_pickphase {
                VisionCaptureMode::DraftOnly
            } else {
                VisionCaptureMode::Any
            });
            let events = adapter.poll_events().expect("poll_events must succeed");
            let slot0_pick = events.iter().find_map(|ev| match ev {
                DraftEvent::PickDetected {
                    side: Side::Allies,
                    slot: 0,
                    hero_id,
                    ..
                } => Some(*hero_id),
                _ => None,
            });
            if slot0_pick != Some(*expected_id) || events.len() != 3 {
                failures.push(format!(
                    "idx={idx} {fw}x{fh} pickphase={is_pickphase} crop={}x{} expected={expected_id:?} got_slot0={slot0_pick:?} events={events:?}",
                    crop.width, crop.height
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "Arcana/Persona adapter failures ({}):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn gsi_parser_emits_init_and_custom_game_setup_states_alongside_lifecycle_states() {
    for (state, clock) in [
        ("DOTA_GAMERULES_STATE_INIT", 0),
        ("DOTA_GAMERULES_STATE_CUSTOM_GAME_SETUP", -15),
        ("DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD", -90),
        ("DOTA_GAMERULES_STATE_HERO_SELECTION", 45),
        ("DOTA_GAMERULES_STATE_STRATEGY_TIME", 10),
        ("DOTA_GAMERULES_STATE_PRE_GAME", -30),
        ("DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", 120),
        ("DOTA_GAMERULES_STATE_POST_GAME", 2400),
        ("DOTA_GAMERULES_STATE_DISCONNECT", 2405),
    ] {
        let payload = format!(r#"{{"map":{{"game_state":"{state}","clock_time":{clock}}}}}"#);
        let events = crate::gsi::parse_gsi_json_events(&payload);
        assert_eq!(
            events,
            vec![DraftEvent::GameRulesStateChanged {
                state_name: state.into(),
                game_time_secs: clock,
            }],
            "GSI parser must emit GameRulesStateChanged for {state}"
        );
    }
}

#[test]
fn vision_recalls_dark_low_contrast_heroes_and_rejects_empty_slots_across_resolutions() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, TopBarSlotRect, VisionCaptureAdapter,
        VisionCaptureMode, compute_ingame_topbar_slot_rects, compute_pickphase_topbar_slot_rects,
        parse_hero_atlas,
    };
    use truesight_core::domain::Faction;

    struct SingleFrame(Option<RgbFrame>);
    impl ScreenFrameSource for SingleFrame {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.take())
        }
    }

    fn paint_slot_crop(
        frame: &mut [u8],
        frame_w: usize,
        rect: TopBarSlotRect,
        src_rgb: &[u8],
        src_w: usize,
        crop: TopBarSlotRect,
    ) {
        for dy in 0..rect.height {
            let sy = crop.y + (dy * crop.height) / rect.height;
            for dx in 0..rect.width {
                let sx = crop.x + (dx * crop.width) / rect.width;
                let s = (sy * src_w + sx) * 3;
                let d = ((rect.y + dy) * frame_w + (rect.x + dx)) * 3;
                frame[d..d + 3].copy_from_slice(&src_rgb[s..s + 3]);
            }
        }
    }

    let atlas = parse_hero_atlas().expect("base atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    let full_crop = TopBarSlotRect {
        x: 0,
        y: 0,
        width: atlas.hero_width,
        height: atlas.hero_height,
    };
    let upper_crop = TopBarSlotRect {
        x: 4,
        y: 2,
        width: atlas.hero_width - 8,
        height: 22,
    };
    let inner_crop = TopBarSlotRect {
        x: 4,
        y: 2,
        width: atlas.hero_width - 8,
        height: atlas.hero_height - 4,
    };

    // Dark/low-contrast trio from #25 paired with negative-control empty slate slots in R3..R4 and D1..D4.
    let targets = [
        (0usize, HeroId(11), Role::Carry),    // Shadow Fiend
        (1usize, HeroId(102), Role::Mid),     // Abaddon
        (2usize, HeroId(109), Role::Offlane), // Terrorblade
    ];

    let configs = [
        (1280usize, 720usize, true, upper_crop),
        (1280usize, 720usize, true, full_crop),
        (1920usize, 1080usize, true, upper_crop),
        (1920usize, 1080usize, true, full_crop),
        (1280usize, 720usize, false, inner_crop),
        (1280usize, 720usize, false, full_crop),
        (1920usize, 1080usize, false, inner_crop),
        (1920usize, 1080usize, false, full_crop),
        (2560usize, 1440usize, false, full_crop),
    ];

    let mut failures = Vec::new();
    for (w, h, is_pickphase, crop) in configs {
        let fh = 160usize;
        let (rad_r, dire_r) = if is_pickphase {
            compute_pickphase_topbar_slot_rects(w, h)
        } else {
            compute_ingame_topbar_slot_rects(w, h)
        };
        let mut rgb = vec![15u8; w * fh * 3];
        // Paint empty-card dark cyan-slate negative controls in R3..R4 & D1..D4.
        for rect in rad_r[3..].iter().chain(dire_r[1..].iter()) {
            for dy in 0..rect.height {
                for dx in 0..rect.width {
                    let border = dy < 3 || dy + 3 >= rect.height || dx < 4 || dx + 4 >= rect.width;
                    let idx = ((rect.y + dy) * w + (rect.x + dx)) * 3;
                    rgb[idx..idx + 3].copy_from_slice(&if border {
                        [32, 48, 66]
                    } else {
                        [18, 28, 42]
                    });
                }
            }
        }
        for (slot, hid, _) in targets {
            paint_slot_crop(
                &mut rgb,
                w,
                rad_r[slot],
                &atlas.heroes[&hid],
                atlas.hero_width,
                crop,
            );
        }
        // Anti-Mage in Dire slot 0 to validate opposite side + zero phantom picks on empty slots.
        paint_slot_crop(
            &mut rgb,
            w,
            dire_r[0],
            &atlas.heroes[&HeroId(1)],
            atlas.hero_width,
            crop,
        );

        let mut adapter = VisionCaptureAdapter::new(
            SingleFrame(Some(RgbFrame {
                width: w,
                height: fh,
                source_height: None,
                capture_strip_rect: None,
                rgb,
            })),
            catalog.clone(),
        );
        adapter.set_ally_faction(Faction::Radiant);
        adapter.set_capture_mode(if is_pickphase {
            VisionCaptureMode::DraftOnly
        } else {
            VisionCaptureMode::Any
        });
        let events = adapter.poll_events().expect("poll_events must succeed");
        for (slot, hid, role) in targets {
            let expected = DraftEvent::PickDetected {
                side: Side::Allies,
                slot,
                hero_id: hid,
                role: Some(role),
            };
            if !events.contains(&expected) {
                failures.push(format!(
                    "{w}x{h} pickphase={is_pickphase} crop={}x{}: missing {hid:?} in {events:?}",
                    crop.width, crop.height
                ));
            }
        }
        if events.len() != 4 {
            failures.push(format!("{w}x{h} pickphase={is_pickphase} crop={}x{}: expected 4 picks (0 phantom empty slots), got {}", crop.width, crop.height, events.len()));
        }
    }

    assert!(
        failures.is_empty(),
        "Dark/low-contrast recall or negative-control failures ({}):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn capture_factory_detects_platform_capabilities_and_reports_actionable_degradation() {
    use crate::vision::{
        CaptureAvailability, CaptureBackendKind, detect_capture_availability_for,
        spawn_platform_worker_for,
    };

    assert_eq!(
        detect_capture_availability_for("linux", false, true, true, true),
        CaptureAvailability::Available {
            backend: CaptureBackendKind::X11,
            detail: "X11/XWayland (xprop + import)".into(),
        }
    );

    let missing_tools = detect_capture_availability_for("linux", false, true, false, true);
    assert!(matches!(
        missing_tools,
        CaptureAvailability::Unavailable {
            backend: CaptureBackendKind::X11,
            ref reason,
        } if reason.contains("xprop") && reason.to_ascii_lowercase().contains("manual")
    ));

    let native_wayland = detect_capture_availability_for("linux", true, false, false, false);
    assert!(matches!(
        native_wayland,
        CaptureAvailability::Unavailable {
            backend: CaptureBackendKind::WaylandPortal,
            ref reason,
        } if reason.contains("Wayland") && reason.contains("#13")
    ));

    let win = detect_capture_availability_for("windows", false, false, false, false);
    assert_eq!(
        win,
        CaptureAvailability::Available {
            backend: CaptureBackendKind::WindowsNative,
            detail: "Windows Win32/GDI (xcap)".into(),
        }
    );
    assert_eq!(win.badge_label(), "CAP: WIN32");
    assert!(win.is_available());

    let other = detect_capture_availability_for("macos", false, false, false, false);
    assert!(matches!(
        other,
        CaptureAvailability::Unavailable {
            backend: CaptureBackendKind::Unsupported,
            ..
        }
    ));

    // When backend matches host OS (`windows`), worker spawns cleanly; otherwise factory
    // safely degrades to `Unavailable` (`CAP: MANUAL`) with no background thread.
    let (worker, avail) = spawn_platform_worker_for(&win);
    if cfg!(target_os = "windows") {
        assert!(worker.is_some());
        assert_eq!(avail, win);
        assert_eq!(avail.badge_label(), "CAP: WIN32");
        assert!(avail.is_available());
    } else {
        assert!(worker.is_none());
        assert_eq!(avail.badge_label(), "CAP: MANUAL");
        assert!(!avail.is_available());
    }

    let unimplemented_avail = CaptureAvailability::Available {
        backend: CaptureBackendKind::WaylandPortal,
        detail: "Wayland stub".into(),
    };
    let (w2, a2) = spawn_platform_worker_for(&unimplemented_avail);
    assert!(w2.is_none());
    assert!(!a2.is_available());
    assert_eq!(a2.badge_label(), "CAP: MANUAL");
}

#[test]
fn windows_capture_filter_and_rgba_strip_reject_browsers_minimized_and_convert_topbar() {
    use crate::vision::{
        HeroVisionCatalog, ScreenFrameSource, VisionCaptureAdapter, evaluate_pid_lookup_output,
        is_dota2_windows_window, parse_hero_atlas, parse_tasklist_exe_for_pid,
        resolve_windows_system32_exe, rgba_to_topbar_rgb_frame, verify_dota_candidate_with,
    };
    use std::path::PathBuf;
    use truesight_core::domain::{HeroId, Side};
    use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

    assert!(is_dota2_windows_window(
        "Dota 2",
        Some("dota2.exe"),
        false,
        1920,
        1080
    ));
    assert!(is_dota2_windows_window(
        "dota2",
        Some(r"C:\Steam\steamapps\common\dota 2 beta\game\bin\win64\dota2.exe"),
        false,
        2560,
        1440
    ));

    // Reject unverified PID lookups, non-dota2 executables with same title, minimized, or tiny windows.
    assert!(!is_dota2_windows_window("Dota 2", None, false, 1280, 720));
    assert!(!is_dota2_windows_window(
        "Dota 2",
        Some(""),
        false,
        1920,
        1080
    ));
    assert!(!is_dota2_windows_window(
        "Dota 2",
        Some("notepad.exe"),
        false,
        1920,
        1080
    ));
    assert!(!is_dota2_windows_window(
        "Dota 2",
        Some("Dota 2"),
        false,
        1920,
        1080
    ));
    assert!(!is_dota2_windows_window(
        "Dota 2",
        Some("dota2.exe"),
        true,
        1920,
        1080
    ));
    assert!(!is_dota2_windows_window(
        "Dota 2",
        Some("dota2.exe"),
        false,
        639,
        1080
    ));
    assert!(!is_dota2_windows_window(
        "Dota 2",
        Some("dota2.exe"),
        false,
        1920,
        359
    ));
    assert!(!is_dota2_windows_window(
        "Dota 2 - Google Chrome",
        Some("chrome.exe"),
        false,
        1920,
        1080
    ));

    assert_eq!(
        parse_tasklist_exe_for_pid(
            "\"dota2.exe\",\"4210\",\"Console\",\"1\",\"2,100,000 K\"\r\n",
            4210
        ),
        Some("dota2.exe".to_string())
    );
    assert_eq!(
        parse_tasklist_exe_for_pid(
            "\"notepad.exe\",\"4210\",\"Console\",\"1\",\"12,000 K\"\r\n",
            9999
        ),
        None
    );
    assert_eq!(
        parse_tasklist_exe_for_pid(
            "INFO: No tasks are running which match the specified criteria.\r\n",
            4210
        ),
        None
    );

    // Windows system binary resolution rejects relative overrides and uses System32.
    assert_eq!(
        resolve_windows_system32_exe(Some(r"D:\WinOS"), None, "tasklist.exe"),
        PathBuf::from(r"D:\WinOS")
            .join("System32")
            .join("tasklist.exe")
    );
    assert_eq!(
        resolve_windows_system32_exe(Some(r"relative\win"), Some(r"E:\Windows"), "tasklist.exe"),
        PathBuf::from(r"E:\Windows")
            .join("System32")
            .join("tasklist.exe")
    );
    assert_eq!(
        resolve_windows_system32_exe(Some(""), None, "tasklist.exe"),
        PathBuf::from(r"C:\Windows")
            .join("System32")
            .join("tasklist.exe")
    );

    // Lookup exit failures must return Err(PortError) instead of collapsing into Ok(None) / WAIT.
    assert_eq!(
        evaluate_pid_lookup_output(
            true,
            Some(0),
            "\"dota2.exe\",\"4210\",\"Console\",\"1\",\"2,100,000 K\"\r\n",
            4210
        )
        .unwrap(),
        Some("dota2.exe".to_string())
    );
    assert_eq!(
        evaluate_pid_lookup_output(
            true,
            Some(0),
            "INFO: No tasks are running which match the specified criteria.\r\n",
            4210
        )
        .unwrap(),
        None
    );
    assert!(evaluate_pid_lookup_output(false, Some(1), "", 4210).is_err());

    assert!(
        verify_dota_candidate_with("Dota 2", 4210, false, 1920, 1080, |_| Err(PortError(
            "spawn denied".into()
        )))
        .is_err()
    );
    assert_eq!(
        verify_dota_candidate_with("Dota 2", 4210, false, 1920, 1080, |_| Ok(None)).unwrap(),
        None
    );
    assert_eq!(
        verify_dota_candidate_with("Dota 2", 4210, false, 1920, 1080, |_| Ok(Some(
            "notepad.exe".into()
        )))
        .unwrap(),
        None
    );
    assert_eq!(
        verify_dota_candidate_with("Dota 2", 4210, false, 1920, 1080, |_| Ok(Some(
            "dota2.exe".into()
        )))
        .unwrap(),
        Some(4210)
    );

    // Full RGBA window conversion strips alpha, crops top 8% strip, and rejects invalid buffers.
    assert!(rgba_to_topbar_rgb_frame(&[0u8; 400], 639, 720).is_none());
    assert!(rgba_to_topbar_rgb_frame(&[0u8; 1280 * 359 * 4], 1280, 359).is_none());
    assert!(rgba_to_topbar_rgb_frame(&[0u8; 1280 * 720 * 4 - 4], 1280, 720).is_none());

    let atlas = parse_hero_atlas().unwrap();
    let w = 1280_usize;
    let h = 720_usize;
    let mut full_rgba = vec![14_u8; w * h * 4];
    for px in full_rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }

    let paint_rgba_slot = |rgba: &mut [u8], hero_id: u16, rect: crate::vision::TopBarSlotRect| {
        let hero_rgb = atlas.heroes.get(&HeroId(hero_id)).unwrap();
        for dy in 0..rect.height {
            let sy = (dy * atlas.hero_height) / rect.height;
            for dx in 0..rect.width {
                let sx = (dx * atlas.hero_width) / rect.width;
                let s_idx = (sy * atlas.hero_width + sx) * 3;
                let d_idx = ((rect.y + dy) * w + (rect.x + dx)) * 4;
                rgba[d_idx] = hero_rgb[s_idx];
                rgba[d_idx + 1] = hero_rgb[s_idx + 1];
                rgba[d_idx + 2] = hero_rgb[s_idx + 2];
                rgba[d_idx + 3] = 128;
            }
        }
    };
    let (rad_rects, dire_rects) = crate::vision::compute_topbar_slot_rects(w, h);
    paint_rgba_slot(&mut full_rgba, 1, rad_rects[0]);
    paint_rgba_slot(&mut full_rgba, 5, dire_rects[0]);

    let frame = rgba_to_topbar_rgb_frame(&full_rgba, w, h).expect("valid topbar strip");
    assert_eq!(frame.width, 1280);
    assert_eq!(frame.height, 60);
    assert_eq!(frame.rgb.len(), 1280 * 60 * 3);

    struct SingleRgbaFrame(Option<crate::vision::RgbFrame>);
    impl ScreenFrameSource for SingleRgbaFrame {
        fn capture_frame(
            &mut self,
        ) -> Result<Option<crate::vision::RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.take())
        }
    }

    let mut adapter = VisionCaptureAdapter::new(
        SingleRgbaFrame(Some(frame)),
        HeroVisionCatalog::from_embedded().unwrap(),
    );
    let events = adapter.poll_events().unwrap();
    assert!(events.contains(&DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 0,
        hero_id: HeroId(1),
        role: Some(truesight_core::domain::Role::Carry),
    }));
    assert!(events.contains(&DraftEvent::PickDetected {
        side: Side::Enemies,
        slot: 0,
        hero_id: HeroId(5),
        role: Some(truesight_core::domain::Role::Carry),
    }));
}

#[test]
fn windows_non_16_9_crop_preserves_source_height_and_detects_4_3_pickphase() {
    use crate::vision::{
        HeroVisionCatalog, ScreenFrameSource, VisionCaptureAdapter,
        compute_pickphase_topbar_slot_rects, parse_hero_atlas, rgba_to_topbar_rgb_frame,
    };
    use truesight_core::domain::{HeroId, Side};
    use truesight_core::ports::{DraftEvent, DraftEventSource};

    let w = 1024_usize;
    let h = 768_usize;
    let (rad_rects, dire_rects) = compute_pickphase_topbar_slot_rects(w, h);
    assert_eq!(rad_rects[0].height, 44);

    let atlas = parse_hero_atlas().unwrap();
    let mut full_rgba = vec![14_u8; w * h * 4];
    for px in full_rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }

    let paint_upper_crop = |rgba: &mut [u8], hero_id: u16, rect: crate::vision::TopBarSlotRect| {
        let hero_rgb = atlas.heroes.get(&HeroId(hero_id)).unwrap();
        let src_h = ((atlas.hero_height * 22) / 36).max(1);
        let src_w = (atlas.hero_width * 56) / 64;
        let src_x0 = (atlas.hero_width - src_w) / 2;
        let src_y0 = 2;
        for dy in 0..rect.height {
            let sy = (src_y0 + (dy * src_h) / rect.height).min(atlas.hero_height - 1);
            for dx in 0..rect.width {
                let sx = (src_x0 + (dx * src_w) / rect.width).min(atlas.hero_width - 1);
                let s_idx = (sy * atlas.hero_width + sx) * 3;
                let d_idx = ((rect.y + dy) * w + (rect.x + dx)) * 4;
                rgba[d_idx] = hero_rgb[s_idx];
                rgba[d_idx + 1] = hero_rgb[s_idx + 1];
                rgba[d_idx + 2] = hero_rgb[s_idx + 2];
            }
        }
    };
    paint_upper_crop(&mut full_rgba, 2, rad_rects[0]);
    paint_upper_crop(&mut full_rgba, 5, dire_rects[0]);

    let frame = rgba_to_topbar_rgb_frame(&full_rgba, w, h).expect("4:3 strip");
    assert_eq!(frame.height, 62);
    assert_eq!(frame.source_height, Some(768));

    struct OnceSource(Option<crate::vision::RgbFrame>);
    impl ScreenFrameSource for OnceSource {
        fn capture_frame(
            &mut self,
        ) -> Result<Option<crate::vision::RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.take())
        }
    }

    let mut adapter = VisionCaptureAdapter::new(
        OnceSource(Some(frame)),
        HeroVisionCatalog::from_embedded().unwrap(),
    );
    let events = adapter.poll_events().unwrap();
    assert!(events.contains(&DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 0,
        hero_id: HeroId(2),
        role: Some(truesight_core::domain::Role::Carry),
    }));
    assert!(events.contains(&DraftEvent::PickDetected {
        side: Side::Enemies,
        slot: 0,
        hero_id: HeroId(5),
        role: Some(truesight_core::domain::Role::Carry),
    }));
}

#[test]
fn vision_worker_tracks_runtime_health_errors_recovery_and_bounded_stalled_drop() {
    use crate::vision::{
        BoundedVisionWorker, CaptureRuntimeHealth, RgbFrame, ScreenFrameSource, VisionCaptureMode,
    };
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};
    use truesight_core::ports::PortError;

    enum Step {
        Wait,
        Err(&'static str),
        BlankFrame,
    }

    struct ScriptedSource(Arc<Mutex<std::collections::VecDeque<Step>>>);
    impl ScreenFrameSource for ScriptedSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            let next = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Step::BlankFrame);
            match next {
                Step::Wait => Ok(None),
                Step::Err(msg) => Err(PortError(msg.into())),
                Step::BlankFrame => Ok(Some(RgbFrame::new(1280, 60, vec![0; 1280 * 60 * 3]))),
            }
        }
    }

    let script = Arc::new(Mutex::new(std::collections::VecDeque::from([Step::Wait])));
    let mut worker = BoundedVisionWorker::spawn_with_source(ScriptedSource(Arc::clone(&script)))
        .expect("spawn worker");

    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(
        worker.runtime_health(),
        CaptureRuntimeHealth::WaitingForGame
    );

    script
        .lock()
        .unwrap()
        .push_back(Step::Err("PrintWindow failed"));
    worker.reset();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if matches!(
            worker.runtime_health(),
            CaptureRuntimeHealth::CaptureError(_)
        ) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(matches!(
        worker.runtime_health(),
        CaptureRuntimeHealth::CaptureError(ref msg) if msg.contains("PrintWindow failed")
    ));

    // A blank valid frame with zero Hero picks marks Capturing (stationary/empty frame != capture error)
    script.lock().unwrap().push_back(Step::BlankFrame);
    worker.reset();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if worker.runtime_health() == CaptureRuntimeHealth::Capturing {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(worker.runtime_health(), CaptureRuntimeHealth::Capturing);

    // Suppression transitions cleanly and drop completes promptly.
    worker.set_capture_mode(VisionCaptureMode::Suppressed);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if worker.runtime_health() == CaptureRuntimeHealth::Suppressed {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(worker.runtime_health(), CaptureRuntimeHealth::Suppressed);
    drop(worker);

    // Stalled capture call on drop must not block shutdown indefinitely.
    let (tx_started, rx_started) = mpsc::channel();
    struct StalledSource(Option<mpsc::Sender<()>>);
    impl ScreenFrameSource for StalledSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
            std::thread::sleep(Duration::from_secs(2));
            Ok(None)
        }
    }
    let stalled_worker =
        BoundedVisionWorker::spawn_with_source(StalledSource(Some(tx_started))).unwrap();
    rx_started
        .recv_timeout(Duration::from_secs(1))
        .expect("capture entered");
    let drop_start = Instant::now();
    drop(stalled_worker);
    assert!(
        drop_start.elapsed() < Duration::from_millis(800),
        "BoundedVisionWorker::drop blocked too long on stalled capture: {:?}",
        drop_start.elapsed()
    );
}

#[test]
fn platform_paths_and_steam_vdf_support_windows_without_home_and_preserve_linux() {
    use crate::platform::{
        PlatformEnv, build_curl_args, detect_steam_vdf_in_candidates, parse_windows_reg_steam_path,
        resolve_cache_dir_for, resolve_config_file_path_for, steam_vdf_candidates_for,
    };
    use std::path::{Path, PathBuf};

    // 1. Linux preserves existing $HOME paths and falls back to temp_dir instead of /tmp when HOME is unset.
    let linux_env = PlatformEnv {
        home: Some(PathBuf::from("/home/tester")),
        temp_dir: PathBuf::from("/var/tmp/ts-test"),
        ..PlatformEnv::default()
    };
    assert_eq!(
        resolve_config_file_path_for("linux", &linux_env),
        PathBuf::from("/home/tester")
            .join(".config")
            .join("truesight-dota")
            .join("config.toml")
    );
    assert_eq!(
        resolve_cache_dir_for("linux", &linux_env),
        PathBuf::from("/home/tester")
            .join(".local")
            .join("share")
            .join("truesight-dota")
            .join("cache")
    );

    let linux_no_home = PlatformEnv {
        temp_dir: PathBuf::from("/var/tmp/ts-headless"),
        ..PlatformEnv::default()
    };
    assert_eq!(
        resolve_config_file_path_for("linux", &linux_no_home),
        PathBuf::from("/var/tmp/ts-headless")
            .join(".config")
            .join("truesight-dota")
            .join("config.toml")
    );
    assert_eq!(
        resolve_cache_dir_for("linux", &linux_no_home),
        PathBuf::from("/var/tmp/ts-headless")
            .join(".local")
            .join("share")
            .join("truesight-dota")
            .join("cache")
    );

    // 2. Windows ignores unset HOME, prefers APPDATA / LOCALAPPDATA, falls back to USERPROFILE or temp_dir, never /tmp.
    let win_env = PlatformEnv {
        home: None,
        appdata: Some(PathBuf::from(r"C:\Users\Player\AppData\Roaming")),
        localappdata: Some(PathBuf::from(r"C:\Users\Player\AppData\Local")),
        userprofile: Some(PathBuf::from(r"C:\Users\Player")),
        program_files_x86: Some(PathBuf::from(r"C:\Program Files (x86)")),
        program_files: Some(PathBuf::from(r"C:\Program Files")),
        temp_dir: PathBuf::from(r"C:\Users\Player\AppData\Local\Temp"),
    };
    assert_eq!(
        resolve_config_file_path_for("windows", &win_env),
        PathBuf::from(r"C:\Users\Player\AppData\Roaming")
            .join("truesight-dota")
            .join("config.toml")
    );
    assert_eq!(
        resolve_cache_dir_for("windows", &win_env),
        PathBuf::from(r"C:\Users\Player\AppData\Local")
            .join("truesight-dota")
            .join("cache")
    );

    let win_profile_only = PlatformEnv {
        userprofile: Some(PathBuf::from(r"C:\Users\Player")),
        temp_dir: PathBuf::from(r"C:\Windows\Temp"),
        ..PlatformEnv::default()
    };
    assert_eq!(
        resolve_config_file_path_for("windows", &win_profile_only),
        PathBuf::from(r"C:\Users\Player")
            .join("AppData")
            .join("Roaming")
            .join("truesight-dota")
            .join("config.toml")
    );
    assert_eq!(
        resolve_cache_dir_for("windows", &win_profile_only),
        PathBuf::from(r"C:\Users\Player")
            .join("AppData")
            .join("Local")
            .join("truesight-dota")
            .join("cache")
    );

    let win_empty = PlatformEnv {
        temp_dir: PathBuf::from(r"C:\Windows\Temp"),
        ..PlatformEnv::default()
    };
    assert_eq!(
        resolve_config_file_path_for("windows", &win_empty),
        PathBuf::from(r"C:\Windows\Temp")
            .join("truesight-dota")
            .join("config.toml")
    );
    assert_eq!(
        resolve_cache_dir_for("windows", &win_empty),
        PathBuf::from(r"C:\Windows\Temp")
            .join("truesight-dota")
            .join("cache")
    );

    // 3. Windows Steam loginusers.vdf candidates & registry SteamPath parsing + real temp fixture fallback.
    let reg_stdout = "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    SteamPath    REG_SZ    D:/Games/Steam\r\n\r\n";
    let parsed_reg = parse_windows_reg_steam_path(reg_stdout);
    assert_eq!(parsed_reg.as_deref(), Some(Path::new("D:/Games/Steam")));
    assert_eq!(
        parse_windows_reg_steam_path("ERROR: unable to find key"),
        None
    );

    let win_candidates = steam_vdf_candidates_for("windows", &win_env, parsed_reg.as_deref());
    assert_eq!(
        win_candidates,
        vec![
            PathBuf::from(r"C:\Program Files (x86)")
                .join("Steam")
                .join("config")
                .join("loginusers.vdf"),
            PathBuf::from(r"C:\Program Files")
                .join("Steam")
                .join("config")
                .join("loginusers.vdf"),
            PathBuf::from(r"C:\Users\Player\AppData\Local")
                .join("Steam")
                .join("config")
                .join("loginusers.vdf"),
            PathBuf::from("D:/Games/Steam")
                .join("config")
                .join("loginusers.vdf"),
        ]
    );

    let tmp_root = std::env::temp_dir().join(format!("ts_win_vdf_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_root);
    let custom_steam = tmp_root.join("CustomSteam");
    std::fs::create_dir_all(custom_steam.join("config")).unwrap();
    std::fs::write(
        custom_steam.join("config").join("loginusers.vdf"),
        r#""users" { "76561198061460213" { "MostRecent" "1" } }"#,
    )
    .unwrap();
    let vdf_env = PlatformEnv {
        program_files_x86: Some(tmp_root.join("MissingProgramFilesX86")),
        ..PlatformEnv::default()
    };
    let candidates = steam_vdf_candidates_for("windows", &vdf_env, Some(&custom_steam));
    assert_eq!(
        detect_steam_vdf_in_candidates(&candidates),
        Some(101_194_485)
    );
    let _ = std::fs::remove_dir_all(&tmp_root);

    // 4. Shared curl argument builder unifies OpenDota GET & STRATZ POST invocations.
    assert_eq!(
        build_curl_args("https://api.opendota.com/api/heroStats", &[]),
        vec![
            "-fsSL",
            "--max-time",
            "6",
            "https://api.opendota.com/api/heroStats"
        ]
    );
    assert_eq!(
        build_curl_args(
            "https://api.stratz.com/graphql",
            &["-H", "Content-Type: application/json", "-d", "{}"]
        ),
        vec![
            "-fsSL",
            "--max-time",
            "6",
            "-H",
            "Content-Type: application/json",
            "-d",
            "{}",
            "https://api.stratz.com/graphql",
        ]
    );
}

#[test]
fn vision_rejects_false_hero_under_interior_glow() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter,
        compute_ingame_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::{Role, Side};
    use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

    struct Once(Option<RgbFrame>);
    impl ScreenFrameSource for Once {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(self.0.take())
        }
    }
    let atlas = parse_hero_atlas().unwrap();
    let ids = [88, 48, 100, 74, 72, 5, 1, 2, 94, 17];
    let (w, h) = (1920, 1080);
    let (rad, dire) = compute_ingame_topbar_slot_rects(w, h);
    let rects: Vec<_> = rad.into_iter().chain(dire).collect();
    let mut baseline = vec![16; w * h * 3];
    for (rect, id) in rects.iter().zip(ids) {
        let src = &atlas.heroes[&HeroId(id)];
        for y in 0..33 {
            for x in 0..57 {
                let dst = ((rect.y + y) * w + rect.x + x) * 3;
                baseline[dst..dst + 3].copy_from_slice(&[36, 40, 48]);
            }
        }
        // Pillow NEAREST uses pixel centers, not floor(dx * source / destination).
        for y in 0..27 {
            for x in 0..47 {
                let sy = 2 + ((2 * y + 1) * 32) / 54;
                let sx = 4 + ((2 * x + 1) * 56) / 94;
                let src_idx = (sy * atlas.hero_width + sx) * 3;
                let dst = ((rect.y + 5 + y) * w + rect.x + 5 + x) * 3;
                baseline[dst..dst + 3].copy_from_slice(&src[src_idx..src_idx + 3]);
            }
        }
    }
    for case in ["baseline", "thin", "blank", "glow"] {
        let mut rgb = baseline.clone();
        let rect = rects[0];
        for y in 0..33_usize {
            for x in 0..57_usize {
                let d = x.min(56 - x).min(y).min(32 - y);
                let dst = ((rect.y + y) * w + rect.x + x) * 3;
                if case == "blank" {
                    let color = if d >= 3 {
                        [16, 16, 16]
                    } else if (17 * x + 31 * y) % 7 < 3 {
                        [255, 220, 80]
                    } else {
                        [10, 70, 130]
                    };
                    rgb[dst..dst + 3].copy_from_slice(&color);
                } else {
                    let a = if case == "glow" {
                        0.5 * (1.0 - d as f64 / 16.0).max(0.0)
                    } else if case == "thin" && d < 3 {
                        0.5
                    } else {
                        0.0
                    };
                    for (channel, overlay) in [255, 240, 100].into_iter().enumerate() {
                        rgb[dst + channel] = ((1.0 - a) * f64::from(rgb[dst + channel])
                            + a * f64::from(overlay))
                        .round_ties_even() as u8;
                    }
                }
            }
        }
        let mut adapter = VisionCaptureAdapter::new(
            Once(Some(RgbFrame::new(w, h, rgb))),
            HeroVisionCatalog::from_embedded().unwrap(),
        );
        let events = adapter.poll_events().unwrap();
        eprintln!("{case}: {events:?}");
        for (index, id) in ids.into_iter().enumerate() {
            let side = if index < 5 {
                Side::Allies
            } else {
                Side::Enemies
            };
            let slot = index % 5;
            let expected = DraftEvent::PickDetected {
                side,
                slot,
                hero_id: HeroId(id),
                role: Some(Role::ALL[slot]),
            };
            if index != 0 || case == "baseline" || case == "thin" {
                assert!(events.contains(&expected), "{case}: missing {expected:?}");
            }
            assert!(
                events.iter().all(|event| match event {
                    DraftEvent::PickDetected {
                        side: s,
                        slot: n,
                        hero_id,
                        ..
                    } if *s == side && *n == slot =>
                        *hero_id == HeroId(id) && !(case == "blank" && index == 0),
                    _ => true,
                }),
                "{case}: false identity in {events:?}"
            );
        }
        assert!(adapter.poll_events().unwrap().is_empty());
    }
}

#[test]
fn vision_ingame_excludes_player_color_strip_and_spectator_borders() {
    use crate::hero_alt_atlas::parse_hero_alt_atlas;
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, TopBarSlotRect, VisionCaptureAdapter,
        VisionCaptureMode, compute_ingame_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::{Faction, Side};

    struct SingleFrameSource(Option<RgbFrame>);
    impl ScreenFrameSource for SingleFrameSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.take())
        }
    }

    let atlas = parse_hero_atlas().expect("base atlas must load");
    let alt_atlas = parse_hero_alt_atlas().expect("alt atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");

    let (w, h) = (1920_usize, 1080_usize);
    let (rad_rects, dire_rects) = compute_ingame_topbar_slot_rects(w, h);
    let mut frame_rgb = vec![16_u8; w * h * 3];

    let player_colors: [[u8; 3]; 10] = [
        [46, 115, 230],  // Blue
        [102, 255, 192], // Teal
        [191, 0, 191],   // Purple
        [243, 240, 11],  // Yellow
        [255, 107, 0],   // Orange
        [254, 134, 194], // Pink
        [161, 180, 71],  // Olive
        [101, 217, 247], // Light Blue
        [0, 131, 33],    // Dark Green
        [164, 105, 0],   // Brown
    ];

    let paint_contaminated_slot =
        |buf: &mut [u8], rect: TopBarSlotRect, src_rgb: &[u8], strip_rgb: [u8; 3], glow: bool| {
            // Paint bright spectator border / bottom bar / Dota Plus corner triangles across full slot
            for dy in 0..rect.height {
                for dx in 0..rect.width {
                    let px = rect.x + dx;
                    let py = rect.y + dy;
                    let dst = (py * w + px) * 3;
                    if dy < 6 {
                        buf[dst..dst + 3].copy_from_slice(&strip_rgb);
                    } else if glow {
                        buf[dst..dst + 3].copy_from_slice(&[245, 230, 165]);
                    } else {
                        buf[dst..dst + 3].copy_from_slice(&[36, 40, 48]);
                    }
                }
            }
            // Paint the actual portrait inside the clean inner core (x+5..w-5, y+5..h-1)
            let inner = TopBarSlotRect {
                x: rect.x + 5,
                y: rect.y + 5,
                width: rect.width.saturating_sub(10),
                height: rect.height.saturating_sub(6),
            };
            let (sx0, sw0, sy0, sh0) = (
                4_usize,
                atlas.hero_width - 8,
                2_usize,
                atlas.hero_height - 4,
            );
            for dy in 0..inner.height {
                let sy = sy0 + (dy * sh0) / inner.height;
                for dx in 0..inner.width {
                    let sx = sx0 + (dx * sw0) / inner.width;
                    let si = (sy * atlas.hero_width + sx) * 3;
                    let di = ((inner.y + dy) * w + (inner.x + dx)) * 3;
                    buf[di..di + 3].copy_from_slice(&src_rgb[si..si + 3]);
                }
            }
        };

    // Match 1 failing Radiant heroes from #43: Nyx(88), Luna(48), Tusk(100), Invoker(74), Gyro(72)
    let rad_ids = [88_u16, 48, 100, 74, 72];
    // Dire includes Crystal Maiden(5) + Anti-Mage Persona(1) + Axe(2) + Medusa(94) + Storm(17)
    let am_persona = &alt_atlas
        .entries
        .iter()
        .find(|(id, _)| *id == HeroId(1))
        .expect("AM persona must exist")
        .1;

    for (slot, &hid) in rad_ids.iter().enumerate() {
        paint_contaminated_slot(
            &mut frame_rgb,
            rad_rects[slot],
            &atlas.heroes[&HeroId(hid)],
            player_colors[slot],
            slot == 3, // Invoker has active spectator selection glow
        );
    }

    let dire_ids = [5_u16, 1, 2, 94, 17];
    for (slot, &hid) in dire_ids.iter().enumerate() {
        let src: &[u8] = if hid == 1 {
            am_persona
        } else {
            &atlas.heroes[&HeroId(hid)]
        };
        paint_contaminated_slot(
            &mut frame_rgb,
            dire_rects[slot],
            src,
            player_colors[5 + slot],
            slot == 1, // AM Persona has active spectator glow
        );
    }

    let mut adapter = VisionCaptureAdapter::new(
        SingleFrameSource(Some(RgbFrame {
            width: w,
            height: h,
            source_height: None,
            capture_strip_rect: None,
            rgb: frame_rgb,
        })),
        catalog,
    );
    adapter.set_ally_faction(Faction::Radiant);
    adapter.set_capture_mode(VisionCaptureMode::Any);

    let events = adapter.poll_events().expect("poll_events must succeed");
    for (slot, &hid) in rad_ids.iter().enumerate() {
        assert!(
            events.contains(&truesight_core::ports::DraftEvent::PickDetected {
                side: Side::Allies,
                slot,
                hero_id: HeroId(hid),
                role: Some(truesight_core::domain::Role::ALL[slot]),
            }),
            "Radiant slot {slot} must detect HeroId({hid}) despite color strip & spectator glow; got {events:?}"
        );
    }
    for (slot, &hid) in dire_ids.iter().enumerate() {
        assert!(
            events.contains(&truesight_core::ports::DraftEvent::PickDetected {
                side: Side::Enemies,
                slot,
                hero_id: HeroId(hid),
                role: Some(truesight_core::domain::Role::ALL[slot]),
            }),
            "Dire slot {slot} must detect HeroId({hid}) despite color strip & spectator glow; got {events:?}"
        );
    }
}

#[test]
fn vision_ingame_2560x1440_real_geometry_detects_rubick_and_dota_plus_badges() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter, VisionCaptureMode,
        parse_hero_atlas,
    };
    use truesight_core::domain::{Faction, Role, Side};
    use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

    struct SingleFrame(Option<RgbFrame>);
    impl ScreenFrameSource for SingleFrame {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(self.0.take())
        }
    }

    let atlas = parse_hero_atlas().expect("base atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");

    // Real 2560x1440 Dota 2 In-Game top-bar strip: 82x46 portraits at y = 6..52 below a 5px
    // player-color bar, cropped to 2560x115 with source_height = 1440.
    let (w, h, source_h) = (2560_usize, 115_usize, 1440_usize);
    let mut rgb = vec![16_u8; w * h * 3];

    let player_colors: [[u8; 3]; 10] = [
        [46, 115, 230],
        [102, 255, 192],
        [191, 0, 191],
        [243, 240, 11],
        [255, 107, 0],
        [254, 134, 194],
        [161, 180, 71],
        [101, 217, 247],
        [0, 131, 33],
        [164, 105, 0],
    ];
    let paint_real_1440_slot =
        |buf: &mut [u8],
         x0: usize,
         hid: u16,
         strip_rgb: [u8; 3],
         badge: Option<(usize, usize, usize, usize, [u8; 3])>| {
            // 5px top player color bar (y = 0..5)
            for py in 0..5 {
                for px in x0..(x0 + 82) {
                    let dst = (py * w + px) * 3;
                    buf[dst..dst + 3].copy_from_slice(&strip_rgb);
                }
            }
            // 82x46 portrait (y = 6..52) matching real 1440p In-Game HUD proportions
            let src = &atlas.heroes[&HeroId(hid)];
            for dy in 0..46_usize {
                let sy = (dy * atlas.hero_height) / 46;
                for dx in 0..82_usize {
                    let sx = (dx * atlas.hero_width) / 82;
                    let si = (sy * atlas.hero_width + sx) * 3;
                    let dst = ((6 + dy) * w + x0 + dx) * 3;
                    buf[dst..dst + 3].copy_from_slice(&src[si..si + 3]);
                }
            }
            // Opaque Dota Plus tier corner badge in the bottom corner (matching live measurements)
            if let Some((bx0, bx1, by0, by1, badge_rgb)) = badge {
                for dy in by0..by1 {
                    for dx in bx0..bx1 {
                        let dst = ((6 + dy) * w + x0 + dx) * 3;
                        buf[dst..dst + 3].copy_from_slice(&badge_rgb);
                    }
                }
            }
        };

    // Match lineup from #87: Invoker(74), Axe(2), Shadow Shaman(27), Juggernaut(8), Pudge(14)
    // vs Rubick(86), Templar Assassin(46), Mars(129), Sand King(16), Hoodwink(123).
    let rad_ids = [74_u16, 2, 27, 8, 14];
    let dire_ids = [86_u16, 46, 129, 16, 123];
    let bronze_badge = [240_u8, 212, 132];
    let platinum_badge = [52_u8, 130, 235];

    for (slot, &hid) in rad_ids.iter().enumerate() {
        let x0 = (728.0 + (slot as f32) * 83.5).round() as usize;
        let badge = if slot == 4 {
            // Radiant slot 4 Pudge wearing a bright blue Platinum Dota Plus shield
            Some((2, 25, 22, 44, platinum_badge))
        } else {
            Some((2, 24, 25, 45, bronze_badge))
        };
        paint_real_1440_slot(&mut rgb, x0, hid, player_colors[slot], badge);
    }
    for (slot, &hid) in dire_ids.iter().enumerate() {
        let x0 = (1420.0 + (slot as f32) * 83.5).round() as usize;
        // Dire portraits carry the bright Dota Plus tier shield highlight in bottom-right
        // (dx=52..78, dy=24..45), overlapping cols 6..7 of ingame_inner_core_rect.
        paint_real_1440_slot(
            &mut rgb,
            x0,
            hid,
            player_colors[5 + slot],
            Some((52, 78, 24, 45, bronze_badge)),
        );
    }

    let mut adapter = VisionCaptureAdapter::new(
        SingleFrame(Some(RgbFrame::with_source_height(w, h, source_h, rgb))),
        catalog,
    );
    adapter.set_ally_faction(Faction::Radiant);
    adapter.set_capture_mode(VisionCaptureMode::Any);

    let events = adapter.poll_events().expect("poll_events must succeed");
    assert!(
        events.contains(&DraftEvent::PickDetected {
            side: Side::Enemies,
            slot: 0,
            hero_id: HeroId(86),
            role: Some(Role::Carry),
        }),
        "Dire slot 0 must detect base Rubick (86) despite Dota Plus badge in 2560x1440 InGame strip; got {events:?}"
    );
    assert!(
        events.contains(&DraftEvent::PickDetected {
            side: Side::Allies,
            slot: 4,
            hero_id: HeroId(14),
            role: Some(Role::HardSupport),
        }),
        "Radiant slot 4 must detect Pudge (14) despite Platinum shield in 2560x1440 InGame strip; got {events:?}"
    );
    for (slot, &hid) in rad_ids.iter().enumerate() {
        assert!(
            events.contains(&DraftEvent::PickDetected {
                side: Side::Allies,
                slot,
                hero_id: HeroId(hid),
                role: Some(Role::ALL[slot]),
            }),
            "Radiant slot {slot} missing HeroId({hid}) in real 2560x1440 InGame strip: {events:?}"
        );
    }
    for (slot, &hid) in dire_ids.iter().enumerate() {
        assert!(
            events.contains(&DraftEvent::PickDetected {
                side: Side::Enemies,
                slot,
                hero_id: HeroId(hid),
                role: Some(Role::ALL[slot]),
            }),
            "Dire slot {slot} missing HeroId({hid}) in real 2560x1440 InGame strip: {events:?}"
        );
    }
}

#[test]
fn vision_ingame_rubick_overwrites_stale_visage_lock_after_two_frames() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter, VisionCaptureMode,
        compute_ingame_topbar_slot_rects, parse_hero_atlas,
    };
    use std::collections::VecDeque;
    use truesight_core::domain::{Faction, Role, Side};
    use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

    struct QueueFrames(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueFrames {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(self.0.pop_front())
        }
    }

    let atlas = parse_hero_atlas().expect("base atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    let (w, h, source_h) = (2560_usize, 115_usize, 1440_usize);

    let paint_lineup_frame = |dire0_hero: u16, dire0_badge: bool, dire0_desaturated: bool| {
        let mut buf = vec![16_u8; w * h * 3];
        let rad_ids = [74_u16, 2, 27, 8, 14];
        let dire_ids = [dire0_hero, 46, 129, 16, 123];
        for (idx, (x0, hid)) in rad_ids
            .iter()
            .enumerate()
            .map(|(i, &id)| (((728.0 + (i as f32) * 83.5).round() as usize), id))
            .chain(
                dire_ids
                    .iter()
                    .enumerate()
                    .map(|(i, &id)| (((1420.0 + (i as f32) * 83.5).round() as usize), id)),
            )
            .enumerate()
        {
            let src = &atlas.heroes[&HeroId(hid)];
            for dy in 0..46_usize {
                let sy = (dy * atlas.hero_height) / 46;
                for dx in 0..82_usize {
                    let sx = (dx * atlas.hero_width) / 82;
                    let si = (sy * atlas.hero_width + sx) * 3;
                    let dst = ((6 + dy) * w + x0 + dx) * 3;
                    if idx == 5 && dire0_desaturated {
                        let lum =
                            ((u16::from(src[si]) + u16::from(src[si + 1]) + u16::from(src[si + 2]))
                                / 3) as u8;
                        buf[dst..dst + 3].copy_from_slice(&[lum, lum, lum]);
                    } else {
                        buf[dst..dst + 3].copy_from_slice(&src[si..si + 3]);
                    }
                }
            }
            if idx == 5 && dire0_badge {
                for dy in 24..45_usize {
                    for dx in 52..78_usize {
                        let dst = ((6 + dy) * w + x0 + dx) * 3;
                        buf[dst..dst + 3].copy_from_slice(&[240, 212, 132]);
                    }
                }
            }
        }
        RgbFrame::with_source_height(w, h, source_h, buf)
    };

    // Frame 0: stale/misdetected Visage(92) in Dire slot 0 -> locked.
    // Frames 1..2: real Rubick(86) + bottom-right bronze Dota Plus badge -> must overwrite on frame 2.
    // Frame 3: desaturated dead Rubick portrait -> must NOT overwrite or emit phantom swaps.
    // Frames 4..5: two consecutive ambiguous shifted Nature's Prophet(53, (0,+2), bronze) frames.
    // Frames 6..7: two consecutive half-obscured LeftHalf Slardar(28) frames.
    let (rad_rects_1440, dire_rects_1440) = compute_ingame_topbar_slot_rects(w, source_h);
    let mut np_shifted_frame = vec![16_u8; w * h * 3];
    for (idx, (x0, hid)) in [
        (rad_rects_1440[0].x, 74_u16),
        (rad_rects_1440[2].x, 27),
        (dire_rects_1440[0].x, 53),
        (dire_rects_1440[3].x, 16),
    ]
    .into_iter()
    .enumerate()
    {
        paint_real_ingame_hud_slot(
            &atlas,
            &mut np_shifted_frame,
            w,
            h,
            source_h,
            HudSlotSpec {
                base_x0: x0,
                dx_off: 0,
                dy_off: if idx == 2 { 3 } else { 0 },
                hid,
                is_dire: idx >= 2,
                strip_rgb: Some([180, 120, 60]),
                badge: if idx == 2 {
                    HudBadgeKind::Bronze
                } else {
                    HudBadgeKind::None
                },
                partial: HudPartialMask::Full,
            },
        );
    }

    let mut slardar_partial_frame = vec![16_u8; w * h * 3];
    for (idx, (x0, hid, partial)) in [
        (rad_rects_1440[0].x, 74_u16, HudPartialMask::Full),
        (rad_rects_1440[2].x, 27, HudPartialMask::Full),
        (dire_rects_1440[0].x, 28, HudPartialMask::LeftHalf),
        (dire_rects_1440[3].x, 16, HudPartialMask::Full),
    ]
    .into_iter()
    .enumerate()
    {
        paint_real_ingame_hud_slot(
            &atlas,
            &mut slardar_partial_frame,
            w,
            h,
            source_h,
            HudSlotSpec {
                base_x0: x0,
                dx_off: 0,
                dy_off: 0,
                hid,
                is_dire: idx >= 2,
                strip_rgb: Some([180, 120, 60]),
                badge: HudBadgeKind::None,
                partial,
            },
        );
    }

    let mut adapter = VisionCaptureAdapter::new(
        QueueFrames(VecDeque::from([
            paint_lineup_frame(92, false, false),
            paint_lineup_frame(86, true, false),
            paint_lineup_frame(86, true, false),
            paint_lineup_frame(92, false, true),
            RgbFrame::with_source_height(w, h, source_h, np_shifted_frame.clone()),
            RgbFrame::with_source_height(w, h, source_h, np_shifted_frame),
            RgbFrame::with_source_height(w, h, source_h, slardar_partial_frame.clone()),
            RgbFrame::with_source_height(w, h, source_h, slardar_partial_frame),
        ])),
        catalog,
    );
    adapter.set_ally_faction(Faction::Radiant);
    adapter.set_capture_mode(VisionCaptureMode::Any);

    let ev0 = adapter.poll_events().expect("frame 0 must succeed");
    assert!(
        ev0.contains(&DraftEvent::PickDetected {
            side: Side::Enemies,
            slot: 0,
            hero_id: HeroId(92),
            role: Some(Role::Carry),
        }),
        "Frame 0 must establish initial Visage (92) lock in Dire slot 0: {ev0:?}"
    );

    let ev1 = adapter.poll_events().expect("frame 1 must succeed");
    assert!(
        ev1.is_empty(),
        "First differing Rubick frame must obey OVERWRITE_CONFIRM_FRAMES=2 hysteresis: {ev1:?}"
    );

    let ev2 = adapter.poll_events().expect("frame 2 must succeed");
    assert_eq!(
        ev2,
        vec![DraftEvent::PickDetected {
            side: Side::Enemies,
            slot: 0,
            hero_id: HeroId(86),
            role: Some(Role::Carry),
        }],
        "Second consecutive Rubick (86) frame must overwrite stale Visage (92) lock: {ev2:?}"
    );

    let ev3 = adapter.poll_events().expect("frame 3 must succeed");
    assert!(
        ev3.is_empty(),
        "Desaturated dead portrait must not overwrite locked Rubick (86): {ev3:?}"
    );

    for (frame_idx, label) in [
        (4, "ambiguous shifted 1"),
        (5, "ambiguous shifted 2"),
        (6, "half-obscured partial 1"),
        (7, "half-obscured partial 2"),
    ] {
        let ev = adapter.poll_events().expect("stability poll must succeed");
        assert!(
            ev.iter().all(|e| !matches!(
                e,
                DraftEvent::PickDetected {
                    side: Side::Enemies,
                    slot: 0,
                    ..
                }
            )),
            "Frame {frame_idx} ({label}) must preserve locked Rubick (86) in Dire slot 0: {ev:?}"
        );
    }
}

#[derive(Clone, Copy)]
enum HudBadgeKind {
    None,
    Thin,
    Bronze,
    Platinum,
}

#[derive(Clone, Copy)]
enum HudPartialMask {
    Full,
    LeftHalf,
    RightHalf,
    BottomOnly,
}

struct HudSlotSpec {
    base_x0: usize,
    dx_off: isize,
    dy_off: isize,
    hid: u16,
    is_dire: bool,
    strip_rgb: Option<[u8; 3]>,
    badge: HudBadgeKind,
    partial: HudPartialMask,
}

fn paint_real_ingame_hud_slot(
    atlas: &crate::vision::DecodedHeroAtlas,
    buf: &mut [u8],
    frame_w: usize,
    frame_h: usize,
    virtual_h: usize,
    spec: HudSlotSpec,
) {
    let sx = (frame_w as f32) / 2560.0;
    let sy = (virtual_h as f32) / 1440.0;
    let pw = (82.0 * sx).round() as usize;
    let ph = (46.0 * sy).round() as usize;
    let py0 = (6.0 * sy).round() as isize;
    let strip_h = (5.0 * sy).round() as isize;
    let x0 = (spec.base_x0 as isize) + spec.dx_off;
    let y0 = py0 + spec.dy_off;

    if let Some(strip) = spec.strip_rgb {
        for py in spec.dy_off.max(0)..(spec.dy_off + strip_h).min(frame_h as isize) {
            for px in x0.max(0)..(x0 + pw as isize).min(frame_w as isize) {
                let dst = ((py as usize) * frame_w + (px as usize)) * 3;
                buf[dst..dst + 3].copy_from_slice(&strip);
            }
        }
    }

    let src = &atlas.heroes[&HeroId(spec.hid)];
    for dy in 0..ph {
        let sy_idx = (dy * atlas.hero_height) / ph;
        let py = y0 + (dy as isize);
        if py < 0 || py >= frame_h as isize {
            continue;
        }
        for dx in 0..pw {
            match spec.partial {
                HudPartialMask::Full => {}
                HudPartialMask::LeftHalf if dx >= pw / 2 => continue,
                HudPartialMask::RightHalf if dx < pw / 2 => continue,
                HudPartialMask::BottomOnly if dy < ph / 2 => continue,
                _ => {}
            }
            let sx_idx = (dx * atlas.hero_width) / pw;
            let px = x0 + (dx as isize);
            if px < 0 || px >= frame_w as isize {
                continue;
            }
            let si = (sy_idx * atlas.hero_width + sx_idx) * 3;
            let dst = ((py as usize) * frame_w + (px as usize)) * 3;
            buf[dst..dst + 3].copy_from_slice(&src[si..si + 3]);
        }
    }

    let (bcol, alpha, by0_ref) = match spec.badge {
        HudBadgeKind::None => return,
        HudBadgeKind::Thin => ([240_u8, 212, 132], 0.55_f32, 28.0_f32),
        HudBadgeKind::Bronze => ([240_u8, 212, 132], 1.0_f32, 24.0_f32),
        HudBadgeKind::Platinum => ([52_u8, 130, 235], 1.0_f32, 22.0_f32),
    };
    let bx0 = ((if spec.is_dire {
        if matches!(spec.badge, HudBadgeKind::Thin) {
            56.0
        } else {
            52.0
        }
    } else {
        2.0
    }) * sx)
        .round() as usize;
    let bx1 = ((if spec.is_dire { 78.0 } else { 24.0 }) * sx).round() as usize;
    let by0 = (by0_ref * sy).round() as usize;
    let by1 = (45.0 * sy).round() as usize;
    for dy in by0..by1 {
        let py = y0 + (dy as isize);
        if py < 0 || py >= frame_h as isize {
            continue;
        }
        for dx in bx0..bx1 {
            let px = x0 + (dx as isize);
            if px < 0 || px >= frame_w as isize {
                continue;
            }
            let dst = ((py as usize) * frame_w + (px as usize)) * 3;
            for c in 0..3 {
                buf[dst + c] = ((1.0 - alpha) * f32::from(buf[dst + c])
                    + alpha * f32::from(bcol[c]))
                .round() as u8;
            }
        }
    }
}

#[test]
fn vision_ingame_shifted_upper_only_matches_abstain_instead_of_locking_wrong_hero() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter, VisionCaptureMode,
        compute_ingame_topbar_slot_rects, parse_hero_atlas,
    };
    use std::collections::VecDeque;
    use truesight_core::domain::{Faction, Side};
    use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

    struct QueueFrames(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueFrames {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(self.0.pop_front())
        }
    }

    let atlas = parse_hero_atlas().expect("base atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    let (w, h, source_h) = (1920_usize, 115_usize, 1080_usize);
    let (rad_rects, dire_rects) = compute_ingame_topbar_slot_rects(w, source_h);

    // Exact 6 1080p bronze corner-badge offset cases reproduced from PR #88 review:
    let cases: [(&str, u16, bool, isize, isize, u16); 6] = [
        ("Nature's Prophet Dire (0,+2)", 53, true, 0, 2, 21),
        ("Nature's Prophet Rad (0,+2)", 53, false, 0, 2, 21),
        ("Oracle Rad (-2,+2)", 111, false, -2, 2, 77),
        ("Nyx Assassin Dire (+3,0)", 88, true, 3, 0, 137),
        ("Nyx Assassin Dire (+2,-2)", 88, true, 2, -2, 137),
        ("Legion Commander Dire (+2,-2)", 104, true, 2, -2, 62),
    ];

    for (label, target_hid, is_dire, dx, dy, known_wrong) in cases {
        let mut buf = vec![16_u8; w * h * 3];
        // Aligned control picks satisfy quorum (Radiant slot 2 Invoker, Dire slot 2 Sand King).
        paint_real_ingame_hud_slot(
            &atlas,
            &mut buf,
            w,
            h,
            source_h,
            HudSlotSpec {
                base_x0: rad_rects[2].x,
                dx_off: 0,
                dy_off: 0,
                hid: 74,
                is_dire: false,
                strip_rgb: Some([191, 0, 191]),
                badge: HudBadgeKind::None,
                partial: HudPartialMask::Full,
            },
        );
        paint_real_ingame_hud_slot(
            &atlas,
            &mut buf,
            w,
            h,
            source_h,
            HudSlotSpec {
                base_x0: dire_rects[2].x,
                dx_off: 0,
                dy_off: 0,
                hid: 16,
                is_dire: true,
                strip_rgb: Some([101, 217, 247]),
                badge: HudBadgeKind::None,
                partial: HudPartialMask::Full,
            },
        );
        let target_x = if is_dire {
            dire_rects[0].x
        } else {
            rad_rects[0].x
        };
        paint_real_ingame_hud_slot(
            &atlas,
            &mut buf,
            w,
            h,
            source_h,
            HudSlotSpec {
                base_x0: target_x,
                dx_off: dx,
                dy_off: dy,
                hid: target_hid,
                is_dire,
                strip_rgb: Some(if is_dire {
                    [254, 134, 194]
                } else {
                    [46, 115, 230]
                }),
                badge: HudBadgeKind::Bronze,
                partial: HudPartialMask::Full,
            },
        );

        let mut adapter = VisionCaptureAdapter::new(
            QueueFrames(VecDeque::from([
                RgbFrame::with_source_height(w, h, source_h, buf.clone()),
                RgbFrame::with_source_height(w, h, source_h, buf.clone()),
                RgbFrame::with_source_height(w, h, source_h, buf),
            ])),
            catalog.clone(),
        );
        adapter.set_ally_faction(Faction::Radiant);
        adapter.set_capture_mode(VisionCaptureMode::Any);

        let target_side = if is_dire { Side::Enemies } else { Side::Allies };
        let mut saw_rad_ctrl = false;
        let mut saw_dire_ctrl = false;
        for frame_idx in 0..3 {
            let events = adapter.poll_events().expect("poll must succeed");
            for ev in events {
                if let DraftEvent::PickDetected {
                    side,
                    slot,
                    hero_id,
                    ..
                } = ev
                {
                    if side == Side::Allies && slot == 2 && hero_id == HeroId(74) {
                        saw_rad_ctrl = true;
                    } else if side == Side::Enemies && slot == 2 && hero_id == HeroId(16) {
                        saw_dire_ctrl = true;
                    } else if side == target_side && slot == 0 {
                        assert_eq!(
                            hero_id,
                            HeroId(target_hid),
                            "{label} on frame {frame_idx} must either abstain or detect HeroId({target_hid}), NOT lock wrong HeroId({hero_id:?}) (known wrong={known_wrong})"
                        );
                    }
                }
            }
        }
        assert!(
            saw_rad_ctrl && saw_dire_ctrl,
            "{label}: aligned control picks must confirm quorum"
        );
    }
}

#[test]
fn vision_ingame_calibration_clean_and_badged_both_resolutions_and_sides() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter, VisionCaptureMode,
        compute_ingame_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::{Faction, Side};
    use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

    struct SingleFrame(Option<RgbFrame>);
    impl ScreenFrameSource for SingleFrame {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(self.0.take())
        }
    }

    let atlas = parse_hero_atlas().expect("base atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");

    for (w, h, source_h) in [(1920_usize, 115_usize, 1080_usize), (2560, 115, 1440)] {
        let (rad_rects, dire_rects) = compute_ingame_topbar_slot_rects(w, source_h);
        for target_hid in [86_u16, 14, 74, 123] {
            for is_dire in [false, true] {
                for slot in [0_usize, 4] {
                    for badge in [
                        HudBadgeKind::None,
                        HudBadgeKind::Thin,
                        HudBadgeKind::Bronze,
                        HudBadgeKind::Platinum,
                    ] {
                        for with_strip in [false, true] {
                            let mut buf = vec![16_u8; w * h * 3];
                            // Quorum control picks at slot 2 on both sides (Axe 2, Mars 129).
                            paint_real_ingame_hud_slot(
                                &atlas,
                                &mut buf,
                                w,
                                h,
                                source_h,
                                HudSlotSpec {
                                    base_x0: rad_rects[2].x,
                                    dx_off: 0,
                                    dy_off: 0,
                                    hid: 2,
                                    is_dire: false,
                                    strip_rgb: Some([191, 0, 191]),
                                    badge: HudBadgeKind::None,
                                    partial: HudPartialMask::Full,
                                },
                            );
                            paint_real_ingame_hud_slot(
                                &atlas,
                                &mut buf,
                                w,
                                h,
                                source_h,
                                HudSlotSpec {
                                    base_x0: dire_rects[2].x,
                                    dx_off: 0,
                                    dy_off: 0,
                                    hid: 129,
                                    is_dire: true,
                                    strip_rgb: Some([101, 217, 247]),
                                    badge: HudBadgeKind::None,
                                    partial: HudPartialMask::Full,
                                },
                            );
                            let x0 = if is_dire {
                                dire_rects[slot].x
                            } else {
                                rad_rects[slot].x
                            };
                            paint_real_ingame_hud_slot(
                                &atlas,
                                &mut buf,
                                w,
                                h,
                                source_h,
                                HudSlotSpec {
                                    base_x0: x0,
                                    dx_off: 0,
                                    dy_off: 0,
                                    hid: target_hid,
                                    is_dire,
                                    strip_rgb: with_strip.then_some(if is_dire {
                                        [254, 134, 194]
                                    } else {
                                        [46, 115, 230]
                                    }),
                                    badge,
                                    partial: HudPartialMask::Full,
                                },
                            );

                            let mut adapter = VisionCaptureAdapter::new(
                                SingleFrame(Some(RgbFrame::with_source_height(
                                    w, h, source_h, buf,
                                ))),
                                catalog.clone(),
                            );
                            adapter.set_ally_faction(Faction::Radiant);
                            adapter.set_capture_mode(VisionCaptureMode::Any);
                            let events = adapter.poll_events().expect("poll must succeed");
                            let expected_side = if is_dire { Side::Enemies } else { Side::Allies };
                            let detected = events.iter().find_map(|ev| match ev {
                                DraftEvent::PickDetected {
                                    side,
                                    slot: s,
                                    hero_id,
                                    ..
                                } if *side == expected_side && *s == slot => Some(*hero_id),
                                _ => None,
                            });
                            assert_eq!(
                                detected,
                                Some(HeroId(target_hid)),
                                "{w}x{source_h} is_dire={is_dire} slot={slot} strip={with_strip} failed for HeroId({target_hid}): {events:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn vision_ingame_negative_dark_blue_clock_and_partial_portraits_emit_zero_phantoms() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter, VisionCaptureMode,
        compute_ingame_topbar_slot_rects, parse_hero_atlas,
    };
    use std::collections::VecDeque;
    use truesight_core::domain::{Faction, Side};
    use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

    struct QueueFrames(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueFrames {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(self.0.pop_front())
        }
    }

    let atlas = parse_hero_atlas().expect("base atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");

    // 1. Dark-blue clock/UI chrome across slots at 2560x1440 must emit 0 picks across 3 frames.
    let (w, h, source_h) = (2560_usize, 115_usize, 1440_usize);
    let (rad_rects, dire_rects) = compute_ingame_topbar_slot_rects(w, source_h);
    let mut chrome_buf = vec![16_u8; w * h * 3];
    for rect in rad_rects.into_iter().chain(dire_rects) {
        for dy in 0..rect.height {
            let v = (dy as f32) / (rect.height as f32);
            for dx in 0..rect.width {
                let u = (dx as f32) / (rect.width as f32);
                let t = 0.6 * u + 0.4 * v;
                let dst = ((rect.y + dy) * w + rect.x + dx) * 3;
                chrome_buf[dst] = (14.0 + 28.0 * t) as u8;
                chrome_buf[dst + 1] = (32.0 + 48.0 * t) as u8;
                chrome_buf[dst + 2] = (54.0 + 70.0 * t) as u8;
            }
        }
    }
    let mut chrome_adapter = VisionCaptureAdapter::new(
        QueueFrames(VecDeque::from([
            RgbFrame::with_source_height(w, h, source_h, chrome_buf.clone()),
            RgbFrame::with_source_height(w, h, source_h, chrome_buf.clone()),
            RgbFrame::with_source_height(w, h, source_h, chrome_buf),
        ])),
        catalog.clone(),
    );
    chrome_adapter.set_ally_faction(Faction::Radiant);
    chrome_adapter.set_capture_mode(VisionCaptureMode::Any);
    for poll in 0..3 {
        let events = chrome_adapter.poll_events().expect("poll must succeed");
        assert!(
            events.is_empty(),
            "Dark-blue clock/chrome must emit 0 picks on poll {poll}: {events:?}"
        );
    }

    // 2. Half-obscured partial portraits across 1080p and 1440p must never emit wrong heroes on repeated frames.
    for (fw, vh) in [(1920_usize, 1080_usize), (2560, 1440)] {
        let (rad, dire) = compute_ingame_topbar_slot_rects(fw, vh);
        for hid in [28_u16, 94, 93] {
            for partial in [
                HudPartialMask::LeftHalf,
                HudPartialMask::RightHalf,
                HudPartialMask::BottomOnly,
            ] {
                let mut buf = vec![16_u8; fw * h * 3];
                paint_real_ingame_hud_slot(
                    &atlas,
                    &mut buf,
                    fw,
                    h,
                    vh,
                    HudSlotSpec {
                        base_x0: rad[2].x,
                        dx_off: 0,
                        dy_off: 0,
                        hid: 74,
                        is_dire: false,
                        strip_rgb: Some([191, 0, 191]),
                        badge: HudBadgeKind::None,
                        partial: HudPartialMask::Full,
                    },
                );
                paint_real_ingame_hud_slot(
                    &atlas,
                    &mut buf,
                    fw,
                    h,
                    vh,
                    HudSlotSpec {
                        base_x0: dire[2].x,
                        dx_off: 0,
                        dy_off: 0,
                        hid: 16,
                        is_dire: true,
                        strip_rgb: Some([101, 217, 247]),
                        badge: HudBadgeKind::None,
                        partial: HudPartialMask::Full,
                    },
                );
                paint_real_ingame_hud_slot(
                    &atlas,
                    &mut buf,
                    fw,
                    h,
                    vh,
                    HudSlotSpec {
                        base_x0: dire[0].x,
                        dx_off: 0,
                        dy_off: 0,
                        hid,
                        is_dire: true,
                        strip_rgb: Some([254, 134, 194]),
                        badge: HudBadgeKind::None,
                        partial,
                    },
                );

                let mut adapter = VisionCaptureAdapter::new(
                    QueueFrames(VecDeque::from([
                        RgbFrame::with_source_height(fw, h, vh, buf.clone()),
                        RgbFrame::with_source_height(fw, h, vh, buf.clone()),
                        RgbFrame::with_source_height(fw, h, vh, buf),
                    ])),
                    catalog.clone(),
                );
                adapter.set_ally_faction(Faction::Radiant);
                adapter.set_capture_mode(VisionCaptureMode::Any);
                for poll in 0..3 {
                    let events = adapter.poll_events().expect("poll must succeed");
                    for ev in events {
                        if let DraftEvent::PickDetected {
                            side: Side::Enemies,
                            slot: 0,
                            hero_id,
                            ..
                        } = ev
                        {
                            assert_eq!(
                                hero_id,
                                HeroId(hid),
                                "Partial portrait HeroId({hid}) at {fw}x{vh} emitted wrong HeroId({hero_id:?}) on poll {poll}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn vision_ingame_upper_core_rect_bounds_and_scaling() {
    use crate::vision::{TopBarSlotRect, compute_ingame_topbar_slot_rects, ingame_upper_core_rect};

    let (rad_1440, dire_1440) = compute_ingame_topbar_slot_rects(2560, 1440);
    for rect in rad_1440.into_iter().chain(dire_1440) {
        let upper = ingame_upper_core_rect(rect);
        assert_eq!(
            upper,
            TopBarSlotRect {
                x: rect.x + 6,
                y: 8,
                width: 70,
                height: 26,
            },
            "2560x1440 upper core must sit strictly inside rect above Dota Plus bottom badges"
        );
        assert!(upper.x + upper.width <= rect.x + rect.width);
        assert!(upper.y + upper.height <= rect.y + rect.height);
    }

    let (rad_1080, dire_1080) = compute_ingame_topbar_slot_rects(1920, 1080);
    for rect in rad_1080.into_iter().chain(dire_1080) {
        let upper = ingame_upper_core_rect(rect);
        assert_eq!(
            upper,
            TopBarSlotRect {
                x: rect.x + 5,
                y: 6,
                width: 52,
                height: 20,
            },
            "1920x1080 upper core must floor top inset to avoid double-rounding drift"
        );
        assert!(upper.x + upper.width <= rect.x + rect.width);
        assert!(upper.y + upper.height <= rect.y + rect.height);
    }

    let tiny = TopBarSlotRect {
        x: 10,
        y: 2,
        width: 8,
        height: 8,
    };
    assert_eq!(ingame_upper_core_rect(tiny), tiny);
}

#[test]
fn vision_dedup_resolves_two_slot_collision_via_next_best_and_drops_uniform_flood() {
    use crate::vision::dedup_layout_candidates;

    // Radiant slot 2 (Tusk 100) accidentally scores CM(5) #1 at 31.0 and Tusk(100) #2 at 32.5,
    // while Dire slot 0 (CM 5) scores CM(5) #1 tighter at 21.0.
    let mut rad_ranked: [Vec<(HeroId, f32, f32)>; 5] = std::array::from_fn(|_| Vec::new());
    let mut dire_ranked: [Vec<(HeroId, f32, f32)>; 5] = std::array::from_fn(|_| Vec::new());
    rad_ranked[0] = vec![(HeroId(88), 24.0, 28.0)];
    rad_ranked[2] = vec![(HeroId(5), 31.0, 22.0), (HeroId(100), 32.5, 25.0)];
    dire_ranked[0] = vec![(HeroId(5), 21.0, 26.0), (HeroId(64), 29.0, 20.0)];
    // Radiant slot 4 also collides on HeroId(88) with no unallocated fallback -> drops to None.
    rad_ranked[4] = vec![(HeroId(88), 28.0, 22.0)];

    let (rad, dire) = dedup_layout_candidates(&rad_ranked, &dire_ranked);
    assert_eq!(rad[0].map(|c| c.0), Some(HeroId(88)));
    assert_eq!(dire[0].map(|c| c.0), Some(HeroId(5)));
    assert_eq!(
        rad[2].map(|c| c.0),
        Some(HeroId(100)),
        "Colliding slot must fall back to next-best unallocated candidate within threshold"
    );
    assert_eq!(
        rad[4], None,
        "Colliding slot with no unallocated fallback must remain None"
    );

    // Uniform-background flood guard: >= 3 slots sharing top-1 hero must drop all flooded slots.
    let mut flood_rad: [Vec<(HeroId, f32, f32)>; 5] = std::array::from_fn(|_| Vec::new());
    let flood_dire: [Vec<(HeroId, f32, f32)>; 5] = std::array::from_fn(|_| Vec::new());
    flood_rad[0] = vec![(HeroId(61), 25.0, 18.0), (HeroId(14), 28.0, 18.0)];
    flood_rad[1] = vec![(HeroId(61), 26.0, 18.0), (HeroId(74), 29.0, 18.0)];
    flood_rad[2] = vec![(HeroId(61), 27.0, 18.0), (HeroId(88), 30.0, 18.0)];
    flood_rad[3] = vec![(HeroId(48), 22.0, 24.0)];

    let (f_rad, _) = dedup_layout_candidates(&flood_rad, &flood_dire);
    assert_eq!(f_rad[0], None);
    assert_eq!(f_rad[1], None);
    assert_eq!(f_rad[2], None);
    assert_eq!(f_rad[3].map(|c| c.0), Some(HeroId(48)));
}

#[test]
fn vision_layout_selection_scores_unique_top1_before_applying_collision_fallbacks() {
    use crate::vision::{TopBarLayout, select_best_layout_candidates};

    // Layout A (noisy PickPhase): two duplicate top-1 pairs retain 20 and 24 (score 27.0),
    // whereas recovery fallbacks at 23 and 25 would inflate it to 50.0.
    let mut noisy_rad: [Vec<(HeroId, f32, f32)>; 5] = std::array::from_fn(|_| Vec::new());
    let noisy_dire: [Vec<(HeroId, f32, f32)>; 5] = std::array::from_fn(|_| Vec::new());
    noisy_rad[0] = vec![(HeroId(1), 20.0, 20.0), (HeroId(2), 23.0, 20.0)];
    noisy_rad[1] = vec![(HeroId(1), 22.0, 20.0), (HeroId(2), 23.0, 20.0)];
    noisy_rad[2] = vec![(HeroId(3), 24.0, 20.0), (HeroId(4), 25.0, 20.0)];
    noisy_rad[3] = vec![(HeroId(3), 25.0, 20.0), (HeroId(4), 25.0, 20.0)];

    // Layout B (clean InGame): two unique top-1 matches at diff 15.0 (score 41.0 > 27.0).
    let mut clean_rad: [Vec<(HeroId, f32, f32)>; 5] = std::array::from_fn(|_| Vec::new());
    let clean_dire: [Vec<(HeroId, f32, f32)>; 5] = std::array::from_fn(|_| Vec::new());
    clean_rad[0] = vec![(HeroId(88), 15.0, 25.0)];
    clean_rad[1] = vec![(HeroId(48), 15.0, 25.0)];

    let (selected, sel_rad, _) = select_best_layout_candidates(&[
        (
            TopBarLayout::PickPhase,
            noisy_rad.clone(),
            noisy_dire.clone(),
        ),
        (TopBarLayout::InGame, clean_rad, clean_dire),
    ])
    .expect("clean layout must win on unique top-1 evidence");
    assert_eq!(selected, TopBarLayout::InGame);
    assert_eq!(sel_rad[0].map(|c| c.0), Some(HeroId(88)));
    assert_eq!(sel_rad[1].map(|c| c.0), Some(HeroId(48)));

    // When the colliding layout itself wins (no cleaner layout), collision recovery still runs on it.
    let (selected_only, rec_rad, _) =
        select_best_layout_candidates(&[(TopBarLayout::InGame, noisy_rad, noisy_dire)])
            .expect("selected winning layout must apply collision recovery");
    assert_eq!(selected_only, TopBarLayout::InGame);
    assert_eq!(rec_rad[0].map(|c| c.0), Some(HeroId(1)));
    assert_eq!(rec_rad[1].map(|c| c.0), Some(HeroId(2)));
    assert_eq!(rec_rad[2].map(|c| c.0), Some(HeroId(3)));
    assert_eq!(rec_rad[3].map(|c| c.0), Some(HeroId(4)));
}

#[test]
fn windows_steam_path_decoder_rejects_oem_lossy_bytes_and_preserves_utf8_unicode() {
    use crate::platform::{
        build_windows_steam_path_powershell_args, decode_windows_steam_path_utf8,
        parse_windows_reg_steam_path_bytes,
    };
    use crate::vision::resolve_windows_system32_exe;
    use std::path::{Path, PathBuf};

    assert_eq!(
        resolve_windows_system32_exe(
            Some(r"D:\WinOS"),
            None,
            r"WindowsPowerShell\v1.0\powershell.exe"
        ),
        PathBuf::from(r"D:\WinOS")
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe")
    );
    assert_eq!(
        resolve_windows_system32_exe(None, Some(r"E:\Windows"), "reg.exe"),
        PathBuf::from(r"E:\Windows")
            .join("System32")
            .join("reg.exe")
    );

    let args = build_windows_steam_path_powershell_args(r"HKEY_CURRENT_USER\Software\Valve\Steam");
    assert_eq!(args[0], "-NoProfile");
    assert_eq!(args[1], "-NonInteractive");
    assert_eq!(args[2], "-Command");
    assert!(
        args[3].contains("UTF8Encoding")
            && args[3].contains("OutputEncoding")
            && args[3].contains(r"HKEY_CURRENT_USER\Software\Valve\Steam")
            && args[3].contains("SteamPath"),
        "PowerShell SteamPath query must enforce UTF-8 output encoding: {}",
        args[3]
    );

    let custom_args =
        build_windows_steam_path_powershell_args(r"HKEY_CURRENT_USER\Software\TrueSight'sTest");
    assert!(
        custom_args[3].contains(r"Registry::HKEY_CURRENT_USER\Software\TrueSight''sTest"),
        "Parameterized registry key must single-quote escape safely: {}",
        custom_args[3]
    );

    // Valid UTF-8 non-ASCII Steam paths (Latin accents, CJK, quotes/newlines) decode losslessly.
    assert_eq!(
        decode_windows_steam_path_utf8(" \"C:\\Juegos\\Estación\\Steam\" \r\n".as_bytes())
            .as_deref(),
        Some(Path::new(r"C:\Juegos\Estación\Steam"))
    );
    assert_eq!(
        decode_windows_steam_path_utf8("D:\\ゲーム\\Steam".as_bytes()).as_deref(),
        Some(Path::new(r"D:\ゲーム\Steam"))
    );
    assert_eq!(decode_windows_steam_path_utf8(b"  \r\n "), None);

    // Raw OEM/CP1252 byte 0xF3 ('ó' in Estación) must NEVER be lossily turned into U+FFFD.
    let oem_raw_bytes = b"C:\\Juegos\\Estaci\xf3n\\Steam";
    assert_eq!(decode_windows_steam_path_utf8(oem_raw_bytes), None);
    assert_eq!(
        decode_windows_steam_path_utf8("C:\\Juegos\\Estaci\u{FFFD}n\\Steam".as_bytes()),
        None
    );

    let oem_reg_bytes =
        b"\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    SteamPath    REG_SZ    C:\\Juegos\\Estaci\xf3n\\Steam\r\n";
    assert_eq!(parse_windows_reg_steam_path_bytes(oem_reg_bytes), None);

    let utf8_reg_bytes =
        "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    SteamPath    REG_SZ    C:\\Juegos\\Estación\\Steam\r\n"
            .as_bytes();
    assert_eq!(
        parse_windows_reg_steam_path_bytes(utf8_reg_bytes).as_deref(),
        Some(Path::new(r"C:\Juegos\Estación\Steam"))
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_native_powershell_utf8_roundtrip_preserves_non_ascii_steampath() {
    use crate::platform::{configure_background_command, query_windows_steam_path_at};
    use crate::vision::resolve_windows_system32_exe;
    use std::path::Path;
    use std::process::Command;

    struct RegCleanup {
        ps_exe: std::path::PathBuf,
        registry_path: String,
    }
    impl Drop for RegCleanup {
        fn drop(&mut self) {
            let escaped = self.registry_path.replace('\'', "''");
            let script = format!(
                "Remove-Item -LiteralPath '{escaped}' -Recurse -Force -ErrorAction SilentlyContinue"
            );
            let mut cmd = Command::new(&self.ps_exe);
            configure_background_command(&mut cmd);
            let _ = cmd
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .output();
        }
    }

    let sys_root = std::env::var("SystemRoot").ok();
    let windir = std::env::var("WINDIR").ok();
    let ps_exe = resolve_windows_system32_exe(
        sys_root.as_deref(),
        windir.as_deref(),
        r"WindowsPowerShell\v1.0\powershell.exe",
    );

    let full_key = format!(
        r"HKEY_CURRENT_USER\Software\TrueSightDotaTest_{}",
        std::process::id()
    );
    let registry_path = format!(r"Registry::{full_key}");
    let _guard = RegCleanup {
        ps_exe: ps_exe.clone(),
        registry_path: registry_path.clone(),
    };

    let expected_path = "C:\\Juegos\\Estación\\Steam_日本語";
    let setup_script = format!(
        "New-Item -Path '{rp}' -Force | Out-Null; Set-ItemProperty -LiteralPath '{rp}' -Name 'SteamPath' -Value '{val}'",
        rp = registry_path.replace('\'', "''"),
        val = expected_path.replace('\'', "''"),
    );
    let mut setup_cmd = Command::new(&ps_exe);
    configure_background_command(&mut setup_cmd);
    let setup_out = setup_cmd
        .args(["-NoProfile", "-NonInteractive", "-Command", &setup_script])
        .output()
        .expect("system PowerShell must create disposable HKCU test key");
    assert!(
        setup_out.status.success(),
        "failed creating disposable test registry key: {:?}",
        setup_out
    );

    assert_eq!(
        query_windows_steam_path_at(&full_key).as_deref(),
        Some(Path::new(expected_path))
    );
}

#[test]
fn player_rank_tier_parser_and_app_config_preserve_bracket_and_account() {
    use crate::sync::{
        AccountLinkConfig, AppConfig, BracketConfig, LocalCacheManager, load_app_config_from,
        parse_player_rank_tier_json, save_account_config_to, save_bracket_config_to,
    };
    use truesight_core::domain::Bracket;

    // 1. OpenDota /api/players/{id} rank_tier parsing across all 8 medals + uncalibrated/edge inputs
    for (tier, expected) in [
        (11, Bracket::Herald),
        (25, Bracket::Guardian),
        (33, Bracket::Crusader),
        (42, Bracket::Archon),
        (55, Bracket::Legend),
        (63, Bracket::Ancient),
        (74, Bracket::Divine),
        (80, Bracket::Immortal),
    ] {
        let json = format!(r#"{{"solo_competitive_rank":null,"rank_tier":{tier}}}"#);
        assert_eq!(parse_player_rank_tier_json(&json), Some(expected));
    }
    assert_eq!(parse_player_rank_tier_json(r#"{"rank_tier":null}"#), None);
    assert_eq!(parse_player_rank_tier_json(r#"{"rank_tier":0}"#), None);
    assert_eq!(parse_player_rank_tier_json(r#"{"rank_tier":95}"#), None);
    assert_eq!(parse_player_rank_tier_json(r#"{"profile":{"id":1}}"#), None);
    assert_eq!(parse_player_rank_tier_json("not json"), None);

    // 2. Unified config.toml persistence must never overwrite account_id or bracket when saving the other
    let tmp_dir =
        std::env::temp_dir().join(format!("truesight_cfg_bracket_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let cfg_path = tmp_dir.join("config.toml");

    assert_eq!(
        load_app_config_from(&cfg_path),
        AppConfig {
            account_link: AccountLinkConfig::Auto,
            bracket: BracketConfig::Auto,
        }
    );

    save_account_config_to(&cfg_path, AccountLinkConfig::Explicit(127_255_741)).unwrap();
    save_bracket_config_to(&cfg_path, BracketConfig::Explicit(Bracket::Divine)).unwrap();
    assert_eq!(
        load_app_config_from(&cfg_path),
        AppConfig {
            account_link: AccountLinkConfig::Explicit(127_255_741),
            bracket: BracketConfig::Explicit(Bracket::Divine),
        }
    );

    save_account_config_to(&cfg_path, AccountLinkConfig::Unlinked).unwrap();
    assert_eq!(
        load_app_config_from(&cfg_path),
        AppConfig {
            account_link: AccountLinkConfig::Unlinked,
            bracket: BracketConfig::Explicit(Bracket::Divine),
        }
    );

    save_bracket_config_to(&cfg_path, BracketConfig::Auto).unwrap();
    assert_eq!(
        load_app_config_from(&cfg_path),
        AppConfig {
            account_link: AccountLinkConfig::Unlinked,
            bracket: BracketConfig::Auto,
        }
    );

    // 3. Cached player_profile.json lookup via LocalCacheManager
    let mut mgr = LocalCacheManager::with_dir(tmp_dir.join("cache"), 127_255_741);
    assert_eq!(mgr.read_cached_player_bracket(), None);
    mgr.write_cached_profile_json(r#"{"rank_tier":64}"#)
        .unwrap();
    assert_eq!(mgr.read_cached_player_bracket(), Some(Bracket::Ancient));
    mgr.set_account_id(None, true);
    assert_eq!(mgr.read_cached_player_bracket(), None);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn vision_rejects_main_menu_chrome_and_weak_uncorrelated_patches() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, TopBarSlotRect, VisionCaptureAdapter,
        VisionCaptureMode, compute_ingame_topbar_slot_rects, parse_hero_atlas,
    };
    use std::collections::VecDeque;
    use truesight_core::domain::{Faction, Side};
    use truesight_core::ports::DraftEvent;

    struct QueueSource(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.pop_front())
        }
    }

    // 1. Deterministic 2560x115 synthetic Dota 2 main-menu navigation bar replicating the measured
    // slanted glowing tab geometry (InGame Rad[0..2] over the illuminated ARMORY tab and seam into WATCH).
    let (mw, mh) = (2560_usize, 115_usize);
    let mut menu_rgb = vec![16_u8; mw * mh * 3];
    for y in 0..mh {
        let vy = (y as f32 / 46.0).clamp(0.0, 1.0);
        for x in 0..mw {
            let idx = (y * mw + x) * 3;
            let slant_x = 918.0 - (y as f32) * 0.25;
            let xf = x as f32;
            let px = if (760.0..=965.0).contains(&xf) && y < 60 {
                let horiz = if xf < slant_x {
                    let t = ((xf - 760.0) / (slant_x - 760.0)).clamp(0.0, 1.0);
                    (1.0 - (t - 0.55).abs() * 0.65).clamp(0.35, 1.0)
                } else {
                    let drop = ((xf - slant_x) / 42.0).clamp(0.0, 1.0);
                    (0.55 * (1.0 - drop)).clamp(0.05, 0.55)
                };
                let lit = vy * horiz;
                let text_band: f32 =
                    if (28..=38).contains(&y) && (805..=895).contains(&x) && (x % 9) < 5 {
                        38.0
                    } else {
                        0.0
                    };
                [
                    (30.0 + 128.0 * lit + text_band).min(255.0),
                    (40.0 + 140.0 * lit + text_band).min(255.0),
                    (55.0 + 150.0 * lit + text_band).min(255.0),
                ]
            } else {
                [18.0, 22.0, 30.0]
            };
            menu_rgb[idx] = px[0] as u8;
            menu_rgb[idx + 1] = px[1] as u8;
            menu_rgb[idx + 2] = px[2] as u8;
        }
    }

    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    for mode in [VisionCaptureMode::Any, VisionCaptureMode::DraftOnly] {
        let mut adapter = VisionCaptureAdapter::new(
            QueueSource(VecDeque::from([
                RgbFrame::new(mw, mh, menu_rgb.clone()),
                RgbFrame::new(mw, mh, menu_rgb.clone()),
                RgbFrame::new(mw, mh, menu_rgb.clone()),
            ])),
            catalog.clone(),
        );
        adapter.set_ally_faction(Faction::Radiant);
        adapter.set_capture_mode(mode);
        for poll in 0..3 {
            let events = adapter.poll_events().expect("poll must succeed");
            assert!(
                events.is_empty(),
                "Synthetic main-menu chrome must emit 0 phantom picks under {mode:?} (poll {poll}), got {events:?}"
            );
        }
    }

    // 2. Weak/marginal initial match (diff > 18.0 but high NCC): must NOT emit on a single transient
    // flash, must reset pending state across an empty frame, and must confirm on 2 consecutive frames.
    let atlas = parse_hero_atlas().expect("base atlas must load");
    let (w, h) = (1920_usize, 1080_usize);
    let (rad_ig, dire_ig) = compute_ingame_topbar_slot_rects(w, h);
    let mut shifted_frame = vec![16_u8; w * h * 3];
    let paint_shifted_ingame = |buf: &mut [u8], rect: TopBarSlotRect, hid: u16| {
        let hero_rgb = &atlas.heroes[&HeroId(hid)];
        let (sx0, sw0, sy0, sh0) = (
            4_usize,
            atlas.hero_width - 8,
            2_usize,
            atlas.hero_height - 4,
        );
        for dy in 0..rect.height {
            let sy = sy0 + (dy * sh0) / rect.height;
            for dx in 0..rect.width {
                let sx = sx0 + (dx * sw0) / rect.width;
                let si = (sy * atlas.hero_width + sx) * 3;
                let di = ((rect.y + dy) * w + (rect.x + dx)) * 3;
                for c in 0..3 {
                    // Positive affine brightness/contrast shift keeps Pearson NCC ~1.0 while pushing L1 > 18.0
                    buf[di + c] =
                        (f32::from(hero_rgb[si + c]) * 0.80 + 38.0).clamp(0.0, 255.0) as u8;
                }
            }
        }
    };
    paint_shifted_ingame(&mut shifted_frame, rad_ig[0], 2); // Axe
    paint_shifted_ingame(&mut shifted_frame, dire_ig[0], 48); // Luna

    let blank_frame = vec![16_u8; w * h * 3];
    let mut flash_adapter = VisionCaptureAdapter::new(
        QueueSource(VecDeque::from([
            RgbFrame::new(w, h, shifted_frame.clone()), // frame 0: single flash (pending=1)
            RgbFrame::new(w, h, blank_frame),           // frame 1: blank (must clear pending)
            RgbFrame::new(w, h, shifted_frame.clone()), // frame 2: first consecutive (pending=1)
            RgbFrame::new(w, h, shifted_frame),         // frame 3: second consecutive -> confirmed
        ])),
        catalog,
    );
    flash_adapter.set_ally_faction(Faction::Radiant);
    flash_adapter.set_capture_mode(VisionCaptureMode::Any);

    let ev0 = flash_adapter.poll_events().unwrap();
    assert!(
        ev0.is_empty(),
        "Single-frame weak initial match (diff > 18.0) must not emit on flash frame 0: {ev0:?}"
    );
    let ev1 = flash_adapter.poll_events().unwrap();
    assert!(ev1.is_empty(), "Blank frame must emit 0 picks: {ev1:?}");
    let ev2 = flash_adapter.poll_events().unwrap();
    assert!(
        ev2.is_empty(),
        "Non-consecutive flash after blank frame must restart 2-frame confirmation: {ev2:?}"
    );
    let ev3 = flash_adapter.poll_events().unwrap();
    assert_eq!(
        ev3.len(),
        2,
        "Two consecutive frames of marginal match must confirm both picks: {ev3:?}"
    );
    assert!(ev3.contains(&DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 0,
        hero_id: HeroId(2),
        role: Some(Role::Carry),
    }));
    assert!(ev3.contains(&DraftEvent::PickDetected {
        side: Side::Enemies,
        slot: 0,
        hero_id: HeroId(48),
        role: Some(Role::Carry),
    }));
}

#[test]
fn vision_capture_error_clears_pending_confirmations_while_preserving_confirmed_picks() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, TopBarSlotRect, VisionCaptureAdapter,
        VisionCaptureMode, compute_ingame_topbar_slot_rects, parse_hero_atlas,
    };
    use std::collections::VecDeque;
    use truesight_core::domain::{Faction, HeroId, Role, Side};
    use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

    struct ScriptedSource(VecDeque<Result<Option<RgbFrame>, PortError>>);
    impl ScreenFrameSource for ScriptedSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            self.0
                .pop_front()
                .expect("scripted frame queue must cover all test polls")
        }
    }

    let atlas = parse_hero_atlas().expect("base atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    let (w, h) = (1920_usize, 1080_usize);
    let (rad_ig, dire_ig) = compute_ingame_topbar_slot_rects(w, h);

    let paint_ingame = |buf: &mut [u8], rect: TopBarSlotRect, hid: u16, marginal: bool| {
        let hero_rgb = &atlas.heroes[&HeroId(hid)];
        let (sx0, sw0, sy0, sh0) = if marginal {
            (
                4_usize,
                atlas.hero_width - 8,
                2_usize,
                atlas.hero_height - 4,
            )
        } else {
            (0_usize, atlas.hero_width, 0_usize, atlas.hero_height)
        };
        for dy in 0..rect.height {
            let sy = sy0 + (dy * sh0) / rect.height;
            for dx in 0..rect.width {
                let sx = sx0 + (dx * sw0) / rect.width;
                let si = (sy * atlas.hero_width + sx) * 3;
                let di = ((rect.y + dy) * w + (rect.x + dx)) * 3;
                for c in 0..3 {
                    buf[di + c] = if marginal {
                        (f32::from(hero_rgb[si + c]) * 0.80 + 38.0).clamp(0.0, 255.0) as u8
                    } else {
                        hero_rgb[si + c]
                    };
                }
            }
        }
    };

    let mut strong_buf = vec![16_u8; w * h * 3];
    paint_ingame(&mut strong_buf, rad_ig[0], 1, false); // Anti-Mage (strong exact match)
    paint_ingame(&mut strong_buf, dire_ig[0], 7, false); // Earthshaker (strong exact match)
    let strong_frame = RgbFrame::new(w, h, strong_buf.clone());

    let mut weak_plus_strong_buf = strong_buf;
    paint_ingame(&mut weak_plus_strong_buf, rad_ig[1], 2, true); // Axe (marginal diff > 18.0)
    let weak_plus_strong_frame = RgbFrame::new(w, h, weak_plus_strong_buf);

    let mut adapter = VisionCaptureAdapter::new(
        ScriptedSource(VecDeque::from([
            Ok(Some(strong_frame)),
            Ok(Some(weak_plus_strong_frame.clone())),
            Err(PortError("transient capture failure".into())),
            Ok(Some(weak_plus_strong_frame.clone())),
            Ok(Some(weak_plus_strong_frame)),
        ])),
        catalog,
    );
    adapter.set_ally_faction(Faction::Radiant);
    adapter.set_capture_mode(VisionCaptureMode::Any);

    // Poll 0: strong confirmed picks emit immediately.
    let ev0 = adapter.poll_events().expect("poll 0 must succeed");
    assert_eq!(
        ev0.len(),
        2,
        "Confirmed strong picks must emit on poll 0: {ev0:?}"
    );
    assert!(ev0.contains(&DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 0,
        hero_id: HeroId(1),
        role: Some(Role::Carry),
    }));
    assert!(ev0.contains(&DraftEvent::PickDetected {
        side: Side::Enemies,
        slot: 0,
        hero_id: HeroId(7),
        role: Some(Role::Carry),
    }));

    // Poll 1: weak match on slot 1 waits in pending (count = 1).
    let ev1 = adapter.poll_events().expect("poll 1 must succeed");
    assert!(
        ev1.is_empty(),
        "First marginal frame must not emit slot 1 immediately: {ev1:?}"
    );

    // Poll 2: transient capture error must surface Err and break the confirmation streak.
    assert!(
        adapter.poll_events().is_err(),
        "Scripted capture failure must return Err"
    );

    // Poll 3: weak match after Err must restart streak (count = 1), preserving slot 0 without re-emit.
    let ev3 = adapter.poll_events().expect("poll 3 must succeed");
    assert!(
        ev3.is_empty(),
        "Post-error weak match must not confirm without 2 consecutive frames, nor re-emit confirmed picks: {ev3:?}"
    );

    // Poll 4: second consecutive weak match confirms only Allies slot 1 (Axe).
    let ev4 = adapter.poll_events().expect("poll 4 must succeed");
    assert_eq!(
        ev4,
        vec![DraftEvent::PickDetected {
            side: Side::Allies,
            slot: 1,
            hero_id: HeroId(2),
            role: Some(Role::Mid),
        }],
        "Only the newly confirmed slot 1 pick should emit after two consecutive post-error frames"
    );
}

#[test]
fn vision_x11_checks_topbar_geometry_occlusion_on_first_and_cached_captures() {
    use crate::vision::{
        X11WindowGeometry, evaluate_x11_window_visibility_with, is_dota2_xprop_output,
        resolve_window_geometry_with, select_visible_dota_window_with, top_bar_intersects_window,
    };

    let win = |x, y, width, height| X11WindowGeometry {
        x,
        y,
        width,
        height,
        viewable: true,
    };

    // 1. is_dota2_xprop_output rejects ICCCM Iconic / Withdrawn states while keeping Normal & omitted WM_STATE.
    assert!(!is_dota2_xprop_output(
        "WM_CLASS(STRING) = \"dota2\", \"dota2\"\nWM_STATE(WM_STATE):\n\t\twindow state: Iconic\n\t\ticon window: 0x0\n"
    ));
    assert!(!is_dota2_xprop_output(
        "WM_CLASS(STRING) = \"dota2\", \"dota2\"\nWM_STATE(WM_STATE):\n\t\twindow state: Withdrawn\n\t\ticon window: 0x0\n"
    ));
    assert!(is_dota2_xprop_output(
        "WM_CLASS(STRING) = \"dota2\", \"dota2\"\nWM_STATE(WM_STATE):\n\t\twindow state: Normal\n\t\ticon window: 0x0\n_NET_WM_STATE(ATOM) = _NET_WM_STATE_FULLSCREEN\n"
    ));

    let root_windowed = "_NET_CURRENT_DESKTOP(CARDINAL) = 0\n_NET_CLIENT_LIST_STACKING(WINDOW): window id # 0x5a00004, 0x0660013d, 0x7200010\n";
    let dota_visible = "WM_CLASS(STRING) = \"dota2\", \"dota2\"\nWM_STATE(WM_STATE):\n\t\twindow state: Normal\n_NET_WM_STATE(ATOM) = _NET_WM_STATE_FULLSCREEN\n_NET_WM_DESKTOP(CARDINAL) = 0\n";
    let dota_windowed_geom = win(100, 100, 1280, 720);

    // 2. Hidden / Iconic / Unmapped Dota 2 window returns invisible (false).
    let dota_hidden = "WM_CLASS(STRING) = \"dota2\", \"dota2\"\n_NET_WM_STATE(ATOM) = _NET_WM_STATE_HIDDEN\n_NET_WM_DESKTOP(CARDINAL) = 0\n";
    assert!(!evaluate_x11_window_visibility_with(
        "0x660013d",
        root_windowed,
        dota_hidden,
        |_| None,
        |_| Some(dota_windowed_geom),
    ));

    let dota_iconic = "WM_CLASS(STRING) = \"dota2\", \"dota2\"\nWM_STATE(WM_STATE):\n\t\twindow state: Iconic\n_NET_WM_DESKTOP(CARDINAL) = 0\n";
    assert!(!evaluate_x11_window_visibility_with(
        "0x660013d",
        root_windowed,
        dota_iconic,
        |_| None,
        |_| Some(dota_windowed_geom),
    ));

    assert!(!evaluate_x11_window_visibility_with(
        "0x660013d",
        root_windowed,
        dota_visible,
        |_| None,
        |wid| {
            (wid == "0x660013d").then_some(X11WindowGeometry {
                viewable: false,
                ..dota_windowed_geom
            })
        },
    ));

    // 3. Desktop mismatch hides cached window; sticky (0xFFFFFFFF / 4294967295) stays visible.
    let root_clean_stack = "_NET_CURRENT_DESKTOP(CARDINAL) = 0\n_NET_CLIENT_LIST_STACKING(WINDOW): window id # 0x5a00004, 0x660013d\n";
    let dota_other_desktop =
        "WM_CLASS(STRING) = \"dota2\", \"dota2\"\n_NET_WM_DESKTOP(CARDINAL) = 1\n";
    assert!(!evaluate_x11_window_visibility_with(
        "0x660013d",
        root_clean_stack,
        dota_other_desktop,
        |_| None,
        |_| Some(dota_windowed_geom),
    ));
    let dota_sticky =
        "WM_CLASS(STRING) = \"dota2\", \"dota2\"\n_NET_WM_DESKTOP(CARDINAL) = 4294967295\n";
    assert!(evaluate_x11_window_visibility_with(
        "0x660013d",
        root_clean_stack,
        dota_sticky,
        |_| None,
        |_| Some(dota_windowed_geom),
    ));

    // 4. First capture in windowed mode (1280x720 at 100,100 on 1920px display):
    // top bar spans [100, 1380) x [100, 158). Must NOT skip stacking checks when desk_w > fw.
    let root_windowed_with_geom = "_NET_CURRENT_DESKTOP(CARDINAL) = 0\n_NET_DESKTOP_GEOMETRY(CARDINAL) = 1920, 1080\n_NET_CLIENT_LIST_STACKING(WINDOW): window id # 0x5a00004, 0x0660013d, 0x7200010\n";
    let firefox_visible = "WM_CLASS(STRING) = \"Navigator\", \"firefox\"\nWM_STATE(WM_STATE):\n\t\twindow state: Normal\n_NET_WM_DESKTOP(CARDINAL) = 0\n";
    let candidates = vec!["0x7200010".to_string(), "0x0660013d".to_string()];

    // Higher-stacked firefox overlapping top bar blocks first discovery capture!
    assert_eq!(
        select_visible_dota_window_with(
            root_windowed_with_geom,
            &candidates,
            |wid| match wid {
                "0x0660013d" => Some(dota_visible.to_string()),
                "0x7200010" => Some(firefox_visible.to_string()),
                _ => None,
            },
            |wid| match wid {
                "0x0660013d" => Some(dota_windowed_geom),
                "0x7200010" => Some(win(150, 80, 500, 400)),
                _ => None,
            },
        ),
        None
    );

    // Higher-stacked firefox positioned below Dota's top bar (y=260 > 158) allows first capture!
    assert_eq!(
        select_visible_dota_window_with(
            root_windowed_with_geom,
            &candidates,
            |wid| match wid {
                "0x0660013d" => Some(dota_visible.to_string()),
                "0x7200010" => Some(firefox_visible.to_string()),
                _ => None,
            },
            |wid| match wid {
                "0x0660013d" => Some(dota_windowed_geom),
                "0x7200010" => Some(win(150, 260, 500, 400)),
                _ => None,
            },
        ),
        Some("0x0660013d".to_string())
    );

    // 5. Cached revalidation in windowed mode: covered directly AND covered only via WM titlebar (_NET_FRAME_EXTENTS).
    assert!(!evaluate_x11_window_visibility_with(
        "0x660013d",
        root_windowed_with_geom,
        dota_visible,
        |wid| (wid == "0x7200010").then(|| firefox_visible.to_string()),
        |wid| match wid {
            "0x660013d" => Some(dota_windowed_geom),
            "0x7200010" => Some(win(150, 80, 500, 400)),
            _ => None,
        },
    ));

    let firefox_with_titlebar = "WM_CLASS(STRING) = \"Navigator\", \"firefox\"\nWM_STATE(WM_STATE):\n\t\twindow state: Normal\n_NET_WM_DESKTOP(CARDINAL) = 0\n_NET_FRAME_EXTENTS(CARDINAL) = 0, 0, 32, 0\n";
    // Client rect y=162 is below top bar [100, 158), but 32px top frame extent pulls it to y=130..162.
    assert!(top_bar_intersects_window(
        dota_windowed_geom,
        win(150, 162, 500, 400),
        [0, 0, 32, 0]
    ));
    assert!(!evaluate_x11_window_visibility_with(
        "0x660013d",
        root_windowed_with_geom,
        dota_visible,
        |wid| (wid == "0x7200010").then(|| firefox_with_titlebar.to_string()),
        |wid| match wid {
            "0x660013d" => Some(dota_windowed_geom),
            "0x7200010" => Some(win(150, 162, 500, 400)),
            _ => None,
        },
    ));

    // 6. Non-overlapping visible window on another monitor stays visible on BOTH first & cached captures.
    let root_multi = "_NET_CURRENT_DESKTOP(CARDINAL) = 0\n_NET_DESKTOP_GEOMETRY(CARDINAL) = 4000, 2560\n_NET_CLIENT_LIST_STACKING(WINDOW): window id # 0x5a00004, 0x0660013d, 0x7200010\n";
    let dota_mon2 = win(1440, 560, 2560, 1440);
    let firefox_mon1 = win(0, 0, 1440, 1080);
    assert!(!top_bar_intersects_window(
        dota_mon2,
        firefox_mon1,
        [0, 0, 0, 0]
    ));
    assert!(evaluate_x11_window_visibility_with(
        "0x660013d",
        root_multi,
        dota_visible,
        |wid| (wid == "0x7200010").then(|| firefox_visible.to_string()),
        |wid| match wid {
            "0x660013d" => Some(dota_mon2),
            "0x7200010" => Some(firefox_mon1),
            _ => None,
        },
    ));
    assert_eq!(
        select_visible_dota_window_with(
            root_multi,
            &candidates,
            |wid| match wid {
                "0x0660013d" => Some(dota_visible.to_string()),
                "0x7200010" => Some(firefox_visible.to_string()),
                _ => None,
            },
            |wid| match wid {
                "0x0660013d" => Some(dota_mon2),
                "0x7200010" => Some(firefox_mon1),
                _ => None,
            },
        ),
        Some("0x0660013d".to_string())
    );

    // 7. Occlusion exceptions even when geometry overlaps top bar:
    // truesight-dota overlay, hidden windows, iconic windows, unmapped windows, or other-desktop windows.
    let truesight_win = "WM_CLASS(STRING) = \"truesight-dota\", \"truesight-dota\"\nWM_STATE(WM_STATE):\n\t\twindow state: Normal\n_NET_WM_DESKTOP(CARDINAL) = 0\n";
    let firefox_hidden = "WM_CLASS(STRING) = \"Navigator\", \"firefox\"\n_NET_WM_STATE(ATOM) = _NET_WM_STATE_HIDDEN\n_NET_WM_DESKTOP(CARDINAL) = 0\n";
    let firefox_other_desk = "WM_CLASS(STRING) = \"Navigator\", \"firefox\"\nWM_STATE(WM_STATE):\n\t\twindow state: Normal\n_NET_WM_DESKTOP(CARDINAL) = 2\n";
    for (label, props, geom) in [
        ("truesight-dota", truesight_win, win(100, 80, 500, 400)),
        ("hidden", firefox_hidden, win(100, 80, 500, 400)),
        ("other-desktop", firefox_other_desk, win(100, 80, 500, 400)),
        (
            "unmapped",
            firefox_visible,
            X11WindowGeometry {
                viewable: false,
                ..win(100, 80, 500, 400)
            },
        ),
    ] {
        assert!(
            evaluate_x11_window_visibility_with(
                "0x660013d",
                root_windowed_with_geom,
                dota_visible,
                |wid| (wid == "0x7200010").then(|| props.to_string()),
                |wid| match wid {
                    "0x660013d" => Some(dota_windowed_geom),
                    "0x7200010" => Some(geom),
                    _ => None,
                },
            ),
            "Non-occluding case {label} must keep capture visible"
        );
    }

    // 8. Per-query geometry fallback + fail-closed policy when occlusion is unverifiable:
    let mut fallback_hits = 0_usize;
    let resolved_primary = resolve_window_geometry_with(
        "0x660013d",
        |_| Some(dota_windowed_geom),
        |_| {
            fallback_hits += 1;
            Some(win(0, 0, 100, 100))
        },
    );
    assert_eq!(resolved_primary, Some(dota_windowed_geom));
    assert_eq!(
        fallback_hits, 0,
        "Fallback must not run when primary succeeds"
    );

    // Primary returns None -> fallback rescues non-overlapping window on monitor 1 (capture allowed).
    assert!(evaluate_x11_window_visibility_with(
        "0x660013d",
        root_multi,
        dota_visible,
        |wid| (wid == "0x7200010").then(|| firefox_visible.to_string()),
        |wid| {
            resolve_window_geometry_with(
                wid,
                |id| (id == "0x0660013d" || id == "0x660013d").then_some(dota_mon2),
                |id| (id == "0x7200010").then_some(firefox_mon1),
            )
        },
    ));
    // Primary returns None -> fallback detects top-bar overlap (capture blocked).
    assert!(!evaluate_x11_window_visibility_with(
        "0x660013d",
        root_windowed_with_geom,
        dota_visible,
        |wid| (wid == "0x7200010").then(|| firefox_visible.to_string()),
        |wid| {
            resolve_window_geometry_with(
                wid,
                |id| (id == "0x0660013d" || id == "0x660013d").then_some(dota_windowed_geom),
                |id| (id == "0x7200010").then_some(win(150, 80, 500, 400)),
            )
        },
    ));

    // When visible non-truesight same-desktop window is stacked above Dota 2 and geometry fails:
    // fail closed (both dota_geom = None and other_geom = None).
    assert!(!evaluate_x11_window_visibility_with(
        "0x660013d",
        root_windowed_with_geom,
        dota_visible,
        |wid| (wid == "0x7200010").then(|| firefox_visible.to_string()),
        |_| None,
    ));
    assert_eq!(
        select_visible_dota_window_with(
            root_windowed_with_geom,
            &candidates,
            |wid| match wid {
                "0x0660013d" => Some(dota_visible.to_string()),
                "0x7200010" => Some(firefox_visible.to_string()),
                _ => None,
            },
            |_| None,
        ),
        None
    );
    assert!(!evaluate_x11_window_visibility_with(
        "0x660013d",
        root_windowed_with_geom,
        dota_visible,
        |wid| (wid == "0x7200010").then(|| firefox_visible.to_string()),
        |wid| (wid == "0x0660013d" || wid == "0x660013d").then_some(dota_windowed_geom),
    ));
    assert_eq!(
        select_visible_dota_window_with(
            root_windowed_with_geom,
            &candidates,
            |wid| match wid {
                "0x0660013d" => Some(dota_visible.to_string()),
                "0x7200010" => Some(firefox_visible.to_string()),
                _ => None,
            },
            |wid| (wid == "0x0660013d" || wid == "0x660013d").then_some(dota_windowed_geom),
        ),
        None
    );

    // Without any visible non-truesight same-desktop window stacked above Dota 2, missing geometry stays open.
    assert!(evaluate_x11_window_visibility_with(
        "0x660013d",
        root_clean_stack,
        dota_visible,
        |_| None,
        |_| None,
    ));
    assert_eq!(
        select_visible_dota_window_with(
            root_clean_stack,
            &["0x0660013d".to_string()],
            |wid| (wid == "0x0660013d").then(|| dota_visible.to_string()),
            |_| None,
        ),
        Some("0x0660013d".to_string())
    );
    for (label, above_props) in [
        ("truesight-above", truesight_win),
        ("hidden-above", firefox_hidden),
        ("other-desktop-above", firefox_other_desk),
    ] {
        assert!(
            evaluate_x11_window_visibility_with(
                "0x660013d",
                root_windowed_with_geom,
                dota_visible,
                |wid| (wid == "0x7200010").then(|| above_props.to_string()),
                |_| None,
            ),
            "No visible non-truesight occluder above ({label}) must remain visible when geometry is None"
        );
    }
}

#[test]
fn vision_x11_wire_geometry_and_parsers() {
    use crate::vision::{
        X11LocalDisplay, X11WindowGeometry, parse_x11_local_display, parse_xauth_cookie_for_local,
        parse_xprop_frame_extents, parse_xwininfo_geometry,
    };

    assert_eq!(
        parse_x11_local_display(":0"),
        Some(X11LocalDisplay {
            path: "/tmp/.X11-unix/X0".to_string(),
            display_num: "0".to_string(),
        })
    );
    assert_eq!(
        parse_x11_local_display(":0.0"),
        Some(X11LocalDisplay {
            path: "/tmp/.X11-unix/X0".to_string(),
            display_num: "0".to_string(),
        })
    );
    assert_eq!(
        parse_x11_local_display("unix:1.0"),
        Some(X11LocalDisplay {
            path: "/tmp/.X11-unix/X1".to_string(),
            display_num: "1".to_string(),
        })
    );
    for remote in ["127.0.0.1:10.2", "localhost:0", "192.168.1.10:0", "[::1]:0"] {
        assert_eq!(
            parse_x11_local_display(remote),
            None,
            "TCP display {remote} must be rejected by local X11 wire client"
        );
    }

    assert_eq!(
        parse_xprop_frame_extents("_NET_FRAME_EXTENTS(CARDINAL) = 4, 4, 32, 4\n"),
        [4, 4, 32, 4]
    );
    assert_eq!(
        parse_xprop_frame_extents("_NET_FRAME_EXTENTS:  not found.\n"),
        [0, 0, 0, 0]
    );

    assert_eq!(
        parse_xwininfo_geometry(
            "xwininfo: Window id: 0x660013d \"Dota 2\"\n  Absolute upper-left X:  1440\n  Absolute upper-left Y:  560\n  Width: 2560\n  Height: 1440\n  Map State: IsViewable\n"
        ),
        Some(X11WindowGeometry {
            x: 1440,
            y: 560,
            width: 2560,
            height: 1440,
            viewable: true,
        })
    );

    // Big-endian Xauthority endpoint binding tests:
    // Reject TCP FamilyInternet (0), FamilyInternet6 (6), wrong local host, wrong display, or missing local hostname.
    let push_entry =
        |buf: &mut Vec<u8>, fam: u16, addr: &[u8], num: &[u8], name: &[u8], data: &[u8]| {
            buf.extend_from_slice(&fam.to_be_bytes());
            for slice in [addr, num, name, data] {
                buf.extend_from_slice(&(slice.len() as u16).to_be_bytes());
                buf.extend_from_slice(slice);
            }
        };

    let mut unrelated = Vec::new();
    push_entry(
        &mut unrelated,
        0, // FamilyInternet
        &[127, 0, 0, 1],
        b"0",
        b"MIT-MAGIC-COOKIE-1",
        &[9; 16],
    );
    push_entry(
        &mut unrelated,
        6, // FamilyInternet6
        &[0; 16],
        b"0",
        b"MIT-MAGIC-COOKIE-1",
        &[8; 16],
    );
    push_entry(
        &mut unrelated,
        256, // FamilyLocal other host
        b"otherhost",
        b"0",
        b"MIT-MAGIC-COOKIE-1",
        &[1; 16],
    );
    push_entry(
        &mut unrelated,
        256, // FamilyLocal right host, wrong display
        b"master",
        b"1",
        b"MIT-MAGIC-COOKIE-1",
        &[2; 16],
    );
    assert_eq!(
        parse_xauth_cookie_for_local(&unrelated, "0", Some("master")),
        None,
        "Unrelated FamilyInternet/otherhost/wrong-display entries must not fall back"
    );
    assert_eq!(
        parse_xauth_cookie_for_local(&unrelated, "1", None),
        None,
        "FamilyLocal entry must not match when local hostname is unknown"
    );

    let mut matched_local = unrelated.clone();
    push_entry(
        &mut matched_local,
        256,
        b"master",
        b"0",
        b"MIT-MAGIC-COOKIE-1",
        &[7; 16],
    );
    assert_eq!(
        parse_xauth_cookie_for_local(&matched_local, "0", Some("master")),
        Some((b"MIT-MAGIC-COOKIE-1".to_vec(), vec![7; 16]))
    );

    let mut wild_only = unrelated;
    push_entry(
        &mut wild_only,
        65535, // FamilyWild
        b"",
        b"0",
        b"MIT-MAGIC-COOKIE-1",
        &[5; 16],
    );
    assert_eq!(
        parse_xauth_cookie_for_local(&wild_only, "0", None),
        Some((b"MIT-MAGIC-COOKIE-1".to_vec(), vec![5; 16]))
    );
    assert_eq!(
        parse_xauth_cookie_for_local(&[0, 1, 0, 5], "0", Some("master")),
        None
    );

    #[cfg(target_os = "linux")]
    {
        use crate::vision::X11GeometryClient;
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        use std::thread;

        let (client_sock, mut server_sock) = UnixStream::pair().expect("UnixStream pair must work");
        let server = thread::spawn(move || {
            let mut setup_hdr = [0_u8; 12];
            server_sock.read_exact(&mut setup_hdr).unwrap();
            assert_eq!(setup_hdr[0], b'l');
            let nlen = u16::from_le_bytes([setup_hdr[6], setup_hdr[7]]) as usize;
            let dlen = u16::from_le_bytes([setup_hdr[8], setup_hdr[9]]) as usize;
            let pad = |n: usize| (n + 3) & !3;
            let mut auth_buf = vec![0_u8; pad(nlen) + pad(dlen)];
            server_sock.read_exact(&mut auth_buf).unwrap();
            assert_eq!(&auth_buf[..nlen], b"MIT-MAGIC-COOKIE-1");

            // Setup success reply (8 bytes header + 0 additional words).
            server_sock.write_all(&[1, 0, 11, 0, 0, 0, 0, 0]).unwrap();

            // Req 1: GetGeometry -> inject 32-byte MappingNotify event (type 34) first, then reply.
            let mut req1 = [0_u8; 8];
            server_sock.read_exact(&mut req1).unwrap();
            assert_eq!(req1[0], 14);
            let mut event = [0_u8; 32];
            event[0] = 34;
            server_sock.write_all(&event).unwrap();

            let mut r1 = [0_u8; 32];
            r1[0] = 1;
            r1[2..4].copy_from_slice(&1_u16.to_le_bytes());
            r1[8..12].copy_from_slice(&0x39c_u32.to_le_bytes());
            r1[16..18].copy_from_slice(&1280_u16.to_le_bytes());
            r1[18..20].copy_from_slice(&720_u16.to_le_bytes());
            server_sock.write_all(&r1).unwrap();

            // Req 2: TranslateCoordinates
            let mut req2 = [0_u8; 16];
            server_sock.read_exact(&mut req2).unwrap();
            assert_eq!(req2[0], 40);
            let mut r2 = [0_u8; 32];
            r2[0] = 1;
            r2[1] = 1; // same_screen
            r2[2..4].copy_from_slice(&2_u16.to_le_bytes());
            r2[12..14].copy_from_slice(&(-120_i16).to_le_bytes());
            r2[14..16].copy_from_slice(&45_i16.to_le_bytes());
            server_sock.write_all(&r2).unwrap();

            // Req 3: GetWindowAttributes (3 extra 4-byte words = 12 bytes)
            let mut req3 = [0_u8; 8];
            server_sock.read_exact(&mut req3).unwrap();
            assert_eq!(req3[0], 3);
            let mut r3 = [0_u8; 44];
            r3[0] = 1;
            r3[2..4].copy_from_slice(&3_u16.to_le_bytes());
            r3[4..8].copy_from_slice(&3_u32.to_le_bytes());
            r3[26] = 2; // IsViewable
            server_sock.write_all(&r3).unwrap();

            // Second window query receives X11 error packet (type 0) -> None.
            let mut req4 = [0_u8; 8];
            server_sock.read_exact(&mut req4).unwrap();
            let mut err = [0_u8; 32];
            err[0] = 0;
            err[1] = 3; // BadWindow
            err[2..4].copy_from_slice(&4_u16.to_le_bytes());
            server_sock.write_all(&err).unwrap();
        });

        let mut client =
            X11GeometryClient::connect_stream(client_sock, b"MIT-MAGIC-COOKIE-1", &[7; 16])
                .expect("handshake over UnixStream must succeed");
        assert_eq!(
            client.query_window_geometry(0x660013d),
            Some(X11WindowGeometry {
                x: -120,
                y: 45,
                width: 1280,
                height: 720,
                viewable: true,
            })
        );
        assert_eq!(client.query_window_geometry(0xdead), None);
        server.join().unwrap();
    }
}

#[test]
fn stratz_matchup_graphql_payload_and_user_agent_header_match_api_contract() {
    use crate::platform::build_curl_args;
    use crate::sync::{STRATZ_USER_AGENT_HEADER, stratz_matchup_graphql_payload};

    assert_eq!(STRATZ_USER_AGENT_HEADER, "User-Agent: STRATZ_API");

    let payload = stratz_matchup_graphql_payload(&[1, 2, 14]);
    assert!(
        payload.contains("heroStats { matchUp(heroIds: [1,2,14], take: 200) { heroId with { heroId1 heroId2 matchCount synergy winCount } } }"),
        "unexpected STRATZ GraphQL payload: {payload}"
    );
    assert!(
        !payload.contains("heroVsHeroMatchup"),
        "broken heroVsHeroMatchup query must not be used"
    );

    let extra_args = [
        "-H",
        "Content-Type: application/json",
        "-H",
        STRATZ_USER_AGENT_HEADER,
        "-H",
        "Authorization: Bearer test-token",
        "-d",
        &payload,
    ];
    let args = build_curl_args("https://api.stratz.com/graphql", &extra_args);
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-H", "User-Agent: STRATZ_API"]),
        "STRATZ curl args must include -H User-Agent: STRATZ_API: {args:?}"
    );
}

#[test]
fn parse_stratz_with_synergy_json_scales_percentage_points_and_preserves_ratio_compat() {
    use crate::sync::parse_stratz_with_synergy_json;

    let stratz_response = r#"{"data":{"heroStats":{"matchUp":[{"heroId":1,"with":[{"heroId1":1,"heroId2":57,"matchCount":3444,"synergy":2.445,"winCount":1859},{"heroId1":1,"heroId2":112,"matchCount":10226,"synergy":-3.12,"winCount":4800},{"heroId1":1,"heroId2":99,"matchCount":500,"synergy":18.5,"winCount":350},{"heroId1":1,"heroId2":14,"matchCount":450,"winCount":270}]}]}}}"#;
    let parsed = parse_stratz_with_synergy_json(stratz_response);
    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed[0].0, HeroId(1));
    assert_eq!(parsed[0].1, HeroId(57));
    assert!((parsed[0].2 - 0.02445).abs() < 1e-5, "got {}", parsed[0].2);
    assert_eq!(parsed[0].3, 3444);

    assert_eq!(parsed[1].0, HeroId(1));
    assert_eq!(parsed[1].1, HeroId(112));
    assert!(
        (parsed[1].2 - (-0.0312)).abs() < 1e-5,
        "got {}",
        parsed[1].2
    );
    assert_eq!(parsed[1].3, 10226);

    assert_eq!(parsed[2].0, HeroId(1));
    assert_eq!(parsed[2].1, HeroId(99));
    assert!((parsed[2].2 - 0.15).abs() < 1e-6, "got {}", parsed[2].2);

    assert_eq!(parsed[3].0, HeroId(1));
    assert_eq!(parsed[3].1, HeroId(14));
    assert!(
        parsed[3].2 > 0.05 && parsed[3].2 <= 0.15,
        "got {}",
        parsed[3].2
    );

    // Explicit probability-ratio field (`with_synergy`) is preserved without /100 scaling,
    // whereas `"synergy"` is always STRATZ percentage points (e.g. 0.10 -> 0.001).
    let legacy_ratio_json =
        r#"[{"heroId1": 1, "heroId2": 97, "with_synergy": 0.042, "matchCount": 12500}]"#;
    let legacy_parsed = parse_stratz_with_synergy_json(legacy_ratio_json);
    assert_eq!(legacy_parsed.len(), 1);
    assert!(
        (legacy_parsed[0].2 - 0.042).abs() < 1e-6,
        "got {}",
        legacy_parsed[0].2
    );

    let small_pct_json = r#"[{"heroId1": 1, "heroId2": 97, "synergy": 0.10, "matchCount": 12500}]"#;
    let small_pct_parsed = parse_stratz_with_synergy_json(small_pct_json);
    assert_eq!(small_pct_parsed.len(), 1);
    assert!(
        (small_pct_parsed[0].2 - 0.001).abs() < 1e-6,
        "expected 0.10 percentage points to parse as 0.001, got {}",
        small_pct_parsed[0].2
    );
}

#[test]
fn fetch_stratz_with_synergy_batches_fails_closed_and_preserves_existing_cache() {
    use crate::sync::{
        LocalCacheManager, fetch_stratz_with_synergy_batches_with, parse_stratz_with_synergy_json,
    };
    use truesight_core::ports::PortError;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_stratz_partial_refresh_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let cache = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    let initial_complete =
        r#"[{"heroId1": 1, "heroId2": 57, "synergy": 2.445, "matchCount": 3444}]"#;
    cache
        .publish_staged_cache(
            None,
            Some(initial_complete),
            None,
            None,
            None,
            1_800_000_000,
        )
        .expect("initial cache persist should succeed");

    // Batch 1 succeeds, batch 2 fails with a transport error -> whole refresh must fail.
    let mut call_idx = 0usize;
    let err_res = fetch_stratz_with_synergy_batches_with(|_chunk| {
        call_idx += 1;
        if call_idx == 1 {
            Ok(r#"{"data":{"heroStats":{"matchUp":[{"heroId":1,"with":[{"heroId1":1,"heroId2":97,"matchCount":1200,"synergy":3.5,"winCount":650}]}]}}}"#.to_string())
        } else {
            Err(PortError("simulated batch 2 timeout".to_string()))
        }
    });
    assert!(
        err_res.is_err(),
        "partial batch failure must fail the entire STRATZ refresh"
    );

    // Batch 1 succeeds, batch 2 returns empty/unparseable JSON -> whole refresh must fail.
    let mut empty_idx = 0usize;
    let empty_res = fetch_stratz_with_synergy_batches_with(|_chunk| {
        empty_idx += 1;
        if empty_idx == 1 {
            Ok(r#"{"data":{"heroStats":{"matchUp":[{"heroId":1,"with":[{"heroId1":1,"heroId2":97,"matchCount":1200,"synergy":3.5,"winCount":650}]}]}}}"#.to_string())
        } else {
            Ok("{}".to_string())
        }
    });
    assert!(
        empty_res.is_err(),
        "empty/malformed batch response must fail the entire STRATZ refresh"
    );

    // Existing stratz_synergy.json cache must remain intact.
    let cached_after = std::fs::read_to_string(tmp_dir.join("stratz_synergy.json"))
        .expect("cached stratz_synergy.json should still exist");
    assert_eq!(cached_after, initial_complete);
    let parsed_cached = parse_stratz_with_synergy_json(&cached_after);
    assert_eq!(parsed_cached.len(), 1);
    assert!((parsed_cached[0].2 - 0.02445).abs() < 1e-5);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn fetch_stratz_with_synergy_batches_rejects_partial_graphql_response_with_nonempty_errors() {
    use crate::sync::{
        LocalCacheManager, fetch_stratz_with_synergy_batches_with, parse_stratz_with_synergy_json,
    };

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_stratz_graphql_errors_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let cache = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    let initial_complete =
        r#"[{"heroId1": 1, "heroId2": 57, "synergy": 2.445, "matchCount": 3444}]"#;
    cache
        .publish_staged_cache(
            None,
            Some(initial_complete),
            None,
            None,
            None,
            1_800_000_000,
        )
        .expect("initial cache persist should succeed");

    let partial_with_errors = r#"{"data":{"heroStats":{"matchUp":[{"heroId":1,"with":[{"heroId1":1,"heroId2":2,"matchCount":500,"synergy":2.5,"winCount":265}]}]}},"errors":[{"message":"partial resolver failure"}]}"#;
    assert!(
        parse_stratz_with_synergy_json(partial_with_errors).is_empty(),
        "parse_stratz_with_synergy_json must return empty Vec when top-level errors is nonempty"
    );

    let empty_errors_array = r#"{"data":{"heroStats":{"matchUp":[{"heroId":1,"with":[{"heroId1":1,"heroId2":2,"matchCount":500,"synergy":2.5,"winCount":265}]}]}},"errors":[]}"#;
    assert_eq!(
        parse_stratz_with_synergy_json(empty_errors_array).len(),
        1,
        "empty errors array must still allow valid synergy rows"
    );

    let null_errors = r#"{"data":{"heroStats":{"matchUp":[{"heroId":1,"with":[{"heroId1":1,"heroId2":2,"matchCount":500,"synergy":2.5,"winCount":265}]}]}},"errors":null}"#;
    assert_eq!(
        parse_stratz_with_synergy_json(null_errors).len(),
        1,
        "null errors must still allow valid synergy rows"
    );

    let mut call_idx = 0usize;
    let res = fetch_stratz_with_synergy_batches_with(|_chunk| {
        call_idx += 1;
        if call_idx == 1 {
            Ok(r#"{"data":{"heroStats":{"matchUp":[{"heroId":1,"with":[{"heroId1":1,"heroId2":97,"matchCount":1200,"synergy":3.5,"winCount":650}]}]}}}"#.to_string())
        } else {
            Ok(partial_with_errors.to_string())
        }
    });
    assert!(
        res.is_err(),
        "partial GraphQL response with nonempty top-level errors must fail the entire refresh"
    );

    let cached_after = std::fs::read_to_string(tmp_dir.join("stratz_synergy.json"))
        .expect("cached stratz_synergy.json should still exist");
    assert_eq!(
        cached_after, initial_complete,
        "existing stratz_synergy.json cache must remain completely unchanged"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn counter_trait_table_items_all_exist_in_adapter_catalog() {
    use truesight_core::domain::all_referenced_counter_item_ids;
    use truesight_core::ports::ItemMetaRepository;

    let repo = crate::item_repo::SeededItemMetaRepository::default();
    let referenced = all_referenced_counter_item_ids();
    assert!(
        !referenced.is_empty(),
        "counter_traits.toml must reference at least one counter item"
    );
    for &item_id in referenced {
        let entry = repo.item_catalog(item_id).unwrap_or_else(|| {
            panic!("counter item {item_id:?} missing from SeededItemMetaRepository catalog")
        });
        assert!(
            entry.cost > 0,
            "counter item {item_id:?} ({}) must have cost > 0",
            entry.slug
        );
        assert!(
            !entry.is_recipe,
            "counter item {item_id:?} ({}) must not be marked is_recipe",
            entry.slug
        );
        assert!(
            !entry.is_consumable_drop,
            "counter item {item_id:?} ({}) must not be marked is_consumable_drop",
            entry.slug
        );
    }
}

#[test]
fn gsi_parser_extracts_local_player_items_ignores_empty_and_coalesces_in_queue() {
    use crate::gsi::{BoundedDraftQueue, MAX_GSI_PAYLOAD_BYTES, parse_gsi_json_events};
    use truesight_core::domain::{HeroId, Role, Side};
    use truesight_core::ports::DraftEvent;

    assert_eq!(MAX_GSI_PAYLOAD_BYTES, 64 * 1024);

    let local_payload = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 900 },
        "player": { "steamid": "76561198000000001", "team_name": "radiant" },
        "hero": { "name": "npc_dota_hero_antimage" },
        "items": {
            "slot0": { "name": "item_bfury" },
            "slot1": { "name": "item_power_treads" },
            "slot2": { "name": "item_empty" },
            "slot6": { "name": "item_magic_wand" },
            "stash0": { "name": "item_manta" },
            "teleport0": { "name": "item_tpscroll" },
            "neutral0": { "name": "item_occult_bracelet" }
        },
        "allplayers": {
            "player5": { "items": { "slot0": { "name": "item_rapier" } } }
        }
    }"#;

    let events = parse_gsi_json_events(local_payload);
    assert!(
        events.contains(&DraftEvent::InventoryUpdated {
            hero_slug: Some("npc_dota_hero_antimage".to_string()),
            item_slugs: vec![
                "bfury".to_string(),
                "power_treads".to_string(),
                "magic_wand".to_string(),
                "manta".to_string(),
                "tpscroll".to_string(),
                "occult_bracelet".to_string(),
            ],
        }),
        "expected InventoryUpdated with canonical stripped slugs, got: {events:?}"
    );
    assert!(
        !events.iter().any(|ev| match ev {
            DraftEvent::InventoryUpdated { item_slugs, .. } =>
                item_slugs.iter().any(|s| s == "rapier"),
            _ => false,
        }),
        "must never parse allplayers inventories"
    );

    let all_empty_payload = r#"{
        "player": { "steamid": "76561198000000001" },
        "items": {
            "slot0": { "name": "item_empty" },
            "slot1": { "name": "item_empty" },
            "teleport0": { "name": "item_empty" }
        }
    }"#;
    let empty_events = parse_gsi_json_events(all_empty_payload);
    assert!(
        empty_events.contains(&DraftEvent::InventoryUpdated {
            hero_slug: None,
            item_slugs: Vec::new(),
        }),
        "all-empty recognized slots must still emit empty InventoryUpdated: {empty_events:?}"
    );

    let queue = BoundedDraftQueue::new(3);
    assert!(queue.push_for_epoch(
        0,
        DraftEvent::PickDetected {
            side: Side::Allies,
            slot: 0,
            hero_id: HeroId(1),
            role: Some(Role::Carry),
        },
    ));
    assert!(queue.push_for_epoch(0, DraftEvent::DraftReset));
    assert!(queue.push_for_epoch(
        0,
        DraftEvent::InventoryUpdated {
            hero_slug: Some("npc_dota_hero_antimage".to_string()),
            item_slugs: vec!["bfury".to_string()],
        },
    ));
    assert!(queue.push_for_epoch(
        0,
        DraftEvent::InventoryUpdated {
            hero_slug: Some("npc_dota_hero_antimage".to_string()),
            item_slugs: vec!["bfury".to_string(), "manta".to_string()],
        },
    ));
    assert!(queue.push_for_epoch(
        0,
        DraftEvent::InventoryUpdated {
            hero_slug: Some("npc_dota_hero_antimage".to_string()),
            item_slugs: vec![
                "bfury".to_string(),
                "manta".to_string(),
                "skadi".to_string()
            ],
        },
    ));

    let drained = queue.drain();
    assert!(drained.contains(&DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 0,
        hero_id: HeroId(1),
        role: Some(Role::Carry),
    }));
    assert!(drained.contains(&DraftEvent::DraftReset));
    assert!(drained.contains(&DraftEvent::InventoryUpdated {
        hero_slug: Some("npc_dota_hero_antimage".to_string()),
        item_slugs: vec![
            "bfury".to_string(),
            "manta".to_string(),
            "skadi".to_string(),
        ],
    }));

    // Hero change without new items must evict stale queued inventory and overflow snapshot.
    let gsi_queue = BoundedDraftQueue::new(16);
    assert!(gsi_queue.record_and_push_gsi_events(parse_gsi_json_events(local_payload)));
    let next_hero_no_items = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 905 },
        "player": { "steamid": "76561198000000001", "team_name": "radiant" },
        "hero": { "name": "npc_dota_hero_nyx_assassin" }
    }"#;
    assert!(gsi_queue.record_and_push_gsi_events(parse_gsi_json_events(next_hero_no_items)));
    let reconciled = gsi_queue.drain_and_reconcile_gsi();
    assert!(reconciled.contains(&DraftEvent::LocalPlayerHeroDetected {
        hero_slug: "npc_dota_hero_nyx_assassin".to_string(),
    }));
    assert!(
        !reconciled
            .iter()
            .any(|ev| matches!(ev, DraftEvent::InventoryUpdated { .. })),
        "switching queued local hero without items must evict prior hero inventory: {reconciled:?}"
    );
}

#[test]
fn item_sync_parsers_decode_stratz_opendota_and_fallback_offline() {
    use crate::item_repo::SeededItemMetaRepository;
    use crate::sync::{
        LocalCacheManager, parse_opendota_item_constants_json, parse_opendota_item_timings_json,
        parse_stratz_hero_item_purchase_json,
    };
    use truesight_core::domain::{Bracket, HeroId, ItemId, ItemPhase, Role};
    use truesight_core::ports::ItemMetaRepository;

    let opendota_constants = r#"{
        "blink": {"id": 1, "dname": "Blink Dagger", "cost": 2250, "components": null},
        "bfury": {"id": 145, "dname": "Battle Fury", "cost": 4100, "components": ["pers", "broadsword"]}
    }"#;
    let parsed_od = parse_opendota_item_constants_json(opendota_constants);
    assert_eq!(parsed_od.len(), 2);
    let bfury_od = parsed_od
        .iter()
        .find(|e| e.id == ItemId(145))
        .expect("Battle Fury should be parsed from OpenDota constants");
    assert_eq!(bfury_od.slug, "bfury");
    assert_eq!(bfury_od.cost, 4100);

    let stratz_constants = r#"{
        "data": {
            "constants": {
                "items": [
                    {"id": 145, "name": "item_bfury", "displayName": "Battle Fury", "stat": {"cost": 4100, "isRecipe": false}, "components": [{"componentId": 69}]}
                ]
            }
        }
    }"#;
    let parsed_stratz_const = parse_opendota_item_constants_json(stratz_constants);
    assert_eq!(parsed_stratz_const.len(), 1);
    assert_eq!(parsed_stratz_const[0].id, ItemId(145));
    assert_eq!(parsed_stratz_const[0].slug, "bfury");
    assert_eq!(parsed_stratz_const[0].components, vec![ItemId(69)]);

    let stratz_purchases = r#"{
        "itemStartingPurchase": [
            {"itemId": 11, "time": -60, "instance": 1, "matchCount": 1000, "winCount": 530, "wasGiven": false},
            {"itemId": 44, "time": -60, "instance": 1, "matchCount": 900, "winCount": 470, "wasGiven": true}
        ],
        "itemFullPurchase": [
            {"itemId": 63, "time": 420, "instance": 1, "matchCount": 1200, "winCount": 640},
            {"itemId": 145, "time": 960, "instance": 1, "matchCount": 1100, "winCount": 610},
            {"itemId": 160, "time": 1800, "instance": 1, "matchCount": 700, "winCount": 420}
        ]
    }"#;
    let stratz_stats = parse_stratz_hero_item_purchase_json(stratz_purchases);
    assert!(
        stratz_stats
            .iter()
            .any(|s| s.item_id == ItemId(11) && s.phase == ItemPhase::Starting)
    );
    assert!(
        !stratz_stats.iter().any(|s| s.item_id == ItemId(44)),
        "wasGiven: true starting items should be excluded"
    );
    assert!(
        stratz_stats
            .iter()
            .any(|s| s.item_id == ItemId(63) && s.phase == ItemPhase::Early)
    );
    assert!(
        stratz_stats
            .iter()
            .any(|s| s.item_id == ItemId(145) && s.phase == ItemPhase::Core)
    );
    assert!(
        stratz_stats
            .iter()
            .any(|s| s.item_id == ItemId(160) && s.phase == ItemPhase::Late)
    );

    let base_repo = SeededItemMetaRepository::default();
    let od_timings = r#"[
        {"hero_id": 1, "item": "bfury", "time": 900, "games": "45", "wins": "25"}
    ]"#;
    let timings_stats = parse_opendota_item_timings_json(od_timings, &base_repo);
    assert_eq!(timings_stats.len(), 1);
    assert_eq!(timings_stats[0].item_id, ItemId(145));
    assert!((timings_stats[0].avg_time_mins - 15.0).abs() < 0.01);
    assert_eq!(timings_stats[0].phase, ItemPhase::Core);

    let tmp_dir = std::env::temp_dir().join("truesight_item_cache_test");
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let mgr = LocalCacheManager::with_dir(tmp_dir.clone(), 0);
    let fallback_repo = mgr.build_item_repository();
    assert!(fallback_repo.item_catalog(ItemId(145)).is_some());
    assert!(
        !fallback_repo
            .hero_item_stats(HeroId(1), Role::Carry, Bracket::Legend)
            .unwrap()
            .is_empty()
    );

    mgr.write_cached_game_patch("7.40b").unwrap();
    assert_eq!(mgr.read_cached_game_patch().as_deref(), Some("7.40b"));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn item_catalog_and_opendota_parser_preserve_components_and_canonical_ids() {
    use crate::item_repo::SeededItemMetaRepository;
    use crate::sync::parse_opendota_item_constants_json;
    use std::collections::HashSet;
    use truesight_core::domain::ItemId;
    use truesight_core::ports::ItemMetaRepository;

    let opendota_json = r#"{
        "pers": {"id": 69, "dname": "Perseverance", "cost": 1400, "components": null},
        "bfury": {"id": 145, "dname": "Battle Fury", "cost": 3900, "components": ["pers", "broadsword", "quelling_blade"]},
        "pipe": {"id": 90, "dname": "Pipe of Insight", "cost": 3725, "components": ["ring_of_tarrasque", "item_cloak", "shawl", ""]}
    }"#;
    let parsed = parse_opendota_item_constants_json(opendota_json);
    let bfury = parsed
        .iter()
        .find(|e| e.id == ItemId(145))
        .expect("bfury should parse");
    assert_eq!(
        bfury.components,
        vec![ItemId(69), ItemId(3), ItemId(11)],
        "bfury components should resolve both in-payload and embedded fallback slugs"
    );
    let pipe = parsed
        .iter()
        .find(|e| e.id == ItemId(90))
        .expect("pipe should parse");
    assert_eq!(
        pipe.components,
        vec![ItemId(279), ItemId(31), ItemId(1848)],
        "pipe components should strip item_ prefix and ignore empty strings"
    );

    let repo = SeededItemMetaRepository::default();
    let soul_ring = repo
        .item_catalog(ItemId(178))
        .expect("178 must be Soul Ring");
    assert_eq!(soul_ring.slug, "soul_ring");
    assert_eq!(soul_ring.display_name, "Soul Ring");
    assert_eq!(soul_ring.cost, 805);

    let cloak = repo.item_catalog(ItemId(31)).expect("31 must be Cloak");
    assert_eq!(cloak.slug, "cloak");
    assert_eq!(cloak.display_name, "Cloak");
    assert_eq!(cloak.cost, 900);

    let relic = repo
        .item_catalog(ItemId(54))
        .expect("54 must be Sacred Relic");
    assert_eq!(relic.slug, "relic");
    assert_eq!(relic.display_name, "Sacred Relic");
    assert_eq!(relic.cost, 3400);

    let rapier = repo
        .item_catalog(ItemId(133))
        .expect("133 must be Divine Rapier");
    assert_eq!(rapier.slug, "rapier");
    assert_eq!(rapier.display_name, "Divine Rapier");
    assert_eq!(rapier.cost, 5600);
    assert_eq!(rapier.components, vec![ItemId(54), ItemId(51)]);

    let mut seen_ids = HashSet::new();
    let mut seen_slugs = HashSet::new();
    for raw_id in 1..=5000u16 {
        if let Some(entry) = repo.item_catalog(ItemId(raw_id)) {
            assert!(seen_ids.insert(entry.id), "duplicate ItemId {:?}", entry.id);
            assert!(
                seen_slugs.insert(entry.slug.clone()),
                "duplicate slug {}",
                entry.slug
            );
            for comp_id in &entry.components {
                let comp = repo.item_catalog(*comp_id).unwrap_or_else(|| {
                    panic!(
                        "component {:?} of {} ({:?}) missing from EMBEDDED_ITEM_CATALOG",
                        comp_id, entry.slug, entry.id
                    )
                });
                assert!(
                    !comp.is_recipe,
                    "component {:?} of {} must not be a recipe",
                    comp_id, entry.slug
                );
            }
        }
    }
}

#[test]
fn sync_fetches_and_atomically_publishes_item_constants_and_builds_from_empty_cache() {
    use crate::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, ItemPhase, Role};
    use truesight_core::ports::ItemMetaRepository;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_sync_items_empty_cache_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    let now = 1_795_000_000_u64;
    assert!(mgr.is_meta_stale(now), "empty cache must be stale");

    let hero_stats_json = r#"[
        {"id": 1, "1_pick": 1000, "1_win": 540, "2_pick": 1000, "2_win": 540, "3_pick": 1000, "3_win": 540, "4_pick": 1000, "4_win": 540, "5_pick": 1000, "5_win": 540, "6_pick": 1000, "6_win": 540, "7_pick": 1000, "7_win": 540}
    ]"#;
    let item_constants_json = r#"{
        "pers": {"id": 69, "dname": "Perseverance", "cost": 1400, "components": null},
        "broadsword": {"id": 3, "dname": "Broadsword", "cost": 1000, "components": null},
        "quelling_blade": {"id": 11, "dname": "Quelling Blade", "cost": 100, "components": null},
        "bfury": {"id": 145, "dname": "Battle Fury", "cost": 3900, "components": ["pers", "broadsword", "quelling_blade"]}
    }"#;
    let hero_item_timings_json = r#"[
        {"hero_id": 1, "item": "bfury", "time": 840, "games": "100", "wins": "60"},
        {"hero_id": 2, "item": "blink", "time": 720, "games": "80", "wins": "48"}
    ]"#;

    let msg = mgr
        .sync_from_sources_with(
            false,
            now,
            |url| match url {
                "https://api.opendota.com/api/heroStats" => Ok(hero_stats_json.to_string()),
                "https://api.opendota.com/api/constants/items" => {
                    Ok(item_constants_json.to_string())
                }
                u if u.starts_with("https://api.opendota.com/api/scenarios/itemTimings") => {
                    Ok(hero_item_timings_json.to_string())
                }
                other => Err(truesight_core::ports::PortError(format!(
                    "unexpected URL: {other}"
                ))),
            },
            |_token| Err(truesight_core::ports::PortError("no stratz token".into())),
            |_token, _hero_id| Err(truesight_core::ports::PortError("no stratz token".into())),
        )
        .expect("sync_from_sources_with should succeed");

    assert!(msg.contains("heroStats"), "sync message: {msg}");
    assert!(msg.contains("itemConstants"), "sync message: {msg}");
    assert!(msg.contains("itemBuilds"), "sync message: {msg}");
    assert!(
        mgr.item_constants_json_path().exists(),
        "item_constants.json must be published atomically"
    );
    assert!(
        mgr.hero_item_stats_json_path().exists(),
        "hero_item_stats.json must be published atomically"
    );
    assert!(
        !mgr.is_meta_stale(now + 60),
        "cache must be fresh immediately after sync"
    );

    let repo = mgr.build_item_repository();
    let bfury = repo
        .item_catalog(ItemId(145))
        .expect("Battle Fury should load from cached item_constants.json");
    assert_eq!(bfury.cost, 3900);
    assert_eq!(
        bfury.components,
        vec![ItemId(69), ItemId(3), ItemId(11)],
        "cached item_constants.json must preserve resolved recipe components"
    );

    let am_legend_stats = repo
        .hero_item_stats(HeroId(1), Role::Carry, Bracket::Legend)
        .expect("Anti-Mage Legend item stats should load");
    let am_bfury_legend = am_legend_stats
        .iter()
        .find(|s| s.item_id == ItemId(145))
        .expect("Anti-Mage should have cached Battle Fury timing");
    assert_eq!(am_bfury_legend.phase, ItemPhase::Core);
    assert!((am_bfury_legend.avg_time_mins - 14.0).abs() < 0.01);
    assert_eq!(am_bfury_legend.match_count, 100);
    assert_eq!(am_bfury_legend.win_count, 60);

    let am_divine_stats = repo
        .hero_item_stats(HeroId(1), Role::Carry, Bracket::Divine)
        .expect("Anti-Mage Divine item stats should load");
    let am_bfury_divine = am_divine_stats
        .iter()
        .find(|s| s.item_id == ItemId(145))
        .expect("Anti-Mage Divine should scale unbracketed fallback");
    assert!((am_bfury_divine.avg_time_mins - (14.0 * 0.92)).abs() < 0.02);

    let axe_stats = repo
        .hero_item_stats(HeroId(2), Role::Offlane, Bracket::Divine)
        .expect("Axe item stats should load from multi-hero timings");
    assert!(
        axe_stats.iter().any(|s| s.item_id == ItemId(1)),
        "multi-hero hero_item_stats.json should populate HeroId(2) too"
    );

    // Deleting item_constants.json must make is_meta_stale return true even within 24h TTL.
    std::fs::remove_file(mgr.item_constants_json_path()).unwrap();
    assert!(
        mgr.is_meta_stale(now + 60),
        "missing item_constants.json must mark meta cache stale"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn sync_from_sources_with_stratz_token_preserves_requested_hero_id_without_row_echo() {
    use crate::sync::{LocalCacheManager, stratz_hero_item_purchases_graphql_payload};
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role};
    use truesight_core::ports::ItemMetaRepository;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_sync_stratz_items_no_row_hero_id_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.stratz_api_token = Some("eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string());
    mgr.set_target_item_heroes(vec![HeroId(2)]);
    let now = 1_795_000_000_u64;

    let payload = stratz_hero_item_purchases_graphql_payload(2);
    assert!(
        payload.contains("heroId: 2")
            && payload.contains("itemStartingPurchase(")
            && payload.contains("itemBootPurchase(")
            && payload.contains("itemFullPurchase(")
            && payload.contains("HERALD_GUARDIAN")
            && payload.contains("DIVINE_IMMORTAL")
            && !payload.contains("itemStartingPurchase(heroId: 2) { itemId time"),
        "STRATZ item query must target heroId: 2 across all brackets without starting time field, got: {payload}"
    );

    let hero_stats_json = r#"[{"id": 2, "4_pick": 1000, "4_win": 530}]"#;
    let item_constants_json = r#"{
        "blink": {"id": 1, "dname": "Blink Dagger", "cost": 2250, "components": null},
        "blade_mail": {"id": 127, "dname": "Blade Mail", "cost": 2100, "components": null},
        "black_king_bar": {"id": 116, "dname": "Black King Bar", "cost": 4050, "components": null}
    }"#;
    let stratz_items_response = r#"{"data":{"heroStats":{"itemStartingPurchase":[{"itemId":1,"time":-60,"instance":0,"matchCount":1200,"winCount":680,"wasGiven":false}],"itemFullPurchase":[{"itemId":1,"time":720,"instance":0,"matchCount":4200,"winCount":2450},{"itemId":127,"time":980,"instance":0,"matchCount":3900,"winCount":2280},{"itemId":116,"time":1650,"instance":0,"matchCount":2800,"winCount":1720}]}}}"#;

    let mut requested_hero_ids = Vec::new();
    mgr.sync_from_sources_with(
        false,
        now,
        |url| match url {
            "https://api.opendota.com/api/heroStats" => Ok(hero_stats_json.to_string()),
            "https://api.opendota.com/api/constants/items" => Ok(item_constants_json.to_string()),
            other => Err(truesight_core::ports::PortError(format!(
                "unexpected fallback URL when STRATZ succeeded: {other}"
            ))),
        },
        |_token| Ok(r#"{"data":{"heroStats":{"matchUp":[]}}}"#.to_string()),
        |_token, hero_id| {
            requested_hero_ids.push(hero_id);
            assert_eq!(hero_id, 2, "must request HeroId(2) instead of hardcoded 1");
            Ok(stratz_items_response.to_string())
        },
    )
    .expect("sync_from_sources_with with STRATZ token should succeed");

    assert_eq!(requested_hero_ids, vec![2]);

    let stats = mgr
        .build_item_repository()
        .hero_item_stats(HeroId(2), Role::Offlane, Bracket::Archon)
        .expect("Axe (HeroId(2)) item stats must be cached from STRATZ response");
    assert!(
        stats.iter().any(|s| s.item_id == ItemId(1)),
        "expected Blink Dagger (ItemId(1)) in HeroId(2) stats"
    );
    assert!(
        stats.iter().any(|s| s.item_id == ItemId(127)),
        "expected Blade Mail (ItemId(127)) in HeroId(2) stats"
    );
    assert!(
        stats.iter().any(|s| s.item_id == ItemId(116)),
        "expected Black King Bar (ItemId(116)) in HeroId(2) stats"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn sync_from_sources_with_opendota_fallback_requests_configured_target_hero() {
    use crate::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role};
    use truesight_core::ports::ItemMetaRepository;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_sync_opendota_target_hero_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.set_target_item_heroes(vec![HeroId(2)]);
    let now = 1_795_000_000_u64;

    let hero_stats_json = r#"[{"id": 2, "4_pick": 1000, "4_win": 530}]"#;
    let item_constants_json = r#"{
        "blink": {"id": 1, "dname": "Blink Dagger", "cost": 2250, "components": null},
        "blade_mail": {"id": 127, "dname": "Blade Mail", "cost": 2100, "components": null}
    }"#;
    let axe_timings_json = r#"[
        {"hero_id": 2, "item": "blink", "time": 720, "games": "320", "wins": "192"},
        {"hero_id": 2, "item": "blade_mail", "time": 960, "games": "280", "wins": "168"}
    ]"#;

    let mut requested_item_urls = Vec::new();
    mgr.sync_from_sources_with(
        false,
        now,
        |url| match url {
            "https://api.opendota.com/api/heroStats" => Ok(hero_stats_json.to_string()),
            "https://api.opendota.com/api/constants/items" => Ok(item_constants_json.to_string()),
            u if u.starts_with("https://api.opendota.com/api/scenarios/itemTimings") => {
                requested_item_urls.push(u.to_string());
                assert_eq!(
                    u,
                    "https://api.opendota.com/api/scenarios/itemTimings?hero_id=2"
                );
                Ok(axe_timings_json.to_string())
            }
            other => Err(truesight_core::ports::PortError(format!(
                "unexpected URL: {other}"
            ))),
        },
        |_token| Err(truesight_core::ports::PortError("no stratz token".into())),
        |_token, _hero_id| Err(truesight_core::ports::PortError("no stratz token".into())),
    )
    .expect("OpenDota fallback sync for HeroId(2) should succeed");

    assert_eq!(
        requested_item_urls,
        vec!["https://api.opendota.com/api/scenarios/itemTimings?hero_id=2"]
    );
    let stats = mgr
        .build_item_repository()
        .hero_item_stats(HeroId(2), Role::Offlane, Bracket::Archon)
        .expect("HeroId(2) item stats must be cached from OpenDota fallback");
    assert!(stats.iter().any(|s| s.item_id == ItemId(1)));
    assert!(stats.iter().any(|s| s.item_id == ItemId(127)));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn sync_from_sources_with_accumulates_multiple_target_item_heroes() {
    use crate::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role};
    use truesight_core::ports::ItemMetaRepository;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_sync_multi_target_heroes_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.set_target_item_heroes(vec![HeroId(2), HeroId(74)]);
    let now = 1_795_000_000_u64;

    let hero_stats_json =
        r#"[{"id": 2, "4_pick": 1000, "4_win": 530}, {"id": 74, "4_pick": 1000, "4_win": 510}]"#;
    let item_constants_json = r#"{
        "blink": {"id": 1, "dname": "Blink Dagger", "cost": 2250, "components": null},
        "hand_of_midas": {"id": 65, "dname": "Hand of Midas", "cost": 2200, "components": null}
    }"#;

    let mut requested_item_urls = Vec::new();
    mgr.sync_from_sources_with(
        false,
        now,
        |url| match url {
            "https://api.opendota.com/api/heroStats" => Ok(hero_stats_json.to_string()),
            "https://api.opendota.com/api/constants/items" => Ok(item_constants_json.to_string()),
            "https://api.opendota.com/api/scenarios/itemTimings?hero_id=2" => {
                requested_item_urls.push(url.to_string());
                Ok(r#"[{"hero_id": 2, "item": "blink", "time": 720, "games": "300", "wins": "180"}]"#.to_string())
            }
            "https://api.opendota.com/api/scenarios/itemTimings?hero_id=74" => {
                requested_item_urls.push(url.to_string());
                Ok(r#"[{"hero_id": 74, "item": "hand_of_midas", "time": 600, "games": "400", "wins": "220"}]"#.to_string())
            }
            other => Err(truesight_core::ports::PortError(format!(
                "unexpected URL: {other}"
            ))),
        },
        |_token| Err(truesight_core::ports::PortError("no stratz token".into())),
        |_token, _hero_id| Err(truesight_core::ports::PortError("no stratz token".into())),
    )
    .expect("multi-hero sync should succeed");

    assert_eq!(
        requested_item_urls,
        vec![
            "https://api.opendota.com/api/scenarios/itemTimings?hero_id=2",
            "https://api.opendota.com/api/scenarios/itemTimings?hero_id=74",
        ]
    );
    let repo = mgr.build_item_repository();
    let axe_stats = repo
        .hero_item_stats(HeroId(2), Role::Offlane, Bracket::Archon)
        .expect("HeroId(2) stats must be cached");
    assert!(axe_stats.iter().any(|s| s.item_id == ItemId(1)));
    let invoker_stats = repo
        .hero_item_stats(HeroId(74), Role::Mid, Bracket::Archon)
        .expect("HeroId(74) stats must be cached");
    assert!(invoker_stats.iter().any(|s| s.item_id == ItemId(65)));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn compute_top_bar_capture_strip_rect_matches_window_offset_and_top_8_percent() {
    use crate::vision::compute_top_bar_capture_strip_rect;

    assert_eq!(compute_top_bar_capture_strip_rect(0, 0, 0, 1080), None);
    assert_eq!(compute_top_bar_capture_strip_rect(0, 0, 1920, 0), None);
    assert_eq!(
        compute_top_bar_capture_strip_rect(200, 180, 1600, 900),
        Some((200.0, 180.0, 1600.0, 72.0))
    );
    assert_eq!(
        compute_top_bar_capture_strip_rect(-1920, 0, 1920, 1080),
        Some((-1920.0, 0.0, 1920.0, 87.0))
    );
}

#[test]
fn parse_xrandr_listmonitors_handles_heterogeneous_and_negative_offsets() {
    use crate::vision::parse_xrandr_listmonitors;

    let sample = "\
Monitors: 3
 0: +*DP-1 1920/510x1080/290+0+0  DP-1
 1: +HDMI-1 1280/340x720/190+1920+0  HDMI-1
 2: +DP-2 1920/527x1080/296-1920+120  DP-2
";
    assert_eq!(
        parse_xrandr_listmonitors(sample),
        vec![
            (0.0, 0.0, 1920.0, 1080.0),
            (1920.0, 0.0, 1280.0, 720.0),
            (-1920.0, 120.0, 1920.0, 1080.0),
        ]
    );
}

#[test]
fn bounded_vision_worker_propagates_and_clears_capture_strip_rect() {
    use crate::vision::{BoundedVisionWorker, RgbFrame, ScreenFrameSource, VisionCaptureMode};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    struct ToggleSource {
        emit_frame: Arc<AtomicBool>,
    }

    impl ScreenFrameSource for ToggleSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            if self.emit_frame.load(Ordering::Acquire) {
                Ok(Some(
                    RgbFrame::with_source_height(1600, 72, 900, vec![0; 1600 * 72 * 3])
                        .with_capture_strip_rect((200.0, 180.0, 1600.0, 72.0)),
                ))
            } else {
                Ok(None)
            }
        }
    }

    let emit_frame = Arc::new(AtomicBool::new(true));
    let worker = BoundedVisionWorker::spawn_with_source(ToggleSource {
        emit_frame: Arc::clone(&emit_frame),
    })
    .expect("worker must spawn");

    let deadline = Instant::now() + Duration::from_secs(3);
    while worker.capture_strip_rect().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        worker.capture_strip_rect(),
        Some((200.0, 180.0, 1600.0, 72.0))
    );

    worker.set_capture_mode(VisionCaptureMode::Suppressed);
    let deadline = Instant::now() + Duration::from_secs(3);
    while worker.capture_strip_rect().is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        worker.capture_strip_rect(),
        None,
        "Suppressed mode must clear capture_strip_rect"
    );

    emit_frame.store(false, Ordering::Release);
    worker.set_capture_mode(VisionCaptureMode::Any);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(worker.capture_strip_rect(), None);
}

#[test]
fn bounded_gsi_listener_waits_for_fragmented_body_after_connect_on_blocking_stream() {
    use std::io::Read;

    let mut listener = BoundedGsiListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr();
    let body = r#"{"map":{"game_state":"DOTA_GAMERULES_STATE_HERO_SELECTION","clock_time":-15}}"#;
    let headers = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );

    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    // Simulate a bursty/delayed client on Windows where accept() completes before the payload arrives
    thread::sleep(Duration::from_millis(45));
    stream.write_all(headers.as_bytes()).unwrap();
    stream.flush().unwrap();
    thread::sleep(Duration::from_millis(45));
    stream.write_all(body.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut resp = String::new();
    let _ = stream.read_to_string(&mut resp);
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "Fragmented/delayed POST must receive 200 OK instead of premature WouldBlock 400: {resp}"
    );

    let events = listener.poll_events().unwrap();
    assert_eq!(
        events,
        vec![DraftEvent::GameRulesStateChanged {
            state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
            game_time_secs: -15,
        }]
    );
}

#[test]
fn embedded_item_atlas_decodes_all_embedded_catalog_items() {
    let atlas = crate::item_repo::parse_item_atlas().expect("embedded item atlas must decode");
    assert_eq!(atlas.item_width, 44);
    assert_eq!(atlas.item_height, 32);
    let ids = crate::item_repo::embedded_item_ids();
    assert!(!ids.is_empty());
    assert!(
        ids.len() >= 199,
        "expanded catalog should cover all 199 active items"
    );
    assert_eq!(atlas.items.len(), ids.len());
    for id in ids {
        let rgb = atlas
            .items
            .get(&id)
            .unwrap_or_else(|| panic!("missing embedded icon for item {:?}", id));
        assert_eq!(rgb.len(), 44 * 32 * 3);
    }

    let catalog_repo = crate::item_repo::SeededItemMetaRepository::default();
    for patch_item_id in [635_u16, 692, 1942, 1944, 1946, 1948, 1953, 1955] {
        let iid = truesight_core::domain::ItemId(patch_item_id);
        assert!(
            atlas.items.contains_key(&iid),
            "item_atlas.bin must contain patch item #{patch_item_id}"
        );
        assert!(
            truesight_core::ports::ItemMetaRepository::item_catalog(&catalog_repo, iid).is_some(),
            "EMBEDDED_ITEM_CATALOG must contain patch item #{patch_item_id}"
        );
    }
    assert_eq!(
        truesight_core::ports::ItemMetaRepository::item_catalog(
            &catalog_repo,
            truesight_core::domain::ItemId(1953)
        )
        .unwrap()
        .display_name,
        "Sabatons of Mordiggian"
    );
}

#[test]
fn opendota_item_timings_aggregates_weighted_times_and_item_popularity_parses_phases() {
    use crate::item_repo::SeededItemMetaRepository;
    use crate::sync::{parse_opendota_item_popularity_json, parse_opendota_item_timings_json};
    use truesight_core::domain::{Bracket, HeroId, HeroItemStat, ItemId, ItemPhase, Role};
    use truesight_core::ports::ItemMetaRepository;

    let repo = SeededItemMetaRepository::default();
    let multi_bucket_timings = r#"[
        {"hero_id": 88, "item": "arcane_boots", "time": 420, "games": "60", "wins": "36"},
        {"hero_id": 88, "item": "arcane_boots", "time": 720, "games": "40", "wins": "22"}
    ]"#;
    let parsed_timings = parse_opendota_item_timings_json(multi_bucket_timings, &repo);
    assert_eq!(parsed_timings.len(), 1);
    assert_eq!(parsed_timings[0].item_id, ItemId(180));
    assert_eq!(parsed_timings[0].match_count, 100);
    assert_eq!(parsed_timings[0].win_count, 58);
    // Weighted average: (60 * 7m + 40 * 12m) / 100 = 9.0m
    assert!((parsed_timings[0].avg_time_mins - 9.0).abs() < 0.05);
    assert_eq!(parsed_timings[0].phase, ItemPhase::Early);

    let pop_json = r#"{
        "start_game_items": {"44": 90, "34": 60, "38": 75, "9999": 40, "4204": 50},
        "early_game_items": {"180": 80, "46": 190, "44": 150, "40": 140, "38": 120, "8": 210, "69": 180, "9999": 110},
        "mid_game_items": {"1107": 45, "104": 30, "30": 95, "43": 85, "90": 70, "24": 300, "51": 280, "52": 260, "53": 240},
        "late_game_items": {"108": 25}
    }"#;
    let pop_stats = parse_opendota_item_popularity_json(pop_json, &repo);
    assert!(
        pop_stats
            .iter()
            .any(|s| s.item_id == ItemId(44) && s.phase == ItemPhase::Starting)
    );
    assert!(
        pop_stats
            .iter()
            .any(|s| s.item_id == ItemId(38) && s.phase == ItemPhase::Starting),
        "Clarity (38) must be accepted in Starting"
    );
    for forbidden_id in [4204_u16, 9999, 46, 30, 40, 43, 90, 8, 24, 51, 52, 53, 69] {
        assert!(
            !pop_stats.iter().any(|s| s.item_id == ItemId(forbidden_id)),
            "uncataloged/consumable/detection/counter/component item {forbidden_id} must be excluded from itemPopularity"
        );
    }
    assert!(
        !pop_stats
            .iter()
            .any(|s| matches!(s.item_id.0, 44 | 38) && s.phase != ItemPhase::Starting),
        "lane consumables (Tango/Clarity) must never leak into Early/Core/Late from itemPopularity"
    );

    // Cached STRATZ custom_hero_stats with raw Secret Shop / Basic components must filter them
    // out of Early/Core/Late while keeping standalone Blink (1)/Shard (609) & completed items.
    let mut custom_repo = SeededItemMetaRepository::default();
    custom_repo.custom_hero_stats.push((
        HeroId(41),
        None,
        None,
        vec![
            HeroItemStat {
                item_id: ItemId(24), // Ultimate Orb
                phase: ItemPhase::Late,
                avg_time_mins: 26.5,
                match_count: 67,
                win_count: 54,
            },
            HeroItemStat {
                item_id: ItemId(51), // Demon Edge
                phase: ItemPhase::Core,
                avg_time_mins: 24.2,
                match_count: 63,
                win_count: 40,
            },
            HeroItemStat {
                item_id: ItemId(69), // Perseverance
                phase: ItemPhase::Early,
                avg_time_mins: 11.0,
                match_count: 380,
                win_count: 190,
            },
            HeroItemStat {
                item_id: ItemId(1), // Blink Dagger (standalone allowed)
                phase: ItemPhase::Core,
                avg_time_mins: 18.0,
                match_count: 120,
                win_count: 70,
            },
            HeroItemStat {
                item_id: ItemId(609), // Aghanim's Shard (standalone allowed)
                phase: ItemPhase::Core,
                avg_time_mins: 20.0,
                match_count: 200,
                win_count: 130,
            },
            HeroItemStat {
                item_id: ItemId(145), // Battle Fury (completed)
                phase: ItemPhase::Core,
                avg_time_mins: 17.4,
                match_count: 359,
                win_count: 165,
            },
            HeroItemStat {
                item_id: ItemId(147), // Manta Style (completed)
                phase: ItemPhase::Core,
                avg_time_mins: 23.0,
                match_count: 293,
                win_count: 152,
            },
        ],
    ));
    let void_stats = custom_repo
        .hero_item_stats(HeroId(41), Role::Carry, Bracket::Ancient)
        .unwrap();
    for raw_id in [24_u16, 51, 69] {
        assert!(
            void_stats.iter().all(|s| s.item_id != ItemId(raw_id)),
            "raw component #{raw_id} must be filtered from cached Void progression: {void_stats:?}"
        );
    }
    for kept_id in [1_u16, 609, 145, 147] {
        assert!(
            void_stats.iter().any(|s| s.item_id == ItemId(kept_id)),
            "standalone/completed item #{kept_id} must stay in cached Void progression"
        );
    }
    assert!(
        pop_stats
            .iter()
            .any(|s| s.item_id == ItemId(1107) && s.phase == ItemPhase::Core)
    );
    assert!(
        pop_stats
            .iter()
            .any(|s| s.item_id == ItemId(108) && s.phase == ItemPhase::Late)
    );

    // Curated hero-specific progressions and role-filtered situational items:
    let nyx_pos4 = repo
        .hero_item_stats(HeroId(88), Role::SoftSupport, Bracket::Ancient)
        .unwrap();
    assert!(
        nyx_pos4
            .iter()
            .any(|s| s.item_id == ItemId(1107) && s.phase == ItemPhase::Core)
    );
    assert!(
        nyx_pos4
            .iter()
            .all(|s| s.item_id != ItemId(90) && s.item_id != ItemId(145))
    );

    let troll_pos1 = repo
        .hero_item_stats(HeroId(95), Role::Carry, Bracket::Ancient)
        .unwrap();
    assert!(
        troll_pos1
            .iter()
            .any(|s| s.item_id == ItemId(145) && s.phase == ItemPhase::Core)
    );
    assert!(
        troll_pos1
            .iter()
            .all(|s| s.item_id != ItemId(30) && s.item_id != ItemId(90))
    );
}

#[test]
fn stratz_item_query_omits_starting_time_and_aliases_all_four_brackets() {
    use crate::sync::stratz_hero_item_purchases_graphql_payload_for_role;
    use truesight_core::domain::Role;

    let payload = stratz_hero_item_purchases_graphql_payload_for_role(32, Some(Role::Mid));
    for expected in [
        "heroId: 32",
        "positionIds: [POSITION_2]",
        "HERALD_GUARDIAN",
        "CRUSADER_ARCHON",
        "LEGEND_ANCIENT",
        "DIVINE_IMMORTAL",
        "hgStart:",
        "hgBoot:",
        "hgFull:",
        "diStart:",
        "diBoot:",
        "diFull:",
        "matchLimit: 5",
    ] {
        assert!(
            payload.contains(expected),
            "expected {expected:?} in payload: {payload}"
        );
    }
    assert!(
        !payload.contains("wasGiven") || !payload.contains("hgStart: itemStartingPurchase(heroId: 32, positionIds: [POSITION_2], bracketBasicIds: [HERALD_GUARDIAN]) { itemId time"),
        "starting item query must NOT request `time` on HeroItemStartingPurchaseType: {payload}"
    );
    // Explicitly check the field list inside hgStart has no `time`.
    let start_idx = payload.find("hgStart:").unwrap();
    let brace_rel = payload[start_idx..].find('{').unwrap();
    let end_rel = payload[start_idx + brace_rel..].find('}').unwrap();
    let start_fields = &payload[start_idx + brace_rel..=start_idx + brace_rel + end_rel];
    assert!(
        !start_fields.contains("time"),
        "itemStartingPurchase must omit `time`, got: {start_fields}"
    );
}

#[test]
fn parse_stratz_live_bracketed_item_purchases_handles_minute_buckets_and_boot_time_sums() {
    use crate::sync::parse_stratz_live_bracketed_item_purchases_json;
    use truesight_core::domain::{HeroId, ItemId, ItemPhase, Role};

    let live_payload = r#"{
        "data": {
            "heroStats": {
                "hgStart": [
                    {"itemId": 20, "matchCount": 1800, "winCount": 950, "wasGiven": false},
                    {"itemId": 44, "matchCount": 900, "winCount": 460, "wasGiven": true}
                ],
                "hgBoot": [
                    {"itemId": 63, "time": 2860000, "matchCount": 3700, "winCount": 1950}
                ],
                "hgFull": [
                    {"itemId": 174, "time": 18, "matchCount": 2000, "winCount": 1060},
                    {"itemId": 147, "time": 28, "matchCount": 1400, "winCount": 780},
                    {"itemId": 108, "time": 34, "matchCount": 900, "winCount": 520}
                ],
                "caStart": [{"itemId": 14, "matchCount": 800, "winCount": 420, "wasGiven": false}],
                "caBoot": [{"itemId": 63, "time": 540000, "matchCount": 900, "winCount": 490}],
                "caFull": [{"itemId": 174, "time": 16, "matchCount": 1500, "winCount": 820}],
                "laStart": [{"itemId": 14, "matchCount": 1200, "winCount": 650, "wasGiven": false}],
                "laBoot": [{"itemId": 63, "time": 24011080, "matchCount": 42287, "winCount": 25455}],
                "laFull": [
                    {"itemId": 174, "time": 14, "matchCount": 4200, "winCount": 2500},
                    {"itemId": 174, "time": 16, "matchCount": 1800, "winCount": 1020},
                    {"itemId": 1097, "time": 32, "matchCount": 1900, "winCount": 1250}
                ],
                "diStart": [{"itemId": 14, "matchCount": 500, "winCount": 280, "wasGiven": false}],
                "diBoot": [{"itemId": 63, "time": 7932913, "matchCount": 14398, "winCount": 8331}],
                "diFull": [
                    {"itemId": 174, "time": 13, "matchCount": 1600, "winCount": 940},
                    {"itemId": 143, "time": 29, "matchCount": 1100, "winCount": 710}
                ]
            }
        }
    }"#;

    let parsed =
        parse_stratz_live_bracketed_item_purchases_json(live_payload, HeroId(32), Some(Role::Mid));
    assert_eq!(
        parsed.len(),
        4,
        "expected all 4 STRATZ bracket buckets: {parsed:?}"
    );

    let la = &parsed
        .iter()
        .find(|(_, _, b, _)| b.map(|v| v.cache_key()) == Some("LEGEND_ANCIENT"))
        .expect("LEGEND_ANCIENT bucket present")
        .3;
    let la_boot = la.iter().find(|s| s.item_id == ItemId(63)).unwrap();
    // 24011080 / 42287 / 60 = 9.463m
    assert!(
        (la_boot.avg_time_mins - 9.46).abs() < 0.05,
        "boot mins: {}",
        la_boot.avg_time_mins
    );
    assert_eq!(la_boot.phase, ItemPhase::Early);

    let la_diff = la.iter().find(|s| s.item_id == ItemId(174)).unwrap();
    // Weighted minutes: (4200*14 + 1800*16)/6000 = 14.6m
    assert!(
        (la_diff.avg_time_mins - 14.6).abs() < 0.05,
        "diff mins: {}",
        la_diff.avg_time_mins
    );
    assert_eq!(la_diff.match_count, 6000);
    assert_eq!(la_diff.phase, ItemPhase::Core);

    let hg = &parsed
        .iter()
        .find(|(_, _, b, _)| b.map(|v| v.cache_key()) == Some("HERALD_GUARDIAN"))
        .expect("HERALD_GUARDIAN bucket present")
        .3;
    assert!(
        hg.iter()
            .any(|s| s.item_id == ItemId(20) && s.phase == ItemPhase::Starting)
    );
    assert!(!hg.iter().any(|s| s.item_id == ItemId(44)));
    assert!(
        hg.iter()
            .any(|s| s.item_id == ItemId(108) && s.phase == ItemPhase::Late)
    );
}

#[test]
fn hero_item_stats_v2_cache_roundtrips_role_bracket_and_scales_unbracketed_fallback() {
    use crate::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role};
    use truesight_core::ports::ItemMetaRepository;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_v2_item_cache_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.stratz_api_token = Some("eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string());
    mgr.set_target_item_hero_roles(vec![(HeroId(32), Role::Mid)]);
    assert_eq!(mgr.target_item_hero_ids, vec![HeroId(32)]);
    assert!(mgr.is_target_item_heroes_uncached());

    let now = 1_795_000_000_u64;
    let hero_stats_json = r#"[{"id": 32, "4_pick": 1000, "4_win": 530}]"#;
    let item_constants_json = r#"{
        "diffusal_blade": {"id": 174, "dname": "Diffusal Blade", "cost": 2500, "components": ["blade_of_alacrity", "robe"]},
        "disperser": {"id": 1097, "dname": "Disperser", "cost": 6100, "components": ["diffusal_blade", "eagle"]},
        "manta": {"id": 147, "dname": "Manta Style", "cost": 4550, "components": ["yasha", "diadem"]},
        "phylactery": {"id": 1107, "dname": "Phylactery", "cost": 2600, "components": ["pers", "diadem"]}
    }"#;
    let stratz_aliased = r#"{
        "data": {
            "heroStats": {
                "hgStart": [],
                "hgBoot": [{"itemId": 63, "time": 660000, "matchCount": 1000, "winCount": 510}],
                "hgFull": [
                    {"itemId": 174, "time": 18, "matchCount": 1000, "winCount": 520},
                    {"itemId": 1107, "time": 20, "matchCount": 900, "winCount": 490}
                ],
                "caStart": [],
                "caBoot": [{"itemId": 63, "time": 540000, "matchCount": 1000, "winCount": 520}],
                "caFull": [{"itemId": 174, "time": 16, "matchCount": 1000, "winCount": 530}],
                "laStart": [],
                "laBoot": [{"itemId": 63, "time": 510000, "matchCount": 1000, "winCount": 540}],
                "laFull": [{"itemId": 174, "time": 15, "matchCount": 1000, "winCount": 560}],
                "diStart": [],
                "diBoot": [{"itemId": 63, "time": 420000, "matchCount": 1000, "winCount": 550}],
                "diFull": [
                    {"itemId": 174, "time": 14, "matchCount": 1000, "winCount": 570},
                    {"itemId": 1097, "time": 26, "matchCount": 900, "winCount": 540}
                ]
            }
        }
    }"#;

    mgr.sync_from_sources_with_role_items(
        false,
        now,
        |url| match url {
            "https://api.opendota.com/api/heroStats" => Ok(hero_stats_json.to_string()),
            "https://api.opendota.com/api/constants/items" => Ok(item_constants_json.to_string()),
            other => Err(truesight_core::ports::PortError(format!(
                "unexpected {other}"
            ))),
        },
        |_token| Ok(r#"{"data":{"heroStats":{"matchUp":[]}}}"#.to_string()),
        |_token, hero_id, role| {
            assert_eq!(hero_id, 32);
            assert_eq!(role, Some(Role::Mid));
            Ok(stratz_aliased.to_string())
        },
    )
    .expect("v2 role-bracketed STRATZ sync should succeed");

    assert!(
        !mgr.is_target_item_heroes_uncached(),
        "cached (HeroId(32), Mid) with STRATZ buckets should mark hero+role cached"
    );
    mgr.set_target_item_hero_roles(vec![(HeroId(32), Role::SoftSupport)]);
    assert!(
        mgr.is_target_item_heroes_uncached(),
        "requesting uncached SoftSupport role on same hero must mark uncached"
    );

    let repo = mgr.build_item_repository();
    let herald = repo
        .hero_item_stats(HeroId(32), Role::Mid, Bracket::Herald)
        .unwrap();
    let divine = repo
        .hero_item_stats(HeroId(32), Role::Mid, Bracket::Divine)
        .unwrap();
    let immortal = repo
        .hero_item_stats(HeroId(32), Role::Mid, Bracket::Immortal)
        .unwrap();

    assert!(herald.iter().any(|s| s.item_id == ItemId(1107)));
    assert!(!divine.iter().any(|s| s.item_id == ItemId(1107)));
    assert!(divine.iter().any(|s| s.item_id == ItemId(1097)));

    let div_diff = divine
        .iter()
        .find(|s| s.item_id == ItemId(174))
        .unwrap()
        .avg_time_mins;
    let imm_diff = immortal
        .iter()
        .find(|s| s.item_id == ItemId(174))
        .unwrap()
        .avg_time_mins;
    assert!(
        div_diff > imm_diff,
        "intra-bucket scaling must keep Divine ({div_diff}) > Immortal ({imm_diff})"
    );

    // Unbracketed custom_hero_stats fallback must still scale by bracket_timing_scale.
    let mut unbracketed_repo = crate::item_repo::SeededItemMetaRepository::default();
    unbracketed_repo.custom_hero_stats.push((
        HeroId(99),
        None,
        None,
        vec![truesight_core::domain::HeroItemStat {
            item_id: ItemId(1),
            phase: truesight_core::domain::ItemPhase::Core,
            avg_time_mins: 15.0,
            match_count: 500,
            win_count: 270,
        }],
    ));
    let u_herald = unbracketed_repo
        .hero_item_stats(HeroId(99), Role::Offlane, Bracket::Herald)
        .unwrap();
    let u_legend = unbracketed_repo
        .hero_item_stats(HeroId(99), Role::Offlane, Bracket::Legend)
        .unwrap();
    let u_immortal = unbracketed_repo
        .hero_item_stats(HeroId(99), Role::Offlane, Bracket::Immortal)
        .unwrap();
    let t_herald = u_herald
        .iter()
        .find(|s| s.item_id == ItemId(1))
        .unwrap()
        .avg_time_mins;
    let t_legend = u_legend
        .iter()
        .find(|s| s.item_id == ItemId(1))
        .unwrap()
        .avg_time_mins;
    let t_immortal = u_immortal
        .iter()
        .find(|s| s.item_id == ItemId(1))
        .unwrap()
        .avg_time_mins;
    assert!(
        t_herald > t_legend && t_legend > t_immortal,
        "unbracketed cached fallback must scale Herald ({t_herald}) > Legend ({t_legend}) > Immortal ({t_immortal})"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn is_valid_timed_progression_item_for_phase_allows_early_core_high_value_and_blocks_basic_and_late_raw()
 {
    use crate::item_repo::{SeededItemMetaRepository, is_valid_timed_progression_item_for_phase};
    use truesight_core::domain::{ItemId, ItemPhase};

    let repo = SeededItemMetaRepository::default();
    // High-value Early/Core stepping stones are valid in Core.
    for id in [24_u16, 54, 55, 170, 149] {
        assert!(
            is_valid_timed_progression_item_for_phase(&repo, ItemId(id), ItemPhase::Core),
            "expected item #{id} to be valid in Core"
        );
    }
    // Raw Secret Shop orbs/rel/hyperstone must stay blocked in Late.
    for raw_late_id in [24_u16, 54, 55] {
        assert!(
            !is_valid_timed_progression_item_for_phase(&repo, ItemId(raw_late_id), ItemPhase::Late),
            "expected raw stepping stone #{raw_late_id} to remain excluded in Late"
        );
    }
    // Cheap basic ingredients must stay blocked in Early, Core, and Late.
    for basic_id in [21_u16, 22, 67, 69, 1125] {
        for ph in [ItemPhase::Early, ItemPhase::Core, ItemPhase::Late] {
            assert!(
                !is_valid_timed_progression_item_for_phase(&repo, ItemId(basic_id), ph),
                "expected basic item #{basic_id} to be blocked in {ph:?}"
            );
        }
    }
}

#[test]
fn sync_partial_stratz_buckets_keeps_opendota_fallback_for_uncovered_brackets() {
    use crate::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role};
    use truesight_core::math::recommend_item_build;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_partial_stratz_fallback_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.stratz_api_token = Some("eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string());
    mgr.set_target_item_hero_roles(vec![(HeroId(32), Role::Mid)]);

    // Partial cache with ONLY DIVINE_IMMORTAL and no unbracketed fallback must stay uncached when STRATZ token is active.
    std::fs::write(
        tmp_dir.join("hero_item_stats.json"),
        r#"[{"heroId":32,"role":"Mid","bracket":"DIVINE_IMMORTAL","itemStartingPurchase":[],"itemFullPurchase":[{"itemId":1097,"phase":"Late","timeSecs":1560,"instance":0,"matchCount":900,"winCount":540}]}]"#,
    )
    .unwrap();
    assert!(
        mgr.is_target_item_heroes_uncached(),
        "single-bucket STRATZ cache without fallback must be treated as uncached for uncovered brackets"
    );

    // Start from empty cache and return DI-only from STRATZ alongside valid OpenDota fallback.
    let _ = std::fs::remove_file(tmp_dir.join("hero_item_stats.json"));
    let now = 1_795_000_000_u64;
    let hero_stats_json = r#"[{"id": 32, "4_pick": 1000, "4_win": 530}]"#;
    let item_constants_json = r#"{
        "diffusal_blade": {"id": 174, "dname": "Diffusal Blade", "cost": 2500, "components": ["blade_of_alacrity", "robe"]},
        "disperser": {"id": 1097, "dname": "Disperser", "cost": 6100, "components": ["diffusal_blade", "eagle"]},
        "manta": {"id": 147, "dname": "Manta Style", "cost": 4550, "components": ["yasha", "diadem"]},
        "butterfly": {"id": 139, "dname": "Butterfly", "cost": 4975, "components": ["eagle"]}
    }"#;
    let partial_stratz = r#"{
        "data": {
            "heroStats": {
                "hgStart": [], "hgBoot": [], "hgFull": [],
                "caStart": [], "caBoot": [], "caFull": [],
                "laStart": [], "laBoot": [], "laFull": [],
                "diStart": [],
                "diBoot": [{"itemId": 63, "time": 420000, "matchCount": 1000, "winCount": 550}],
                "diFull": [
                    {"itemId": 174, "time": 14, "matchCount": 1000, "winCount": 570},
                    {"itemId": 1097, "time": 26, "matchCount": 900, "winCount": 540}
                ]
            }
        }
    }"#;
    let opendota_timings = r#"[
        {"hero_id": 32, "item": "diffusal_blade", "time": 960, "games": "1200", "wins": "660"},
        {"hero_id": 32, "item": "manta", "time": 1440, "games": "1100", "wins": "630"},
        {"hero_id": 32, "item": "butterfly", "time": 1920, "games": "800", "wins": "490"}
    ]"#;
    let opendota_popularity = r#"{"start_game_items":{"44":900},"early_game_items":{"63":1200},"mid_game_items":{},"late_game_items":{}}"#;

    let mut timings_calls = 0_usize;
    mgr.sync_from_sources_with_role_items(
        false,
        now,
        |url| match url {
            "https://api.opendota.com/api/heroStats" => Ok(hero_stats_json.to_string()),
            "https://api.opendota.com/api/constants/items" => Ok(item_constants_json.to_string()),
            "https://api.opendota.com/api/scenarios/itemTimings?hero_id=32" => {
                timings_calls += 1;
                Ok(opendota_timings.to_string())
            }
            "https://api.opendota.com/api/heroes/32/itemPopularity" => {
                Ok(opendota_popularity.to_string())
            }
            other => Err(truesight_core::ports::PortError(format!(
                "unexpected URL {other}"
            ))),
        },
        |_token| Ok(r#"{"data":{"heroStats":{"matchUp":[]}}}"#.to_string()),
        |_token, hero_id, role| {
            assert_eq!(hero_id, 32);
            assert_eq!(role, Some(Role::Mid));
            Ok(partial_stratz.to_string())
        },
    )
    .expect("partial STRATZ sync with OpenDota fallback should succeed");

    assert!(
        timings_calls >= 1,
        "partial STRATZ response leaving uncovered buckets must still fetch OpenDota fallback"
    );
    assert!(
        !mgr.is_target_item_heroes_uncached(),
        "once partial STRATZ buckets + OpenDota fallback are persisted, resync check must converge"
    );

    let repo = mgr.build_item_repository();
    let enemies = [None; 5];
    let divine_build = recommend_item_build(
        &repo,
        HeroId(32),
        Role::Mid,
        Bracket::Divine,
        &enemies,
        &[],
        300,
    )
    .unwrap();
    let herald_build = recommend_item_build(
        &repo,
        HeroId(32),
        Role::Mid,
        Bracket::Herald,
        &enemies,
        &[],
        300,
    )
    .unwrap();

    assert!(
        divine_build.steps.iter().any(|s| s.item_id == ItemId(1097))
            && divine_build.steps.iter().all(|s| s.item_id != ItemId(139)),
        "covered Divine bracket must use exact STRATZ DI data (Disperser 1097): {:?}",
        divine_build.steps
    );
    assert!(
        herald_build.steps.iter().any(|s| s.item_id == ItemId(147))
            && herald_build.steps.iter().any(|s| s.item_id == ItemId(139))
            && herald_build.steps.iter().all(|s| s.item_id != ItemId(1097)),
        "uncovered Herald bracket must use OpenDota fallback (Manta 147 / Butterfly 139) rather than embedded snapshot: {:?}",
        herald_build.steps
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn hero_item_stats_v2_cache_preserves_early_boot_phase_and_support_core_phase_on_roundtrip() {
    use crate::sync::{LocalCacheManager, parse_stratz_live_bracketed_item_purchases_json};
    use truesight_core::domain::{Bracket, HeroId, ItemId, ItemPhase, Role};
    use truesight_core::math::recommend_item_build;
    use truesight_core::ports::ItemMetaRepository;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_v2_phase_roundtrip_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.stratz_api_token = Some("eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string());
    mgr.set_target_item_hero_roles(vec![(HeroId(79), Role::SoftSupport)]);

    let stratz_payload = r#"{
        "data": {
            "heroStats": {
                "hgStart": [],
                "hgBoot": [{"itemId": 48, "time": 780000, "matchCount": 1000, "winCount": 560}],
                "hgFull": [
                    {"itemId": 1, "time": 21, "matchCount": 1200, "winCount": 670},
                    {"itemId": 232, "time": 26, "matchCount": 1100, "winCount": 610},
                    {"itemId": 254, "time": 27, "matchCount": 950, "winCount": 510},
                    {"itemId": 100, "time": 32, "matchCount": 800, "winCount": 460}
                ],
                "caStart": [], "caBoot": [], "caFull": [],
                "laStart": [],
                "laBoot": [{"itemId": 48, "time": 780000, "matchCount": 1000, "winCount": 560}],
                "laFull": [
                    {"itemId": 1, "time": 21, "matchCount": 1200, "winCount": 670},
                    {"itemId": 232, "time": 26, "matchCount": 1100, "winCount": 610},
                    {"itemId": 254, "time": 27, "matchCount": 950, "winCount": 510},
                    {"itemId": 100, "time": 32, "matchCount": 800, "winCount": 460}
                ],
                "diStart": [], "diBoot": [], "diFull": []
            }
        }
    }"#;

    let live_entries = parse_stratz_live_bracketed_item_purchases_json(
        stratz_payload,
        HeroId(79),
        Some(Role::SoftSupport),
    );
    let live_la = &live_entries
        .iter()
        .find(|(_, _, b, _)| b.map(|v| v.cache_key()) == Some("LEGEND_ANCIENT"))
        .unwrap()
        .3;
    let live_bot = live_la.iter().find(|s| s.item_id == ItemId(48)).unwrap();
    assert_eq!(live_bot.phase, ItemPhase::Early);
    let live_lens = live_la.iter().find(|s| s.item_id == ItemId(232)).unwrap();
    assert_eq!(
        live_lens.phase,
        ItemPhase::Core,
        "26m item on SoftSupport must be classified as Core (<= 28.5m)"
    );

    let now = 1_795_000_000_u64;
    let hero_stats_json = r#"[{"id": 79, "4_pick": 1000, "4_win": 520}]"#;
    let item_constants_json = r#"{
        "blink": {"id": 1, "dname": "Blink Dagger", "cost": 2250, "components": null},
        "aether_lens": {"id": 232, "dname": "Aether Lens", "cost": 2100, "components": ["void_stone", "energy_booster"]},
        "glimmer_cape": {"id": 254, "dname": "Glimmer Cape", "cost": 2150, "components": ["shadow_amulet", "cloak"]},
        "cyclone": {"id": 100, "dname": "Eul's Scepter of Divinity", "cost": 2600, "components": ["staff_of_wizardry", "void_stone", "wind_lace"]}
    }"#;

    mgr.sync_from_sources_with_role_items(
        false,
        now,
        |url| match url {
            "https://api.opendota.com/api/heroStats" => Ok(hero_stats_json.to_string()),
            "https://api.opendota.com/api/constants/items" => Ok(item_constants_json.to_string()),
            _ => Ok("[]".to_string()),
        },
        |_token| Ok(r#"{"data":{"heroStats":{"matchUp":[]}}}"#.to_string()),
        |_token, _hero_id, _role| Ok(stratz_payload.to_string()),
    )
    .expect("v2 phase sync should succeed");

    let repo = mgr.build_item_repository();
    let cached_stats = repo
        .hero_item_stats(HeroId(79), Role::SoftSupport, Bracket::Legend)
        .unwrap();
    let cached_bot = cached_stats
        .iter()
        .find(|s| s.item_id == ItemId(48))
        .expect("Boots of Travel present after reload");
    assert_eq!(
        cached_bot.phase,
        ItemPhase::Early,
        "13m STRATZ itemBootPurchase row for Boots of Travel must preserve Early phase after v2 cache roundtrip"
    );
    let cached_lens = cached_stats
        .iter()
        .find(|s| s.item_id == ItemId(232))
        .expect("Aether Lens present after reload");
    assert_eq!(
        cached_lens.phase,
        ItemPhase::Core,
        "26m Support Core item must preserve Core phase after v2 cache roundtrip"
    );

    let enemies = [None; 5];
    let build = recommend_item_build(
        &repo,
        HeroId(79),
        Role::SoftSupport,
        Bracket::Legend,
        &enemies,
        &[],
        300,
    )
    .unwrap();
    let bot_step = build
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(48))
        .expect("Boots of Travel step must be recommended");
    assert_eq!(
        bot_step.phase,
        ItemPhase::Early,
        "Boots of Travel must stay in Early rather than competing for Core: {:?}",
        build.steps
    );
    assert_eq!(
        build
            .steps
            .iter()
            .filter(|s| s.phase == ItemPhase::Core)
            .count(),
        2,
        "Core should hold Blink + Aether Lens without Boots of Travel competing: {:?}",
        build.steps
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn sync_from_sources_with_role_items_publishes_stratz_items_when_opendota_player_or_meta_fails() {
    use crate::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role};
    use truesight_core::ports::{ItemMetaRepository, PortError};

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_partial_opendota_fail_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let now = 1_795_000_000_u64;
    let valid_token = "eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string();
    let sample_stratz = |full_item_id: u16| {
        format!(
            r#"{{
                "data": {{
                    "heroStats": {{
                        "hgStart": [{{"itemId": 44, "instance": 0, "matchCount": 800, "winCount": 410, "wasGiven": false}}],
                        "hgBoot": [{{"itemId": 63, "time": 540000, "matchCount": 900, "winCount": 460}}],
                        "hgFull": [{{"itemId": {full_item_id}, "time": 16, "matchCount": 900, "winCount": 510}}],
                        "caStart": [{{"itemId": 44, "instance": 0, "matchCount": 800, "winCount": 410, "wasGiven": false}}],
                        "caBoot": [{{"itemId": 63, "time": 540000, "matchCount": 900, "winCount": 460}}],
                        "caFull": [{{"itemId": {full_item_id}, "time": 16, "matchCount": 900, "winCount": 520}}],
                        "laStart": [{{"itemId": 44, "instance": 0, "matchCount": 800, "winCount": 410, "wasGiven": false}}],
                        "laBoot": [{{"itemId": 63, "time": 540000, "matchCount": 900, "winCount": 460}}],
                        "laFull": [{{"itemId": {full_item_id}, "time": 15, "matchCount": 900, "winCount": 540}}],
                        "diStart": [{{"itemId": 44, "instance": 0, "matchCount": 800, "winCount": 410, "wasGiven": false}}],
                        "diBoot": [{{"itemId": 63, "time": 540000, "matchCount": 900, "winCount": 460}}],
                        "diFull": [{{"itemId": {full_item_id}, "time": 14, "matchCount": 900, "winCount": 550}}]
                    }}
                }}
            }}"#
        )
    };

    // Case A: Meta is fresh (<24h), linked player comfort is stale (>1h), OpenDota player endpoint times out.
    // STRATZ item builds for Invoker (74, SoftSupport) MUST still be published!
    let item_constants_json =
        r#"{"blink":{"id":1,"dname":"Blink Dagger","cost":2250,"components":null}}"#;
    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), Some(127_255_741));
    mgr.stratz_api_token = Some(valid_token.clone());
    mgr.publish_staged_cache_with_items(
        Some(r#"[{"id": 74, "5_pick": 1000, "5_win": 520}]"#),
        None,
        Some(item_constants_json),
        None,
        Some("[]"),
        None,
        None,
        now - 7_200, // 2h ago: Meta fresh (<24h), Player stale (>1h)
    )
    .unwrap();
    mgr.set_target_item_hero_roles(vec![(HeroId(74), Role::SoftSupport)]);

    let msg_a = mgr
        .sync_from_sources_with_role_items(
            false,
            now,
            |url| Err(PortError(format!("OpenDota timeout on {url}"))),
            |_token| Err(PortError("unexpected stratz syn".into())),
            |_token, hero_id, role| {
                assert_eq!(hero_id, 74);
                assert_eq!(role, Some(Role::SoftSupport));
                Ok(sample_stratz(267)) // Spirit Vessel
            },
        )
        .expect("STRATZ item builds must persist even when OpenDota player endpoint times out");
    assert!(
        msg_a.contains("itemBuilds"),
        "sync result must mention persisted itemBuilds: {msg_a}"
    );
    assert!(
        !mgr.is_target_item_heroes_uncached(),
        "Invoker Pos 4 must now be cached in hero_item_stats.json"
    );
    let repo_a = mgr.build_item_repository();
    let invoker_stats = repo_a
        .hero_item_stats(HeroId(74), Role::SoftSupport, Bracket::Ancient)
        .expect("Invoker Pos 4 Ancient STRATZ stats must be readable from cache");
    assert!(
        invoker_stats.iter().any(|s| s.item_id == ItemId(267)),
        "Spirit Vessel (267) from STRATZ must be persisted: {invoker_stats:?}"
    );

    // Case B: Meta is stale (>24h) and OpenDota /heroStats times out, while STRATZ itemBuilds for
    // Juggernaut (8, Carry) succeeds -> item builds persist and existing meta cache remains intact.
    std::fs::write(
        tmp_dir.join("cache_metadata.txt"),
        format!("meta_sync_unix={}\n", now - 100_000),
    )
    .unwrap();
    mgr.set_target_item_hero_roles(vec![(HeroId(8), Role::Carry)]);
    mgr.sync_from_sources_with_role_items(
        false,
        now,
        |url| Err(PortError(format!("OpenDota timeout on {url}"))),
        |_token| Ok(r#"{"data":{"heroStats":{"matchUp":[]}}}"#.to_string()),
        |_token, hero_id, role| {
            assert_eq!(hero_id, 8);
            assert_eq!(role, Some(Role::Carry));
            Ok(sample_stratz(166)) // Maelstrom
        },
    )
    .expect("STRATZ item builds must persist even when OpenDota heroStats times out");

    let repo_b = mgr.build_item_repository();
    let jugg_stats = repo_b
        .hero_item_stats(HeroId(8), Role::Carry, Bracket::Ancient)
        .expect("Juggernaut Carry STRATZ stats must be persisted");
    assert!(jugg_stats.iter().any(|s| s.item_id == ItemId(166)));
    assert_eq!(
        std::fs::read_to_string(tmp_dir.join("opendota_hero_stats.json")).unwrap(),
        r#"[{"id": 74, "5_pick": 1000, "5_win": 520}]"#,
        "existing opendota_hero_stats.json must stay intact when /heroStats times out"
    );

    // Case C: Total outage (stale meta, no STRATZ token, every OpenDota endpoint fails) still returns Err.
    let mut offline_mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    offline_mgr.set_target_item_hero_roles(vec![(HeroId(2), Role::Offlane)]);
    let err_c = offline_mgr.sync_from_sources_with_role_items(
        false,
        now,
        |url| Err(PortError(format!("offline on {url}"))),
        |_token| Err(PortError("offline".into())),
        |_token, _hid, _role| Err(PortError("offline".into())),
    );
    assert!(
        err_c.is_err(),
        "total network outage with zero staged artifacts must still return Err: {err_c:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn sync_from_sources_with_role_items_skips_already_cached_hero_roles_when_not_forced() {
    use crate::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role};
    use truesight_core::ports::{ItemMetaRepository, PortError};

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_skip_cached_hero_roles_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let now = 1_795_000_000_u64;
    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.stratz_api_token = Some("eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string());
    let item_constants_json =
        r#"{"dragon_lance":{"id":236,"dname":"Dragon Lance","cost":1900,"components":null}}"#;
    mgr.publish_staged_cache_with_items(
        Some(r#"[{"id": 46, "5_pick": 1000, "5_win": 520}, {"id": 74, "5_pick": 1000, "5_win": 510}]"#),
        None,
        Some(item_constants_json),
        Some(concat!(
            r#"[{"heroId":46,"role":"Carry","bracket":"HERALD_GUARDIAN","itemStartingPurchase":[],"itemFullPurchase":[{"itemId":236,"phase":"Core","timeSecs":960,"instance":0,"matchCount":800,"winCount":430}]},"#,
            r#"{"heroId":46,"role":"Carry","bracket":"CRUSADER_ARCHON","itemStartingPurchase":[],"itemFullPurchase":[{"itemId":236,"phase":"Core","timeSecs":900,"instance":0,"matchCount":800,"winCount":440}]},"#,
            r#"{"heroId":46,"role":"Carry","bracket":"LEGEND_ANCIENT","itemStartingPurchase":[],"itemFullPurchase":[{"itemId":236,"phase":"Core","timeSecs":840,"instance":0,"matchCount":800,"winCount":450}]},"#,
            r#"{"heroId":46,"role":"Carry","bracket":"DIVINE_IMMORTAL","itemStartingPurchase":[],"itemFullPurchase":[{"itemId":236,"phase":"Core","timeSecs":780,"instance":0,"matchCount":800,"winCount":460}]}]"#
        )),
        None,
        None,
        None,
        now - 100, // Meta fresh (<24h)
    )
    .unwrap();

    mgr.set_target_item_hero_roles(vec![
        (HeroId(46), Role::Carry),
        (HeroId(74), Role::SoftSupport),
    ]);

    let make_invoker_stratz = || {
        r#"{
            "data": {
                "heroStats": {
                    "hgStart": [], "hgBoot": [], "hgFull": [{"itemId": 267, "time": 16, "matchCount": 900, "winCount": 510}],
                    "caStart": [], "caBoot": [], "caFull": [{"itemId": 267, "time": 16, "matchCount": 900, "winCount": 510}],
                    "laStart": [], "laBoot": [], "laFull": [{"itemId": 267, "time": 15, "matchCount": 900, "winCount": 520}],
                    "diStart": [], "diBoot": [], "diFull": [{"itemId": 267, "time": 14, "matchCount": 900, "winCount": 530}]
                }
            }
        }"#.to_string()
    };

    let mut fetched = Vec::new();
    mgr.sync_from_sources_with_role_items(
        false,
        now,
        |url| Err(PortError(format!("unexpected OpenDota URL {url}"))),
        |_token| Err(PortError("unexpected stratz syn".into())),
        |_token, hero_id, role| {
            fetched.push((hero_id, role));
            Ok(make_invoker_stratz())
        },
    )
    .expect("incremental uncached item sync must succeed");

    assert_eq!(
        fetched,
        vec![(74, Some(Role::SoftSupport))],
        "already-cached TA (46, Carry) must be skipped when not forced"
    );

    let repo = mgr.build_item_repository();
    assert!(
        repo.hero_item_stats(HeroId(46), Role::Carry, Bracket::Ancient)
            .is_ok_and(|s| s.iter().any(|x| x.item_id == ItemId(236))),
        "previously cached TA (46, Carry) must be retained after merge"
    );
    assert!(
        repo.hero_item_stats(HeroId(74), Role::SoftSupport, Bracket::Ancient)
            .is_ok_and(|s| s.iter().any(|x| x.item_id == ItemId(267))),
        "newly fetched Invoker (74, SoftSupport) must be added to cache"
    );

    // Forced sync (`force = true`) must still refresh all requested target hero-roles.
    let mut forced_fetched = Vec::new();
    mgr.sync_from_sources_with_role_items(
        true,
        now,
        |_url| Ok("[]".to_string()),
        |_token| Ok(r#"{"data":{"heroStats":{"matchUp":[]}}}"#.to_string()),
        |_token, hero_id, role| {
            forced_fetched.push((hero_id, role));
            Ok(make_invoker_stratz())
        },
    )
    .expect("forced refresh must succeed");
    assert_eq!(
        forced_fetched,
        vec![(46, Some(Role::Carry)), (74, Some(Role::SoftSupport))],
        "forced sync must re-fetch every target hero-role"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn sync_fetches_item_constants_independently_of_hero_stats_and_keeps_meta_retryable() {
    use crate::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role};
    use truesight_core::ports::{ItemMetaRepository, PortError};

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_independent_constants_sync_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let now = 1_795_000_000_u64;
    let valid_token = "eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string();
    let item_constants_json = r#"{
        "broadsword": {"id": 3, "dname": "Broadsword", "cost": 1000, "components": null},
        "new_patch_blade": {"id": 9999, "dname": "New Patch Blade", "cost": 3200, "components": ["broadsword"]}
    }"#;
    let stratz_new_item = r#"{
        "data": {
            "heroStats": {
                "hgStart": [], "hgBoot": [], "hgFull": [{"itemId": 9999, "time": 18, "matchCount": 900, "winCount": 510}],
                "caStart": [], "caBoot": [], "caFull": [{"itemId": 9999, "time": 18, "matchCount": 900, "winCount": 510}],
                "laStart": [], "laBoot": [], "laFull": [{"itemId": 9999, "time": 18, "matchCount": 900, "winCount": 510}],
                "diStart": [], "diBoot": [], "diFull": [{"itemId": 9999, "time": 18, "matchCount": 900, "winCount": 510}]
            }
        }
    }"#;

    // 1. Empty cache: /heroStats times out, /constants/items returns an item absent from the
    // embedded catalog (#9999), and STRATZ returns timings for #9999.
    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.stratz_api_token = Some(valid_token);
    mgr.set_target_item_hero_roles(vec![(HeroId(74), Role::SoftSupport)]);

    let mut constants_calls = 0_usize;
    let msg = mgr
        .sync_from_sources_with_role_items(
            false,
            now,
            |url| match url {
                "https://api.opendota.com/api/constants/items" => {
                    constants_calls += 1;
                    Ok(item_constants_json.to_string())
                }
                other => Err(PortError(format!("OpenDota timeout on {other}"))),
            },
            |_token| Err(PortError("no syn needed".into())),
            |_token, hero_id, role| {
                assert_eq!(hero_id, 74);
                assert_eq!(role, Some(Role::SoftSupport));
                Ok(stratz_new_item.to_string())
            },
        )
        .expect("constants + STRATZ item builds must persist when heroStats times out");

    assert_eq!(
        constants_calls, 1,
        "item constants must still be fetched even when heroStats times out"
    );
    assert!(
        msg.contains("itemConstants") && msg.contains("itemBuilds") && msg.contains("partial:"),
        "sync message must report itemConstants + itemBuilds + partial failure: {msg}"
    );
    assert!(
        tmp_dir.join("hero_item_stats.json").exists(),
        "hero_item_stats.json must be written when new catalog item #9999 comes from /constants/items"
    );
    let repo = mgr.build_item_repository();
    let stats = repo
        .hero_item_stats(HeroId(74), Role::SoftSupport, Bracket::Ancient)
        .expect("cached STRATZ stats must be readable");
    assert!(
        stats.iter().any(|s| s.item_id == ItemId(9999)),
        "ItemId(9999) from /constants/items must survive filtering and load from cache: {stats:?}"
    );
    assert!(
        mgr.is_meta_stale(now),
        "meta must stay stale when heroStats failed on empty cache"
    );

    // 2. Existing stale opendota_hero_stats.json (>24h): catalog-only success MUST NOT bump
    // meta_sync_unix and mask the failed heroStats from future retries.
    std::fs::write(
        tmp_dir.join("opendota_hero_stats.json"),
        r#"[{"id": 74, "5_pick": 1000, "5_win": 520}]"#,
    )
    .unwrap();
    std::fs::write(
        tmp_dir.join("cache_metadata.txt"),
        format!("meta_sync_unix={}\n", now - 100_000),
    )
    .unwrap();

    mgr.sync_from_sources_with_role_items(
        false,
        now,
        |url| match url {
            "https://api.opendota.com/api/constants/items" => Ok(item_constants_json.to_string()),
            other => Err(PortError(format!("OpenDota timeout on {other}"))),
        },
        |_token| Err(PortError("no syn needed".into())),
        |_token, _hid, _role| Ok(stratz_new_item.to_string()),
    )
    .expect("catalog-only refresh with failed heroStats should succeed");
    assert!(
        mgr.is_meta_stale(now),
        "catalog-only success must keep stale heroStats eligible for retry instead of bumping meta_sync_unix"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn sync_validates_player_comfort_before_staging_so_malformed_player_does_not_discard_stratz_items()
{
    use crate::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role};
    use truesight_core::ports::ItemMetaRepository;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_malformed_player_item_preserve_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let now = 1_795_000_000_u64;
    let valid_token = "eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string();
    let item_constants_json =
        r#"{"blink":{"id":1,"dname":"Blink Dagger","cost":2250,"components":null}}"#;
    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), Some(127_255_741));
    mgr.stratz_api_token = Some(valid_token);
    mgr.publish_staged_cache_with_items(
        Some(r#"[{"id": 74, "5_pick": 1000, "5_win": 520}]"#),
        None,
        Some(item_constants_json),
        None,
        Some("[]"),
        None,
        None,
        now - 7_200, // Meta fresh (<24h), player comfort stale (>1h)
    )
    .unwrap();
    mgr.set_target_item_hero_roles(vec![(HeroId(74), Role::SoftSupport)]);

    let stratz_invoker = r#"{
        "data": {
            "heroStats": {
                "hgStart": [], "hgBoot": [], "hgFull": [{"itemId": 267, "time": 16, "matchCount": 900, "winCount": 510}],
                "caStart": [], "caBoot": [], "caFull": [{"itemId": 267, "time": 16, "matchCount": 900, "winCount": 520}],
                "laStart": [], "laBoot": [], "laFull": [{"itemId": 267, "time": 15, "matchCount": 900, "winCount": 530}],
                "diStart": [], "diBoot": [], "diFull": [{"itemId": 267, "time": 14, "matchCount": 900, "winCount": 540}]
            }
        }
    }"#;

    // 1. OpenDota /players/{id}/heroes returns malformed `[{}]` while STRATZ returns valid item builds.
    let msg = mgr
        .sync_from_sources_with_role_items(
            false,
            now,
            |url| match url {
                "https://api.opendota.com/api/players/127255741/heroes" => Ok("[{}]".to_string()),
                other => Err(truesight_core::ports::PortError(format!(
                    "unexpected OpenDota URL {other}"
                ))),
            },
            |_token| Err(truesight_core::ports::PortError("no syn".into())),
            |_token, hero_id, role| {
                assert_eq!(hero_id, 74);
                assert_eq!(role, Some(Role::SoftSupport));
                Ok(stratz_invoker.to_string())
            },
        )
        .expect(
            "malformed OpenDota player comfort `[{}]` must not discard valid STRATZ itemBuilds",
        );

    assert!(
        msg.contains("itemBuilds") && msg.contains("partial:") && msg.contains("player"),
        "sync message must note persisted itemBuilds and partial player failure: {msg}"
    );
    assert!(
        tmp_dir.join("hero_item_stats.json").exists(),
        "hero_item_stats.json must still be written despite malformed player comfort `[{{}}]`"
    );
    let repo = mgr.build_item_repository();
    let invoker_stats = repo
        .hero_item_stats(HeroId(74), Role::SoftSupport, Bracket::Ancient)
        .expect("Invoker Pos 4 STRATZ stats must be present");
    assert!(invoker_stats.iter().any(|s| s.item_id == ItemId(267)));
    assert!(!mgr.is_target_item_heroes_uncached());
    assert!(
        mgr.is_player_stale(now),
        "malformed player comfort must not be written or mark player cache fresh"
    );

    // 2. Subsequent player-only sync (now all items are cached) with `[{}]` still returns Err.
    let err = mgr.sync_from_sources_with_role_items(
        false,
        now,
        |url| match url {
            "https://api.opendota.com/api/players/127255741/heroes" => Ok("[{}]".to_string()),
            other => Err(truesight_core::ports::PortError(format!(
                "unexpected OpenDota URL {other}"
            ))),
        },
        |_token| Err(truesight_core::ports::PortError("no syn".into())),
        |_token, _hid, _role| Ok(stratz_invoker.to_string()),
    );
    assert!(
        err.is_err(),
        "player-only sync with malformed `[{{}}]` and zero staged artifacts must return Err: {err:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn pickphase_topbar_excludes_player_color_strip_and_recovers_dota_plus_badged_portraits() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter,
        compute_pickphase_topbar_slot_rects, parse_hero_atlas, pickphase_upper_core_rect,
    };
    use std::collections::VecDeque;
    use truesight_core::ports::{DraftEvent, DraftEventSource};

    struct QueueSource(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.pop_front())
        }
    }

    let atlas = parse_hero_atlas().expect("embedded hero atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");

    // Verify the upper-core inset preserves x/w and restricts height cleanly above bottom badges.
    let probe_rect = compute_pickphase_topbar_slot_rects(2560, 1440).0[0];
    let upper_core = pickphase_upper_core_rect(probe_rect);
    assert!(
        probe_rect.y >= 8,
        "PickPhase rect must skip y=0..7 player color strip"
    );
    assert_eq!(upper_core.x, probe_rect.x);
    assert_eq!(upper_core.y, probe_rect.y);
    assert!(upper_core.height < probe_rect.height);

    let player_colors: [[u8; 3]; 10] = [
        [52, 118, 255],
        [102, 255, 192],
        [191, 0, 191],
        [243, 240, 11],
        [255, 107, 0],
        [254, 134, 194],
        [161, 180, 71],
        [101, 217, 247],
        [0, 131, 33],
        [164, 105, 0],
    ];
    let rad_ids = [94_u16, 16, 48, 3, 31];
    let dire_ids = [111_u16, 43, 30, 26, 37];

    for (fw, vh, strip_h) in [
        (2560_usize, 1440_usize, 116_usize),
        (1920_usize, 1080_usize, 87_usize),
    ] {
        let sx = fw as f32 / 2560.0;
        let sy = vh as f32 / 1440.0;
        let mut rgb = vec![16_u8; fw * strip_h * 3];

        let paint_pick_slot =
            |buf: &mut [u8], slot_x: f32, hid: u16, color: [u8; 3], paint_bottom_badge: bool| {
                let bar_x0 = (slot_x * sx).round() as usize;
                let bar_w = (143.0 * sx).round() as usize;
                let bar_h = (8.0 * sy).round().max(1.0) as usize;
                for py in 0..bar_h {
                    for px in bar_x0..(bar_x0 + bar_w).min(fw) {
                        let idx = (py * fw + px) * 3;
                        buf[idx..idx + 3].copy_from_slice(&color);
                    }
                }
                let card_y0 = (8.0 * sy).round() as usize;
                let card_h = (84.0 * sy).round().max(1.0) as usize;
                let hero_rgb = &atlas.heroes[&HeroId(hid)];
                for dy in 0..card_h {
                    let sy_src = (dy * atlas.hero_height) / card_h;
                    for dx in 0..bar_w {
                        let sx_src = (dx * atlas.hero_width) / bar_w;
                        let s = (sy_src * atlas.hero_width + sx_src) * 3;
                        let px = (bar_x0 + dx).min(fw - 1);
                        let py = (card_y0 + dy).min(strip_h - 1);
                        let d = (py * fw + px) * 3;
                        buf[d..d + 3].copy_from_slice(&hero_rgb[s..s + 3]);
                    }
                }
                if paint_bottom_badge {
                    let badge_y0 = card_y0 + (card_h * 22) / 36;
                    for py in badge_y0..(card_y0 + card_h).min(strip_h) {
                        for px in bar_x0..(bar_x0 + bar_w).min(fw) {
                            let idx = (py * fw + px) * 3;
                            buf[idx] = 175;
                            buf[idx + 1] = 105;
                            buf[idx + 2] = 42;
                        }
                    }
                }
            };

        for (i, &hid) in rad_ids.iter().enumerate() {
            paint_pick_slot(
                &mut rgb,
                280.0 + (i as f32) * 165.0,
                hid,
                player_colors[i],
                i < 2,
            );
        }
        for (i, &hid) in dire_ids.iter().enumerate() {
            paint_pick_slot(
                &mut rgb,
                1475.0 + (i as f32) * 165.0,
                hid,
                player_colors[5 + i],
                i == 0,
            );
        }

        let frames = VecDeque::from([
            RgbFrame::with_source_height(fw, strip_h, vh, rgb.clone()),
            RgbFrame::with_source_height(fw, strip_h, vh, rgb),
        ]);
        let mut adapter = VisionCaptureAdapter::new(QueueSource(frames), catalog.clone());
        let mut seen = Vec::new();
        for _ in 0..2 {
            seen.extend(adapter.poll_events().unwrap());
        }
        let picked_ids: Vec<HeroId> = seen
            .iter()
            .filter_map(|e| match e {
                DraftEvent::PickDetected { hero_id, .. } => Some(*hero_id),
                _ => None,
            })
            .collect();
        for expected in rad_ids.iter().chain(dire_ids.iter()) {
            assert!(
                picked_ids.contains(&HeroId(*expected)),
                "PickPhase at {fw}x{vh} missed hero #{expected} (got {picked_ids:?})"
            );
        }
        assert_eq!(picked_ids.len(), 10);
    }
}

#[test]
fn pickphase_ranked_2_2_1_blind_pick_waves_lock_all_picks_without_empty_slot_phantoms() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter, parse_hero_atlas,
    };
    use std::collections::VecDeque;
    use truesight_core::domain::Side;
    use truesight_core::ports::{DraftEvent, DraftEventSource};

    struct QueueSource(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.pop_front())
        }
    }

    let atlas = parse_hero_atlas().expect("embedded hero atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    let (fw, vh, strip_h) = (2560_usize, 1440_usize, 116_usize);

    let paint_wave = |rad_picks: &[Option<u16>; 5], dire_picks: &[Option<u16>; 5]| -> RgbFrame {
        let mut rgb = vec![16_u8; fw * strip_h * 3];
        let colors = [
            [52, 118, 255],
            [102, 255, 192],
            [191, 0, 191],
            [243, 240, 11],
            [255, 107, 0],
        ];
        for (base_x, picks) in [(280_usize, rad_picks), (1475_usize, dire_picks)] {
            for (slot, maybe_hid) in picks.iter().enumerate() {
                let x0 = base_x + slot * 165;
                for py in 0..8 {
                    for px in x0..x0 + 143 {
                        let idx = (py * fw + px) * 3;
                        rgb[idx..idx + 3].copy_from_slice(&colors[slot]);
                    }
                }
                if let Some(hid) = maybe_hid {
                    let hero_rgb = &atlas.heroes[&HeroId(*hid)];
                    for dy in 0..84 {
                        let sy = (dy * atlas.hero_height) / 84;
                        for dx in 0..143 {
                            let sx = (dx * atlas.hero_width) / 143;
                            let s = (sy * atlas.hero_width + sx) * 3;
                            let d = ((8 + dy) * fw + (x0 + dx)) * 3;
                            rgb[d..d + 3].copy_from_slice(&hero_rgb[s..s + 3]);
                        }
                    }
                } else {
                    for py in 8..92 {
                        for px in x0..x0 + 143 {
                            let idx = (py * fw + px) * 3;
                            rgb[idx] = 24;
                            rgb[idx + 1] = 28;
                            rgb[idx + 2] = 36;
                        }
                    }
                }
            }
        }
        RgbFrame::with_source_height(fw, strip_h, vh, rgb)
    };

    // Wave 1 (2+2 blind picks), Wave 2 (4+4), Wave 3 (5+5).
    let w1 = paint_wave(
        &[Some(25), Some(29), None, None, None],
        &[Some(14), Some(26), None, None, None],
    );
    let w2 = paint_wave(
        &[Some(25), Some(29), Some(31), Some(48), None],
        &[Some(14), Some(26), Some(43), Some(37), None],
    );
    let w3 = paint_wave(
        &[Some(25), Some(29), Some(31), Some(48), Some(3)],
        &[Some(14), Some(26), Some(43), Some(37), Some(111)],
    );

    let frames = VecDeque::from([w1.clone(), w1, w2.clone(), w2, w3.clone(), w3]);
    let mut adapter = VisionCaptureAdapter::new(QueueSource(frames), catalog);

    let mut wave1_picks = Vec::new();
    for _ in 0..2 {
        for ev in adapter.poll_events().unwrap() {
            if let DraftEvent::PickDetected {
                side,
                slot,
                hero_id,
                ..
            } = ev
            {
                wave1_picks.push((side, slot, hero_id));
            }
        }
    }
    assert_eq!(
        wave1_picks.len(),
        4,
        "Wave 1 must lock exactly 2+2 picks with 0 empty-slot phantoms: {wave1_picks:?}"
    );
    assert!(wave1_picks.contains(&(Side::Allies, 0, HeroId(25))));
    assert!(wave1_picks.contains(&(Side::Allies, 1, HeroId(29))));
    assert!(wave1_picks.contains(&(Side::Enemies, 0, HeroId(14))));
    assert!(wave1_picks.contains(&(Side::Enemies, 1, HeroId(26))));

    let mut wave2_picks = Vec::new();
    for _ in 0..2 {
        for ev in adapter.poll_events().unwrap() {
            if let DraftEvent::PickDetected {
                side,
                slot,
                hero_id,
                ..
            } = ev
            {
                wave2_picks.push((side, slot, hero_id));
            }
        }
    }
    assert_eq!(
        wave2_picks.len(),
        4,
        "Wave 2 must add the 4 new picks cleanly: {wave2_picks:?}"
    );
    assert!(wave2_picks.contains(&(Side::Allies, 2, HeroId(31))));
    assert!(wave2_picks.contains(&(Side::Allies, 3, HeroId(48))));
    assert!(wave2_picks.contains(&(Side::Enemies, 2, HeroId(43))));
    assert!(wave2_picks.contains(&(Side::Enemies, 3, HeroId(37))));

    let mut wave3_picks = Vec::new();
    for _ in 0..2 {
        for ev in adapter.poll_events().unwrap() {
            if let DraftEvent::PickDetected {
                side,
                slot,
                hero_id,
                ..
            } = ev
            {
                wave3_picks.push((side, slot, hero_id));
            }
        }
    }
    assert_eq!(
        wave3_picks.len(),
        2,
        "Wave 3 must lock both last picks cleanly: {wave3_picks:?}"
    );
    assert!(wave3_picks.contains(&(Side::Allies, 4, HeroId(3))));
    assert!(wave3_picks.contains(&(Side::Enemies, 4, HeroId(111))));
}

#[test]
fn pickphase_ranked_reveal_sequence_and_shorter_cards_detect_spirit_breaker_and_muerta() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter, parse_hero_atlas,
    };
    use std::collections::VecDeque;
    use truesight_core::domain::Side;
    use truesight_core::ports::{DraftEvent, DraftEventSource};

    struct QueueSource(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.pop_front())
        }
    }

    let atlas = parse_hero_atlas().expect("embedded hero atlas must load");
    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    let rad_colors = [
        [52_u8, 118, 255],
        [102, 255, 192],
        [191, 0, 191],
        [243, 240, 11],
        [255, 107, 0],
    ];
    let dire_colors = [
        [254_u8, 134, 194],
        [161, 180, 71],
        [101, 217, 247],
        [0, 131, 33],
        [164, 105, 0],
    ];

    for (fw, vh, strip_h) in [
        (2560_usize, 1440_usize, 116_usize),
        (1920_usize, 1080_usize, 87_usize),
    ] {
        let sx = fw as f32 / 2560.0;
        let sy = vh as f32 / 1440.0;

        for (card_y_base, card_h_base, rad_base_x, dire_base_x, pad_base, card_w_base, label) in [
            (
                8.0_f32,
                84.0_f32,
                280_usize,
                1475_usize,
                0.0_f32,
                143.0_f32,
                "full_84px",
            ),
            (
                4.0_f32,
                56.0_f32,
                282_usize,
                1470_usize,
                12.0_f32,
                121.0_f32,
                "shorter_56px",
            ),
        ] {
            type SideRevealSpec<'a> = (&'a [Option<u16>; 5], usize, f32, [[u8; 3]; 5]);
            let paint_reveal_frame = |revealed: bool, glow_boost: i16| -> RgbFrame {
                let mut rgb = vec![16_u8; fw * strip_h * 3];
                let picks: [SideRevealSpec<'_>; 2] = [
                    (
                        &[Some(71), Some(25), None, None, None],
                        rad_base_x,
                        pad_base,
                        rad_colors,
                    ),
                    (
                        &[Some(138), Some(14), None, None, None],
                        dire_base_x,
                        pad_base,
                        dire_colors,
                    ),
                ];
                let card_y0 = (card_y_base * sy).round() as usize;
                let card_h = (card_h_base * sy).round().max(1.0) as usize;
                let card_w = (card_w_base * sx).round().max(1.0) as usize;
                let bar_w = (143.0 * sx).round().max(1.0) as usize;
                let bar_h = (card_y0.saturating_sub(0)).min((8.0 * sy).round().max(1.0) as usize);

                for (slot_picks, base_x, pad_x, colors) in picks {
                    for (slot, maybe_hid) in slot_picks.iter().enumerate() {
                        let bar_x0 = ((base_x as f32 + slot as f32 * 165.0) * sx).round() as usize;
                        for py in 0..bar_h {
                            for px in bar_x0..(bar_x0 + bar_w).min(fw) {
                                let idx = (py * fw + px) * 3;
                                rgb[idx..idx + 3].copy_from_slice(&colors[slot]);
                            }
                        }
                        let cx0 = (((base_x as f32 + slot as f32 * 165.0 + pad_x) * sx).round()
                            as usize)
                            .min(fw - 1);
                        if revealed && let Some(hid) = maybe_hid {
                            let hero_rgb = &atlas.heroes[&HeroId(*hid)];
                            for dy in 0..card_h {
                                let sy_src = (dy * atlas.hero_height) / card_h;
                                for dx in 0..card_w {
                                    let sx_src = (dx * atlas.hero_width) / card_w;
                                    let s = (sy_src * atlas.hero_width + sx_src) * 3;
                                    let d = ((card_y0 + dy).min(strip_h - 1) * fw
                                        + (cx0 + dx).min(fw - 1))
                                        * 3;
                                    for c in 0..3 {
                                        rgb[d + c] = (i16::from(hero_rgb[s + c]) + glow_boost)
                                            .clamp(0, 255)
                                            as u8;
                                    }
                                }
                            }
                        } else {
                            for py in card_y0..(card_y0 + card_h).min(strip_h) {
                                for px in cx0..(cx0 + card_w).min(fw) {
                                    let idx = (py * fw + px) * 3;
                                    rgb[idx] = 24;
                                    rgb[idx + 1] = 28;
                                    rgb[idx + 2] = 36;
                                }
                            }
                        }
                    }
                }
                RgbFrame::with_source_height(fw, strip_h, vh, rgb)
            };

            let f0_empty = paint_reveal_frame(false, 0);
            let f1_flash = paint_reveal_frame(true, 34);
            let f2_settle_a = paint_reveal_frame(true, 10);
            let f3_settle_b = paint_reveal_frame(true, 0);

            let frames = VecDeque::from([f0_empty, f1_flash, f2_settle_a, f3_settle_b]);
            let mut adapter = VisionCaptureAdapter::new(QueueSource(frames), catalog.clone());

            let ev0 = adapter.poll_events().unwrap();
            assert!(
                ev0.is_empty(),
                "{fw}x{vh} {label}: empty PickPhase slots must emit 0 picks, got {ev0:?}"
            );

            let mut locked = Vec::new();
            for _ in 0..3 {
                for ev in adapter.poll_events().unwrap() {
                    if let DraftEvent::PickDetected {
                        side,
                        slot,
                        hero_id,
                        ..
                    } = ev
                    {
                        locked.push((side, slot, hero_id));
                    }
                }
            }
            assert_eq!(
                locked.len(),
                4,
                "{fw}x{vh} {label}: first reveal wave must lock exactly 4 heroes without empty-slot phantoms: {locked:?}"
            );
            assert!(locked.contains(&(Side::Allies, 0, HeroId(71)))); // Spirit Breaker
            assert!(locked.contains(&(Side::Allies, 1, HeroId(25)))); // Lina
            assert!(locked.contains(&(Side::Enemies, 0, HeroId(138)))); // Muerta
            assert!(locked.contains(&(Side::Enemies, 1, HeroId(14)))); // Pudge
        }
    }
}

#[test]
fn pickphase_ranked_medal_banner_and_slanted_cards_detect_luna_last_pick_without_phantoms() {
    use crate::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter,
        compute_pickphase_topbar_slot_rects, parse_ppm_p6, pickphase_upper_core_rect,
    };
    use std::collections::VecDeque;
    use truesight_core::domain::Side;
    use truesight_core::ports::{DraftEvent, DraftEventSource};

    let (rad_2k, dire_2k) = compute_pickphase_topbar_slot_rects(2560, 1440);
    let core_2k = pickphase_upper_core_rect(rad_2k[0]);
    assert_eq!(rad_2k[0].x, 292, "Radiant slot 0 must align at +12px");
    assert_eq!(rad_2k[0].width, 126, "PickPhase width must cover 126px");
    assert_eq!(dire_2k[0].x, 1477, "Dire slot 0 must align at +2px");
    assert_eq!(core_2k.height, 50, "Upper core must preserve 22/36 of 82px");

    struct QueueSource(VecDeque<RgbFrame>);
    impl ScreenFrameSource for QueueSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(self.0.pop_front())
        }
    }

    let load_ranked_strip = |ppm: &[u8]| -> RgbFrame {
        let strip = parse_ppm_p6(ppm).expect("anonymized Ranked 2-2-1 topbar PPM must decode");
        assert_eq!((strip.width, strip.height), (1279, 47));
        RgbFrame::with_source_height(strip.width, strip.height, 722, strip.rgb)
    };

    // Anonymized top-bar-only captures (y = 0..47, no player names) from the live Ranked 2-2-1
    // session where the previous 118px (+18/+8) PickPhase crop missed Luna (#48) on Wave 3.
    let w1 = load_ranked_strip(include_bytes!("../tests/fixtures/ranked_221_w1.ppm"));
    let w2 = load_ranked_strip(include_bytes!("../tests/fixtures/ranked_221_w2.ppm"));
    let w3 = load_ranked_strip(include_bytes!("../tests/fixtures/ranked_221_w3.ppm"));

    let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
    let frames = VecDeque::from([w1.clone(), w1, w2.clone(), w2, w3.clone(), w3]);
    let mut adapter = VisionCaptureAdapter::new(QueueSource(frames), catalog);

    let poll_two_frames_sorted = |ad: &mut VisionCaptureAdapter<QueueSource>| {
        let mut wave = Vec::new();
        for _ in 0..2 {
            for ev in ad.poll_events().unwrap() {
                if let DraftEvent::PickDetected {
                    side,
                    slot,
                    hero_id,
                    ..
                } = ev
                {
                    wave.push((side, slot, hero_id));
                }
            }
        }
        wave.sort_by_key(|(side, slot, id)| (*side as u8, *slot, id.0));
        wave
    };

    assert_eq!(
        poll_two_frames_sorted(&mut adapter),
        vec![
            (Side::Enemies, 1, HeroId(72)),
            (Side::Enemies, 4, HeroId(5)),
        ],
        "Wave 1 must emit exactly the 2 Dire picks with 0 empty-slot phantoms"
    );

    assert_eq!(
        poll_two_frames_sorted(&mut adapter),
        vec![
            (Side::Allies, 0, HeroId(104)),
            (Side::Allies, 1, HeroId(30)),
            (Side::Allies, 2, HeroId(87)),
            (Side::Allies, 4, HeroId(35)),
            (Side::Enemies, 2, HeroId(57)),
            (Side::Enemies, 3, HeroId(41)),
        ],
        "Wave 2 must emit exact 6 new picks without prematching open Radiant slot 3 or Dire slot 0"
    );

    assert_eq!(
        poll_two_frames_sorted(&mut adapter),
        vec![
            (Side::Allies, 3, HeroId(48)),
            (Side::Enemies, 0, HeroId(21)),
        ],
        "Wave 3 must lock last-pick Luna (#48) in Radiant slot 3 and Windranger (#21) in Dire slot 0"
    );
}
