pub mod gsi;
pub mod meta_repo;
pub mod snapshot_data;
pub mod snapshot_pairs;
pub mod sync;
pub mod vision;

#[cfg(test)]
mod tests {
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
            slardar_vs_silencer.vs_matches > 0
                && slardar_vs_silencer.counter_delta.abs() > 0.0001,
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
                .set_ally(slot, Some(DraftPick { hero_id: hid, role: Some(role) }))
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
                .set_enemy(slot, Some(DraftPick { hero_id: hid, role: Some(role) }))
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

        assert_eq!(META_TTL_SECS, 86_400, "Meta DB TTL must be 24 hours (86400s)");
        assert_eq!(PLAYER_TTL_SECS, 3_600, "Player comfort TTL must be 1 hour (3600s)");

        let tmp_dir = std::env::temp_dir().join(format!(
            "truesight_cache_test_{}",
            std::process::id()
        ));
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

        assert!(!mgr.is_meta_stale(now + 3600), "Meta cache at +1h must be fresh (<24h)");
        assert!(mgr.is_meta_stale(now + 86_401), "Meta cache at +24h1s must be stale (>24h)");
        assert!(!mgr.is_player_stale(now + 1800), "Player cache at +30m must be fresh (<1h)");
        assert!(mgr.is_player_stale(now + 3601), "Player cache at +60m1s must be stale (>1h)");

        let repo = mgr.build_repository();
        let mat = repo.load_matrix(Bracket::Legend).unwrap();
        let am = mat.hero(HeroId(1)).unwrap();
        assert_eq!(am.personal_matches, 50);
        assert_eq!(am.personal_wins, 40);
        assert!(am.comfort_decay > 0.98, "Recently played Anti-Mage must have ~1.0 decay");
        assert!(am.role_winrates[0] > 0.59, "Cached heroStats 60% winrate must override snapshot: {}", am.role_winrates[0]);
        assert!(am.trend_delta > 0.01, "Cached 7-day trend jump must populate trend_delta: {}", am.trend_delta);

        let jugg = mat.hero(HeroId(8)).unwrap();
        assert_eq!(jugg.personal_matches, 30);
        assert!(jugg.comfort_decay < 0.50, "Stale Juggernaut games must decay: {}", jugg.comfort_decay);

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
                    rgb: rgb1,
                }),
                Some(RgbFrame {
                    width: w2,
                    height: h2,
                    rgb: rgb2,
                }),
            ]),
        };

        let mut adapter = VisionCaptureAdapter::new(source, catalog);
        adapter.set_ally_faction(Faction::Radiant);
        let evs1 = adapter.poll_events().unwrap();
        assert_eq!(evs1.len(), 3, "Frame 1 must detect 3 picked heroes and ignore 7 dark slots");
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
            LocalCacheManager, normalize_steam32_id, parse_loginusers_vdf,
            resolve_steam32_account_id,
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

        assert_eq!(normalize_steam32_id(76_561_198_061_460_213), Some(101_194_485));
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

        let mat_a = mgr_a.build_repository().load_matrix(Bracket::Legend).unwrap();
        let mat_b = mgr_b.build_repository().load_matrix(Bracket::Legend).unwrap();
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
            {"heroId1": 1, "heroId2": 97, "synergy": 0.042, "matchCount": 12500}
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
            rgb: pick_rgb,
        }]));
        let mut adapter = VisionCaptureAdapter::new(source, catalog);
        let events = adapter.poll_events().unwrap();
        assert_eq!(events.len(), 8, "Expected 8 picks and 0 empty medal slots: {events:?}");

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
            fn capture_frame(
                &mut self,
            ) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
                Ok(self.0.pop_front())
            }
        }

        let atlas = parse_hero_atlas().expect("embedded hero atlas must load");
        let catalog = HeroVisionCatalog::from_embedded().expect("vision catalog must load");
        let frame_w = 2560_usize;
        let frame_h = 96_usize;
        let virtual_h = 1440_usize;
        let (rad_game, _) = compute_ingame_topbar_slot_rects(frame_w, virtual_h);

        let paint_ingame = |buf: &mut [u8],
                            rect: crate::vision::TopBarSlotRect,
                            hid: u16,
                            chroma_scale: f32| {
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
                rgb: single_slot_rgb,
            },
            RgbFrame {
                width: frame_w,
                height: frame_h,
                rgb: valid_rgb,
            },
            RgbFrame {
                width: frame_w,
                height: frame_h,
                rgb: dead_overwrite_rgb,
            },
            RgbFrame {
                width: frame_w,
                height: frame_h,
                rgb: swap_rgb.clone(),
            },
            RgbFrame {
                width: frame_w,
                height: frame_h,
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
        assert_eq!(ev_valid.len(), 2, "Valid 2-slot frame must emit both heroes: {ev_valid:?}");

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
}
