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
                rgb: rgb1,
            }),
            Some(RgbFrame {
                width: w2,
                height: h2,
                source_height: None,
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
            rgb: single_slot_rgb,
        },
        RgbFrame {
            width: frame_w,
            height: frame_h,
            source_height: None,
            rgb: valid_rgb,
        },
        RgbFrame {
            width: frame_w,
            height: frame_h,
            source_height: None,
            rgb: dead_overwrite_rgb,
        },
        RgbFrame {
            width: frame_w,
            height: frame_h,
            source_height: None,
            rgb: swap_rgb.clone(),
        },
        RgbFrame {
            width: frame_w,
            height: frame_h,
            source_height: None,
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
            rgb: dark_gradient,
        },
        RgbFrame {
            width: w2,
            height: h2,
            source_height: None,
            rgb: empty_pick,
        },
        RgbFrame {
            width: w3,
            height: h3,
            source_height: None,
            rgb: gray_ui,
        },
        RgbFrame {
            width: w4,
            height: h4,
            source_height: None,
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
    assert_eq!(rad_rects[0].height, 30);

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
            DraftEvent::InventoryUpdated { item_slugs } => item_slugs.iter().any(|s| s == "rapier"),
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
            item_slugs: vec!["bfury".to_string()],
        },
    ));
    assert!(queue.push_for_epoch(
        0,
        DraftEvent::InventoryUpdated {
            item_slugs: vec!["bfury".to_string(), "manta".to_string()],
        },
    ));
    assert!(queue.push_for_epoch(
        0,
        DraftEvent::InventoryUpdated {
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
        item_slugs: vec![
            "bfury".to_string(),
            "manta".to_string(),
            "skadi".to_string(),
        ],
    }));
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
