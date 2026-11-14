use super::{AppSession, execute_command, parse_account_id_arg, render_dashboard_string};

#[test]
fn interactive_commands_update_5v5_table_and_recommendations() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 3 magnus").unwrap());
    assert!(execute_command(&mut session, "enemy 1 medusa").unwrap());
    assert!(execute_command(&mut session, "enemy 2 storm").unwrap());
    assert!(execute_command(&mut session, "role 1").unwrap());

    let rendered = render_dashboard_string(&session, false);
    assert!(rendered.contains("TRUESIGHT-DOTA · 5v5 LIVE DRAFT ORACLE"));
    assert!(rendered.contains("A3: Magnus"));
    assert!(rendered.contains("Medusa"));
    assert!(rendered.contains("Storm Spi"));
    assert!(rendered.contains("1. Anti-Mage"));

    assert!(execute_command(&mut session, "faction dire").unwrap());
    assert_eq!(session.draft.ally_faction, truesight_core::domain::Faction::Dire);
    session.apply_event(truesight_core::ports::DraftEvent::FactionDetected {
        faction: truesight_core::domain::Faction::Radiant,
    });
    assert_eq!(session.draft.ally_faction, truesight_core::domain::Faction::Radiant);

    assert!(execute_command(&mut session, "account none").unwrap());
    assert_eq!(session.cache_mgr.player_account_id, None);
    assert!(session.db_status.contains("Player: unlinked"));

    assert!(execute_command(&mut session, "account 76561198061460213").unwrap());
    assert_eq!(session.cache_mgr.player_account_id, Some(101_194_485));
    assert!(session.db_status.contains("Player #101194485"));

    // Explicit account override must not be overwritten by GSI SteamID
    session.apply_event(truesight_core::ports::DraftEvent::PlayerSteamIdDetected {
        steam32: 222_333_444,
    });
    assert_eq!(session.cache_mgr.player_account_id, Some(101_194_485));

    // In Auto mode, GSI SteamID updates active player account
    assert!(execute_command(&mut session, "account auto").unwrap());
    session.apply_event(truesight_core::ports::DraftEvent::PlayerSteamIdDetected {
        steam32: 222_333_444,
    });
    assert_eq!(session.cache_mgr.player_account_id, Some(222_333_444));
}

#[test]
fn bracket_command_and_slot_search_filter_work_across_all_eight_medals() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
    assert!(execute_command(&mut session, "ally 2 invoker").unwrap());
    assert!(execute_command(&mut session, "ally 3 magnus").unwrap());
    assert!(execute_command(&mut session, "ally 4 spirit breaker").unwrap());
    assert!(execute_command(&mut session, "ally 5 crystal maiden").unwrap());
    assert!(execute_command(&mut session, "enemy 1 medusa").unwrap());
    assert!(execute_command(&mut session, "enemy 2 storm spirit").unwrap());
    assert!(execute_command(&mut session, "enemy 3 axe").unwrap());
    assert!(execute_command(&mut session, "enemy 4 shadow demon").unwrap());
    assert!(execute_command(&mut session, "enemy 5 abaddon").unwrap());

    assert!(execute_command(&mut session, "bracket herald").unwrap());
    let (eval_herald, _) = session.evaluate();

    assert!(execute_command(&mut session, "bracket immortal").unwrap());
    let (eval_immortal, _) = session.evaluate();

    assert!(
        (eval_herald.total_counter_edge - eval_immortal.total_counter_edge).abs() > 0.001,
        "Switching bracket via command must update Counter Edge"
    );
    assert!(
        (eval_herald.total_synergy_edge - eval_immortal.total_synergy_edge).abs() > 0.0002,
        "Switching bracket via command must update Synergy Edge"
    );

    let matches = session.filter_heroes_for_slot(truesight_core::domain::Side::Allies, 1, "inv");
    assert!(!matches.is_empty());
    assert_eq!(matches[0].1, "Invoker");
}

#[test]
fn pick_detected_assigns_roles_by_affinity_preserves_user_locks_and_tracks_local_hero() {
    use truesight_core::domain::{Faction, HeroId, Role, Side};
    use truesight_core::ports::DraftEvent;

    let mut session = AppSession::new().unwrap();
    session.set_account_id(Some(111_222_333), true);

    // Live GSI local player Night Stalker (60) auto-places into Pos 3 Offlane and sets target_role
    session.apply_event(DraftEvent::LocalPlayerHeroDetected {
        hero_slug: "npc_dota_hero_night_stalker".into(),
    });
    assert_eq!(session.local_player_hero, Some(HeroId(60)));
    assert_eq!(session.draft.allies[2].unwrap().hero_id, HeroId(60));
    assert_eq!(session.draft.allies[2].unwrap().role, Some(Role::Offlane));
    assert_eq!(session.draft.target_role, Role::Offlane);

    // Feeding Allied replay top-bar lobby order assigns optimal P1..P5 roles rather than lobby slot indices
    for (slot, hid) in [(0, 67_u16), (1, 60), (2, 27), (3, 64), (4, 128)] {
        session.apply_event(DraftEvent::PickDetected {
            side: Side::Allies,
            slot,
            hero_id: HeroId(hid),
            role: Role::ALL.get(slot).copied(),
        });
    }
    assert_eq!(session.draft.allies[0].unwrap().hero_id, HeroId(67)); // Spectre -> P1 Carry
    assert_eq!(session.draft.allies[1].unwrap().hero_id, HeroId(128)); // Snapfire -> P2 Mid
    assert_eq!(session.draft.allies[2].unwrap().hero_id, HeroId(60)); // Night Stalker -> P3 Offlane
    assert_eq!(session.draft.allies[3].unwrap().hero_id, HeroId(27)); // Shadow Shaman -> P4
    assert_eq!(session.draft.allies[4].unwrap().hero_id, HeroId(64)); // Jakiro -> P5
    assert_eq!(session.draft.target_role, Role::Offlane);

    // Enemy top-bar picks: Slark=93, Viper=47, Tusk=100, Beastmaster=38, Clinkz=56
    for (slot, hid) in [(0, 93_u16), (1, 47), (2, 100), (3, 38), (4, 56)] {
        session.apply_event(DraftEvent::PickDetected {
            side: Side::Enemies,
            slot,
            hero_id: HeroId(hid),
            role: Role::ALL.get(slot).copied(),
        });
    }
    assert_eq!(session.draft.enemies[0].unwrap().hero_id, HeroId(93));
    let viper_slot = session
        .draft
        .enemies
        .iter()
        .position(|p| p.map(|d| d.hero_id) == Some(HeroId(47)))
        .expect("Viper placed on Enemies");
    assert!(viper_slot == 1 || viper_slot == 2);

    // User manually swaps Viper to P5 (slot 4) and Beastmaster to P3 (slot 2)
    session.swap_slots(Side::Enemies, viper_slot, 4);
    let bm_slot = session
        .draft
        .enemies
        .iter()
        .position(|p| p.map(|d| d.hero_id) == Some(HeroId(38)))
        .expect("Beastmaster placed on Enemies");
    if bm_slot != 2 {
        session.swap_slots(Side::Enemies, bm_slot, 2);
    }
    assert!(session.enemy_locked_slots[4]);
    assert!(session.enemy_locked_slots[2]);
    assert_eq!(session.draft.enemies[4].unwrap().hero_id, HeroId(47));
    assert_eq!(session.draft.enemies[4].unwrap().role, Some(Role::HardSupport));
    assert_eq!(session.draft.enemies[2].unwrap().hero_id, HeroId(38));
    assert_eq!(session.draft.enemies[2].unwrap().role, Some(Role::Offlane));

    // Subsequent vision PickDetected frames must respect locked slots
    session.apply_event(DraftEvent::PickDetected {
        side: Side::Enemies,
        slot: 1,
        hero_id: HeroId(47),
        role: Some(Role::Mid),
    });
    assert_eq!(session.draft.enemies[4].unwrap().hero_id, HeroId(47));
    assert_eq!(session.draft.enemies[2].unwrap().hero_id, HeroId(38));

    // Clearing Spectre (67) removes it and suppresses re-adding from subsequent vision polls until reset
    session.clear_slot_manual(Side::Allies, 0);
    assert!(session.draft.allies[0].is_none());
    session.apply_event(DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 0,
        hero_id: HeroId(67),
        role: Some(Role::Carry),
    });
    assert!(session.draft.allies[0].is_none());

    // Replay PlayerHeroMapped matches linked Steam32 ID to faction, local hero, and role
    session.apply_event(DraftEvent::PlayerHeroMapped {
        steam32: 111_222_333,
        faction: Faction::Dire,
        hero_slug: "npc_dota_hero_night_stalker".into(),
    });
    assert_eq!(session.draft.ally_faction, Faction::Dire);
    assert_eq!(session.local_player_hero, Some(HeroId(60)));
    assert_eq!(session.draft.target_role, Role::Offlane);
}

#[test]
fn cli_argument_and_command_parsers_validate_swap_ban_clear_and_invalid_inputs() {
    let args_missing = vec!["truesight-dota".into(), "--account-id".into()];
    assert!(parse_account_id_arg(&args_missing).is_err());

    let args_unlinked = vec!["truesight-dota".into(), "--account-id=none".into()];
    assert_eq!(parse_account_id_arg(&args_unlinked), Ok(Some(None)));

    let args_auto = vec!["truesight-dota".into(), "--account-id".into(), "auto".into()];
    assert_eq!(parse_account_id_arg(&args_auto), Ok(None));

    let args_steam64 = vec![
        "truesight-dota".into(),
        "--account-id".into(),
        "76561198087521469".into(),
    ];
    assert_eq!(parse_account_id_arg(&args_steam64), Ok(Some(Some(127_255_741))));

    let args_invalid = vec!["truesight-dota".into(), "--account-id=not_a_number".into()];
    assert!(parse_account_id_arg(&args_invalid).is_err());

    let mut session = AppSession::new().unwrap();
    let void_id = session.find_hero("faceless void").unwrap();
    let puck_id = session.find_hero("puck").unwrap();
    let am_id = session.find_hero("anti-mage").unwrap();

    assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
    assert!(execute_command(&mut session, "ally 2 puck").unwrap());
    assert!(execute_command(&mut session, "swap ally 1 2").unwrap());
    assert_eq!(session.draft.allies[0].unwrap().hero_id, puck_id);
    assert_eq!(session.draft.allies[1].unwrap().hero_id, void_id);

    assert!(execute_command(&mut session, "ban anti-mage").unwrap());
    assert!(session.draft.bans.contains(&Some(am_id)));
    assert!(execute_command(&mut session, "ban anti-mage").unwrap());
    assert!(!session.draft.bans.contains(&Some(am_id)));

    assert!(execute_command(&mut session, "ally 1 none").unwrap());
    assert!(session.draft.allies[0].is_none());

    assert!(execute_command(&mut session, "faction dire").unwrap());
    assert_eq!(session.draft.ally_faction, truesight_core::domain::Faction::Dire);

    assert!(execute_command(&mut session, "swap ally 1 9").is_err());
    assert!(execute_command(&mut session, "role 99").is_err());
    assert!(execute_command(&mut session, "unknown_cmd").is_err());
    assert!(!execute_command(&mut session, "quit").unwrap());
}

#[test]
fn background_sync_reloads_active_matrix_preserves_model_on_error_and_guards_account_switch() {
    use std::time::Duration;
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId};

    let tmp_dir = std::env::temp_dir().join("truesight_test_async_sync");
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mut session = AppSession::new().unwrap();
    session.cache_mgr = LocalCacheManager::with_dir(tmp_dir.clone(), 111_222_333);
    session.reload_repository();

    let (gate_tx, gate_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let started = session.start_background_sync_with(false, move |mgr, _force| {
        let _ = gate_rx.recv();
        let meta = r#"[{"id": 1, "1_pick": 1000, "1_win": 650, "2_pick": 1000, "2_win": 650, "3_pick": 1000, "3_win": 650, "4_pick": 1000, "4_win": 650, "5_pick": 1000, "5_win": 650, "6_pick": 1000, "6_win": 650, "7_pick": 1000, "7_win": 650}]"#;
        mgr.write_cached_files(Some(meta), None, LocalCacheManager::now_unix())
            .map_err(|e| e.0)?;
        Ok("Synced OpenDota: heroStats (24h TTL).".into())
    });
    assert!(started);
    assert!(session.sync_in_flight);
    assert!(!session.start_background_sync_with(false, |_, _| Ok("duplicate".into())));

    session.set_bracket(Bracket::Divine);
    gate_tx.send(()).unwrap();

    for _ in 0..100 {
        session.poll_live_sources(None, None);
        if !session.sync_in_flight {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!session.sync_in_flight);
    assert_eq!(session.draft.bracket, Bracket::Divine);
    assert!(
        session.matrix.hero(HeroId(1)).unwrap().role_winrates[0] > 0.62,
        "matrix must reload refreshed cached meta"
    );
    assert_eq!(
        session.last_sync_notice.take().as_deref(),
        Some("Synced OpenDota: heroStats (24h TTL).")
    );

    // Failed refresh must preserve last valid model and report status clearly
    assert!(session.start_background_sync_with(true, |_, _| Err("network timeout".into())));
    for _ in 0..100 {
        session.poll_live_sources(None, None);
        if !session.sync_in_flight {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!session.sync_in_flight);
    assert!(session.matrix.hero(HeroId(1)).unwrap().role_winrates[0] > 0.62);
    assert!(
        session
            .last_sync_notice
            .take()
            .as_deref()
            .unwrap_or_default()
            .contains("network timeout")
    );
    assert!(session.db_status.contains("sync failed"));

    // Account switch while a sync is in flight must discard stale completion from the old generation
    let (gate2_tx, gate2_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel::<()>(1);
    assert!(session.start_background_sync_with(true, move |_mgr, _| {
        let _ = gate2_rx.recv();
        let _ = done_tx.send(());
        Ok("Stale account completion".into())
    }));
    session.set_account_id(Some(222_333_444), true);
    gate2_tx.send(()).unwrap();
    let _ = done_rx.recv_timeout(Duration::from_millis(500));
    std::thread::sleep(Duration::from_millis(10));
    session.poll_live_sources(None, None);
    assert_eq!(session.cache_mgr.player_account_id, Some(222_333_444));
    assert_eq!(session.last_sync_notice, None);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn draft_reset_and_new_match_transition_reset_vision_worker_without_mid_draft_wipes() {
    use std::time::Duration;
    use truesight_adapters::vision::{
        BoundedVisionWorker, RgbFrame, ScreenFrameSource, TopBarSlotRect,
        compute_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::{HeroId, Side};
    use truesight_core::ports::DraftEvent;

    struct StationarySource(RgbFrame);
    impl ScreenFrameSource for StationarySource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(Some(self.0.clone()))
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

    let atlas = parse_hero_atlas().unwrap();
    let (w, h) = (1920usize, 1080usize);
    let mut rgb = vec![0u8; w * h * 3];
    let (rad_r, dire_r) = compute_topbar_slot_rects(w, h);
    let rad_heroes = [1_u16, 13, 97, 79, 5];
    let dire_heroes = [94_u16, 17, 2, 100, 30];
    for (slot, hid) in rad_heroes.iter().enumerate() {
        paint_slot(&mut rgb, w, rad_r[slot], &atlas.heroes[&HeroId(*hid)], atlas.hero_width, atlas.hero_height);
    }
    for (slot, hid) in dire_heroes.iter().enumerate() {
        paint_slot(&mut rgb, w, dire_r[slot], &atlas.heroes[&HeroId(*hid)], atlas.hero_width, atlas.hero_height);
    }
    let mut worker = BoundedVisionWorker::spawn_with_source(StationarySource(RgbFrame {
        width: w,
        height: h,
        rgb,
    }))
    .unwrap();

    let mut session = AppSession::new().unwrap();
    for _ in 0..40 {
        session.poll_live_sources(None, Some(&mut worker));
        if session.draft.allies.iter().flatten().count() == 5
            && session.draft.enemies.iter().flatten().count() == 5
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(session.draft.allies.iter().flatten().count(), 5);
    assert_eq!(session.draft.enemies.iter().flatten().count(), 5);

    // Mid-draft and in-game GSI heartbeats must NOT wipe active draft picks or manual locks
    let locked_id = HeroId(67);
    session.set_slot_manual(Side::Allies, 0, locked_id);
    for state in [
        "DOTA_GAMERULES_STATE_HERO_SELECTION",
        "DOTA_GAMERULES_STATE_HERO_SELECTION",
        "DOTA_GAMERULES_STATE_STRATEGY_TIME",
        "DOTA_GAMERULES_STATE_PRE_GAME",
        "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS",
    ] {
        session.apply_event(DraftEvent::GameRulesStateChanged {
            state_name: state.into(),
            game_time_secs: 10,
        });
    }
    assert_eq!(session.draft.allies[0].map(|p| p.hero_id), Some(locked_id));
    assert!(session.ally_locked_slots[0]);

    // Manual RESET followed by stationary portraits repopulates all 10 slots via vision worker
    session.reset_draft();
    assert_eq!(session.draft.allies.iter().flatten().count(), 0);
    for _ in 0..40 {
        session.poll_live_sources(None, Some(&mut worker));
        if session.draft.allies.iter().flatten().count() == 5
            && session.draft.enemies.iter().flatten().count() == 5
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(session.draft.allies.iter().flatten().count(), 5);
    assert_eq!(session.draft.enemies.iter().flatten().count(), 5);

    // Transition from previous match IN_PROGRESS -> new match HERO_SELECTION resets prior-match picks
    session.set_slot_manual(Side::Allies, 0, locked_id);
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 1200,
    });
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
        game_time_secs: -60,
    });
    assert_eq!(session.draft.allies.iter().flatten().count(), 0);
    assert!(!session.ally_locked_slots[0]);
}
