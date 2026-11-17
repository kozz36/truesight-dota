use super::{
    AppSession, CliInputEvent, execute_command, parse_account_id_arg, render_dashboard_string,
    run_cli_loop, spawn_input_reader,
};

#[test]
fn interactive_commands_update_5v5_table_and_recommendations() {
    use truesight_adapters::sync::LocalCacheManager;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_interactive_cmds_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mut session = AppSession::new().unwrap();
    session.cache_mgr = LocalCacheManager::with_dir(tmp_dir.clone(), 0);
    session.cache_mgr.set_account_id(None, false);
    session.reload_repository();
    session.set_background_sync_runner(|_, _| Ok("Mock sync ok".into()));

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
    assert_eq!(
        session.draft.ally_faction,
        truesight_core::domain::Faction::Dire
    );
    session.apply_event(truesight_core::ports::DraftEvent::FactionDetected {
        faction: truesight_core::domain::Faction::Radiant,
    });
    assert_eq!(
        session.draft.ally_faction,
        truesight_core::domain::Faction::Radiant
    );

    assert!(execute_command(&mut session, "account none").unwrap());
    assert_eq!(session.cache_mgr.player_account_id, None);
    assert!(session.db_status.contains("Player: unlinked"));

    assert!(execute_command(&mut session, "account 76561198061460213").unwrap());
    assert_eq!(session.cache_mgr.player_account_id, Some(101_194_485));
    for _ in 0..100 {
        session.poll_live_sources(None, None);
        if !session.sync_in_flight {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
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
    for _ in 0..100 {
        session.poll_live_sources(None, None);
        if !session.sync_in_flight {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
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
    assert_eq!(
        session.draft.enemies[4].unwrap().role,
        Some(Role::HardSupport)
    );
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

    let args_auto = vec![
        "truesight-dota".into(),
        "--account-id".into(),
        "auto".into(),
    ];
    assert_eq!(parse_account_id_arg(&args_auto), Ok(None));

    let args_steam64 = vec![
        "truesight-dota".into(),
        "--account-id".into(),
        "76561198087521469".into(),
    ];
    assert_eq!(
        parse_account_id_arg(&args_steam64),
        Ok(Some(Some(127_255_741)))
    );

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
    assert_eq!(
        session.draft.ally_faction,
        truesight_core::domain::Faction::Dire
    );

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
    let mut worker = BoundedVisionWorker::spawn_with_source(StationarySource(RgbFrame {
        width: w,
        height: h,
        source_height: None,
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

#[derive(Clone, Default)]
struct SharedBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn as_string(&self) -> String {
        let bytes = self.0.lock().unwrap().clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl std::io::Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn cli_loop_polls_live_gsi_while_awaiting_input_and_preserves_partial_command() {
    let mut cursor_session = AppSession::new().unwrap();
    let cursor_rx = spawn_input_reader(std::io::Cursor::new(b"role 2\nquit\n".to_vec()));
    let mut cursor_out = SharedBuffer::default();
    run_cli_loop(
        &mut cursor_session,
        None,
        None,
        &cursor_rx,
        &mut cursor_out,
        false,
        std::time::Duration::from_millis(10),
    )
    .unwrap();
    assert_eq!(
        cursor_session.draft.target_role,
        truesight_core::domain::Role::Mid
    );

    let mut session = AppSession::new().unwrap();
    let medusa_id = session.find_hero("medusa").unwrap().0;
    let antimage_id = session.find_hero("antimage").unwrap();
    let mut listener = truesight_adapters::gsi::BoundedGsiListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr();
    let (tx, rx) = std::sync::mpsc::sync_channel::<CliInputEvent>(16);
    let mut out = SharedBuffer::default();
    let out_reader = out.clone();

    std::thread::scope(|scope| {
        let handle = scope.spawn(move || {
            run_cli_loop(
                &mut session,
                Some(&mut listener),
                None,
                &rx,
                &mut out,
                false,
                std::time::Duration::from_millis(15),
            )
            .unwrap();
            session
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !out_reader.as_string().contains("TRUESIGHT-DOTA") {
            assert!(
                std::time::Instant::now() < deadline,
                "Timed out waiting for initial CLI render"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        tx.send(CliInputEvent::Chunk("ally 1 ".into())).unwrap();

        let body =
            format!(r#"{{"truesight_pick":{{"side":"enemy","slot":0,"hero_id":{medusa_id}}}}}"#);
        let req = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        std::io::Write::write_all(&mut stream, req.as_bytes()).unwrap();
        std::io::Write::flush(&mut stream).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let snapshot = out_reader.as_string();
            let last_frame = snapshot.rsplit("\x1b[2J\x1b[H").next().unwrap_or("");
            if last_frame.contains("Medusa") && last_frame.ends_with(": ally 1 ") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Timed out waiting for live GSI frame preserving partial prompt; latest frame:\n{last_frame}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        tx.send(CliInputEvent::Chunk("antimage\nquit\n".into()))
            .unwrap();
        let final_session = handle.join().unwrap();
        assert_eq!(
            final_session
                .draft
                .allies
                .iter()
                .flatten()
                .map(|p| p.hero_id)
                .collect::<Vec<_>>(),
            vec![antimage_id]
        );
        assert_eq!(
            final_session
                .draft
                .enemies
                .iter()
                .flatten()
                .map(|p| p.hero_id)
                .collect::<Vec<_>>(),
            vec![truesight_core::domain::HeroId(medusa_id)]
        );
    });
}

#[test]
fn faction_change_atomically_swaps_teams_and_enemy_scout_uses_neutral_comfort() {
    use truesight_core::domain::{Faction, HeroId, Role, Side};
    use truesight_core::math::evaluate_draft_5v5;
    use truesight_core::ports::DraftEvent;

    let mut session = AppSession::new().unwrap();
    let rad_picks = ["antimage", "puck", "magnus", "rubick", "crystal_maiden"];
    let dire_picks = ["medusa", "storm_spirit", "axe", "tusk", "lion"];
    for (slot, name) in rad_picks.iter().enumerate() {
        assert!(execute_command(&mut session, &format!("ally {} {name}", slot + 1)).unwrap());
    }
    for (slot, name) in dire_picks.iter().enumerate() {
        let hid = session.find_hero(name).unwrap();
        session.apply_event(DraftEvent::PickDetected {
            side: Side::Enemies,
            slot,
            hero_id: hid,
            role: Role::ALL.get(slot).copied(),
        });
    }
    let expected_rad: Vec<HeroId> = session
        .draft
        .allies
        .iter()
        .flatten()
        .map(|p| p.hero_id)
        .collect();
    let expected_dire: Vec<HeroId> = session
        .draft
        .enemies
        .iter()
        .flatten()
        .map(|p| p.hero_id)
        .collect();
    assert_eq!(session.draft.ally_faction, Faction::Radiant);
    assert_eq!(expected_rad.len(), 5);
    assert_eq!(expected_dire.len(), 5);
    assert!(session.ally_locked_slots.iter().all(|&l| l));
    assert!(!session.enemy_locked_slots[0]);
    session.set_slot_manual(Side::Enemies, 0, expected_dire[0]);
    assert!(session.enemy_locked_slots[0]);

    // Switching faction to Dire via GSI event swaps 5v5 teams and locks atomically without dropping heroes
    session.apply_event(DraftEvent::FactionDetected {
        faction: Faction::Dire,
    });
    assert_eq!(session.draft.ally_faction, Faction::Dire);
    let allies_after: Vec<HeroId> = session
        .draft
        .allies
        .iter()
        .flatten()
        .map(|p| p.hero_id)
        .collect();
    let enemies_after: Vec<HeroId> = session
        .draft
        .enemies
        .iter()
        .flatten()
        .map(|p| p.hero_id)
        .collect();
    assert_eq!(allies_after, expected_dire);
    assert_eq!(enemies_after, expected_rad);
    assert!(session.ally_locked_slots[0]);
    assert!(session.enemy_locked_slots.iter().all(|&l| l));

    // Switching back via CLI command restores Radiant on Allies
    assert!(execute_command(&mut session, "faction radiant").unwrap());
    assert_eq!(session.draft.ally_faction, Faction::Radiant);
    assert_eq!(
        session
            .draft
            .allies
            .iter()
            .flatten()
            .map(|p| p.hero_id)
            .collect::<Vec<_>>(),
        expected_rad
    );

    // Clear Enemy Pos 1 so we can scout enemy options; confirm repeated BanDetected is idempotent
    session.clear_slot_manual(Side::Enemies, 0);
    let ban_target = session.find_hero("invoker").unwrap();
    session.apply_event(DraftEvent::BanDetected {
        hero_id: ban_target,
    });
    session.apply_event(DraftEvent::BanDetected {
        hero_id: ban_target,
    });
    assert!(session.draft.bans.contains(&Some(ban_target)));

    let scout_recs = session.recommend_enemy_heroes(Role::Carry, 5);
    assert_eq!(scout_recs.len(), 5);
    assert!(scout_recs.iter().all(|r| r.hero_id != ban_target));
    assert!(scout_recs.iter().all(|r| r.comfort_delta == 0.0));

    let top_enemy = &scout_recs[0];
    let mut check_draft = session.draft;
    check_draft.enemies[Role::Carry.index()] = Some(truesight_core::domain::DraftPick {
        hero_id: top_enemy.hero_id,
        role: Some(Role::Carry),
    });
    let mut neutral_weights = session.weights;
    neutral_weights.comfort_weight = 0.0;
    let check_analysis = evaluate_draft_5v5(&check_draft, &session.matrix, &neutral_weights);
    let expected_enemy_wr = 1.0 - check_analysis.ally_win_probability;
    assert!(
        (top_enemy.projected_win_probability - expected_enemy_wr).abs() < 1e-5,
        "Scout WR {} != neutral 1 - ally_win {}",
        top_enemy.projected_win_probability,
        expected_enemy_wr
    );
}

#[test]
fn gsi_http_listener_reconciles_full_session_state_after_overflow_when_peer_stops() {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;
    use truesight_adapters::gsi::BoundedGsiListener;
    use truesight_core::domain::Faction;

    let mut listener = BoundedGsiListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr();
    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_gsi_reconcile_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let mut session = AppSession::new().unwrap();
    session.cache_mgr = truesight_adapters::sync::LocalCacheManager::with_dir(tmp_dir.clone(), 0);
    session.set_background_sync_runner(|_, _| Ok("Mock sync ok".into()));
    session.set_account_auto();

    let post_json = |body: &str| {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        write!(
            s,
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        s.flush().unwrap();
        let mut resp = String::new();
        let _ = s.read_to_string(&mut resp);
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "Expected 200 OK, got: {resp}"
        );
    };

    // 1. Pre-reset match traffic (old hero + in-progress state)
    post_json(
        r#"{"map":{"game_state":"DOTA_GAMERULES_STATE_GAME_IN_PROGRESS","clock_time":1200},"truesight_pick":{"side":"ally","slot":0,"hero_id":67}}"#,
    );

    // 2. Match transition reset + Dire faction + SteamID + 16 distinct bans (exceeds 16-item capacity)
    post_json(concat!(
        r#"{"map":{"game_state":"DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD","clock_time":-90},"#,
        r#""player":{"team_name":"dire","steamid":"127255741"},"#,
        r#""draft":{"team2":{"ban0_id":3,"ban1_id":4,"ban2_id":6,"ban3_id":7,"ban4_id":8,"ban5_id":9,"ban6_id":10,"ban7_id":11},"#,
        r#""team3":{"ban0_id":12,"ban1_id":14,"ban2_id":15,"ban3_id":16,"ban4_id":18,"ban5_id":19,"ban6_id":20,"ban7_id":21}}}"#,
    ));

    // 3. Saturate with all 10 picks plus replay player mappings, then stop sending
    let ally_heroes = [1_u16, 13, 97, 79, 5];
    let enemy_heroes = [94_u16, 17, 2, 100, 30];
    for (slot, hid) in ally_heroes.iter().enumerate() {
        post_json(&format!(
            r#"{{"truesight_pick":{{"side":"ally","slot":{slot},"hero_id":{hid}}}}}"#
        ));
    }
    for (slot, hid) in enemy_heroes.iter().enumerate() {
        post_json(&format!(
            r#"{{"truesight_pick":{{"side":"enemy","slot":{slot},"hero_id":{hid}}}}}"#
        ));
    }
    post_json(concat!(
        r#"{"map":{"game_state":"DOTA_GAMERULES_STATE_HERO_SELECTION","clock_time":35},"#,
        r#""player":{"team2":{"player0":{"steamid":"76561198087521469","team_name":"dire"}},"#,
        r#""team3":{"player5":{"steamid":"76561198000000005","team_name":"radiant"}}},"#,
        r#""hero":{"team2":{"player0":{"name":"npc_dota_hero_antimage"}}}}"#,
    ));

    // 4. Single poll_live_sources drains overflowed HTTP queue and reconciles coherent 5v5 + 16 bans
    assert!(session.poll_live_sources(Some(&mut listener), None));
    let stats_after_first = listener.queue_stats();
    assert!(
        stats_after_first.dropped > 0,
        "Expected overflow drops: {stats_after_first:?}"
    );
    assert_eq!(stats_after_first.recovered_resyncs, 1);
    assert_eq!(session.draft.ally_faction, Faction::Dire);
    assert_eq!(session.cache_mgr.player_account_id, Some(127_255_741));
    assert_eq!(session.draft.allies.iter().flatten().count(), 5);
    assert_eq!(session.draft.enemies.iter().flatten().count(), 5);
    assert_eq!(session.draft.bans.iter().flatten().count(), 16);
    assert!(
        !session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id.0 == 67),
        "Pre-reset hero 67 must not leak into reconciled new match"
    );

    // Drain any auto-triggered player sync completion before verifying idle poll stability.
    for _ in 0..100 {
        if !session.sync_in_flight {
            break;
        }
        let _ = session.poll_live_sources(None, None);
        std::thread::sleep(Duration::from_millis(5));
    }

    // 5. Polling again with no traffic must not falsely increment recovered_resyncs
    assert!(!session.poll_live_sources(Some(&mut listener), None));
    assert_eq!(listener.queue_stats().recovered_resyncs, 1);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn sync_coordinator_serializes_generations_rejects_stale_writes_and_preserves_valid_cache_on_corruption()
 {
    use std::time::Duration;
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId};
    use truesight_core::ports::StatsRepository;

    let tmp_dir =
        std::env::temp_dir().join(format!("truesight_sync_gen_coord_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let mut session = AppSession::new().unwrap();
    session.cache_mgr = LocalCacheManager::with_dir(tmp_dir.clone(), 111_111_111);
    session.reload_repository();

    let (started1_tx, started1_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let (gate1_tx, gate1_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let (w1_res_tx, w1_res_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);

    let now_base = LocalCacheManager::now_unix();
    let stale_meta = r#"[{"id": 1, "1_pick": 1000, "1_win": 410, "2_pick": 1000, "2_win": 410, "3_pick": 1000, "3_win": 410, "4_pick": 1000, "4_win": 410, "5_pick": 1000, "5_win": 410, "6_pick": 1000, "6_win": 410, "7_pick": 1000, "7_win": 410}]"#;
    let acct1_player = r#"[{"hero_id": 1, "last_played": 1794499000, "games": 40, "win": 35}]"#;

    // 1. Launch Worker 1 for Account 111_111_111 and pause it behind gate1
    assert!(session.start_background_sync_with(true, move |mgr, _| {
        let _ = started1_tx.send(());
        let _ = gate1_rx.recv();
        let res = mgr
            .write_cached_files(
                Some(stale_meta),
                Some(acct1_player),
                now_base.saturating_add(50),
            )
            .map_err(|e| e.0);
        let _ = w1_res_tx.send(res.clone());
        res.map(|_| "Worker 1".into())
    }));
    started1_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    // 2. Switch to Account 222_333_444 and complete Worker 2 with newer valid shared meta + acct 222 data
    session.set_account_id(Some(222_333_444), true);
    let fresh_meta = r#"[{"id": 1, "1_pick": 1000, "1_win": 700, "2_pick": 1000, "2_win": 700, "3_pick": 1000, "3_win": 700, "4_pick": 1000, "4_win": 700, "5_pick": 1000, "5_win": 700, "6_pick": 1000, "6_win": 700, "7_pick": 1000, "7_win": 700}]"#;
    let acct2_player = r#"[{"hero_id": 2, "last_played": 1794499000, "games": 50, "win": 40}]"#;
    assert!(session.start_background_sync_with(true, move |mgr, _| {
        assert!(!mgr.is_cancelled());
        mgr.write_cached_files(Some(fresh_meta), Some(acct2_player), now_base)
            .map_err(|e| e.0)?;
        Ok("Worker 2 newer cache".into())
    }));

    for _ in 0..100 {
        session.poll_live_sources(None, None);
        if !session.sync_in_flight {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!session.sync_in_flight);
    assert!(session.matrix.hero(HeroId(1)).unwrap().role_winrates[0] > 0.62);
    assert_eq!(session.matrix.hero(HeroId(2)).unwrap().personal_matches, 50);

    // 3. Release stale Worker 1: generation fence must reject its write instead of overwriting Worker 2
    gate1_tx.send(()).unwrap();
    let w1_res = w1_res_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        w1_res.is_err(),
        "Stale Worker 1 write must be fenced out: {w1_res:?}"
    );

    session.reload_repository();
    assert!(
        session.matrix.hero(HeroId(1)).unwrap().role_winrates[0] > 0.62,
        "Stale Worker 1 must not overwrite newer shared heroStats"
    );
    assert_eq!(
        session.matrix.hero(HeroId(1)).unwrap().personal_matches,
        0,
        "Account 111 personal hero data must not leak into Account 222"
    );
    assert_eq!(session.matrix.hero(HeroId(2)).unwrap().personal_matches, 50);

    // 4. Truncated/corrupt payload must be rejected without touching valid cache or advancing timestamps
    let err = session.cache_mgr.write_cached_files(
        Some(r#"[{"id": 1, "1_pick": 1000"#),
        None,
        now_base + 9_999,
    );
    assert!(err.is_err(), "Truncated JSON payload must be rejected");
    assert_ne!(
        session.cache_mgr.read_timestamps().0,
        Some(now_base + 9_999)
    );
    let still_valid = session
        .cache_mgr
        .build_repository()
        .load_matrix(Bracket::Legend)
        .unwrap();
    assert!(still_valid.hero(HeroId(1)).unwrap().role_winrates[0] > 0.62);

    // No leftover temp staging files in cache_dir
    for entry in std::fs::read_dir(&tmp_dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(!name.contains(".tmp."), "Leftover temp file found: {name}");
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn session_suppresses_vision_during_wait_for_players_and_resumes_in_hero_selection() {
    use std::time::Duration;
    use truesight_adapters::vision::{
        BoundedVisionWorker, RgbFrame, ScreenFrameSource, TopBarSlotRect,
        compute_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::HeroId;
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
    paint_slot(
        &mut rgb,
        w,
        rad_r[0],
        &atlas.heroes[&HeroId(1)],
        atlas.hero_width,
        atlas.hero_height,
    );
    paint_slot(
        &mut rgb,
        w,
        dire_r[0],
        &atlas.heroes[&HeroId(94)],
        atlas.hero_width,
        atlas.hero_height,
    );

    let mut worker = BoundedVisionWorker::spawn_with_source(StationarySource(RgbFrame {
        width: w,
        height: h,
        source_height: None,
        rgb,
    }))
    .unwrap();

    let mut session = AppSession::new().unwrap();
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD".into(),
        game_time_secs: -90,
    });
    for _ in 0..8 {
        session.poll_live_sources(None, Some(&mut worker));
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(
        session.draft.allies.iter().flatten().count(),
        0,
        "Vision must remain suppressed during WAIT_FOR_PLAYERS_TO_LOAD"
    );
    assert_eq!(
        session.draft.enemies.iter().flatten().count(),
        0,
        "Vision must remain suppressed during WAIT_FOR_PLAYERS_TO_LOAD"
    );

    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
        game_time_secs: -60,
    });
    for _ in 0..40 {
        session.poll_live_sources(None, Some(&mut worker));
        if session.draft.allies.iter().flatten().count() == 1
            && session.draft.enemies.iter().flatten().count() == 1
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(session.draft.allies.iter().flatten().count(), 1);
    assert_eq!(session.draft.enemies.iter().flatten().count(), 1);
}

#[test]
fn live_gsi_init_and_custom_setup_suppress_vision_and_restore_across_match_transitions() {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;
    use truesight_adapters::gsi::BoundedGsiListener;
    use truesight_adapters::vision::{
        BoundedVisionWorker, RgbFrame, ScreenFrameSource, compute_topbar_slot_rects,
        parse_hero_atlas,
    };
    use truesight_core::domain::{HeroId, Side};

    struct StationarySource(RgbFrame);
    impl ScreenFrameSource for StationarySource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            Ok(Some(self.0.clone()))
        }
    }

    fn paint_slot(
        frame: &mut [u8],
        frame_w: usize,
        rect: truesight_adapters::vision::TopBarSlotRect,
        hero_rgb: &[u8],
        src_w: usize,
        src_h: usize,
    ) {
        for dy in 0..rect.height {
            let sy = (dy * src_h) / rect.height;
            for dx in 0..rect.width {
                let sx = (dx * src_w) / rect.width;
                let s = (sy * src_w + sx) * 3;
                let d = ((rect.y + dy) * frame_w + (rect.x + dx)) * 3;
                frame[d..d + 3].copy_from_slice(&hero_rgb[s..s + 3]);
            }
        }
    }

    let atlas = parse_hero_atlas().unwrap();
    let (w, h) = (1920usize, 1080usize);
    let mut rgb = vec![0u8; w * h * 3];
    let (rad_r, dire_r) = compute_topbar_slot_rects(w, h);
    paint_slot(
        &mut rgb,
        w,
        rad_r[0],
        &atlas.heroes[&HeroId(1)],
        atlas.hero_width,
        atlas.hero_height,
    );
    paint_slot(
        &mut rgb,
        w,
        dire_r[0],
        &atlas.heroes[&HeroId(94)],
        atlas.hero_width,
        atlas.hero_height,
    );

    let mut listener = BoundedGsiListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr();
    let send_state = |state: &str, clock: i32| {
        let body = format!(r#"{{"map":{{"game_state":"{state}","clock_time":{clock}}}}}"#);
        let req = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        s.write_all(req.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut resp = String::new();
        let _ = s.read_to_string(&mut resp);
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "Expected 200 OK, got: {resp}"
        );
    };

    let mut worker = BoundedVisionWorker::spawn_with_source(StationarySource(RgbFrame {
        width: w,
        height: h,
        source_height: None,
        rgb,
    }))
    .unwrap();

    // 1. Absent GSI (None) allows vision picks normally (VisionCaptureMode::Any).
    let mut absent_gsi_session = AppSession::new().unwrap();
    for _ in 0..30 {
        absent_gsi_session.poll_live_sources(None, Some(&mut worker));
        if absent_gsi_session.draft.allies.iter().flatten().count() == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(absent_gsi_session.draft.allies.iter().flatten().count(), 1);

    // 2. Fresh session in INIT and CUSTOM_GAME_SETUP receives 0 new vision picks (no reset_draft()).
    let mut session = AppSession::new().unwrap();
    send_state("DOTA_GAMERULES_STATE_INIT", 0);
    for _ in 0..8 {
        session.poll_live_sources(Some(&mut listener), Some(&mut worker));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        session.last_gsi_state.contains("DOTA_GAMERULES_STATE_INIT"),
        "Expected INIT state in AppSession, got: {}",
        session.last_gsi_state
    );
    assert_eq!(
        session.draft.allies.iter().flatten().count(),
        0,
        "INIT must prevent new vision picks without requiring manual reset_draft()"
    );

    send_state("DOTA_GAMERULES_STATE_CUSTOM_GAME_SETUP", -10);
    for _ in 0..8 {
        session.poll_live_sources(Some(&mut listener), Some(&mut worker));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        session
            .last_gsi_state
            .contains("DOTA_GAMERULES_STATE_CUSTOM_GAME_SETUP")
    );
    assert_eq!(
        session.draft.allies.iter().flatten().count(),
        0,
        "CUSTOM_GAME_SETUP must prevent new vision picks without requiring manual reset_draft()"
    );

    // 3. HERO_SELECTION lifts suppression and populates match picks; PRE_GAME/GAME_IN_PROGRESS continue.
    send_state("DOTA_GAMERULES_STATE_HERO_SELECTION", 45);
    for _ in 0..40 {
        session.poll_live_sources(Some(&mut listener), Some(&mut worker));
        if session.draft.allies.iter().flatten().count() == 1
            && session.draft.enemies.iter().flatten().count() == 1
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(session.draft.allies.iter().flatten().count(), 1);
    assert_eq!(session.draft.enemies.iter().flatten().count(), 1);

    for (state, clock) in [
        ("DOTA_GAMERULES_STATE_PRE_GAME", -20),
        ("DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", 120),
    ] {
        send_state(state, clock);
        session.poll_live_sources(Some(&mut listener), Some(&mut worker));
        assert!(
            session.last_gsi_state.contains(state),
            "Expected state {state}"
        );
    }

    // 4. Existing draft picks stay preserved across INIT / CUSTOM_GAME_SETUP / POST_GAME / DISCONNECT
    //    for post-game review or manual testing, until HERO_SELECTION automatically resets them.
    session.set_slot_manual(Side::Allies, 3, HeroId(2));
    assert!(session.draft.allies[3].is_some());

    for (state, clock) in [
        ("DOTA_GAMERULES_STATE_POST_GAME", 2000),
        ("DOTA_GAMERULES_STATE_DISCONNECT", 2005),
        ("DOTA_GAMERULES_STATE_INIT", 0),
        ("DOTA_GAMERULES_STATE_CUSTOM_GAME_SETUP", -5),
    ] {
        send_state(state, clock);
        for _ in 0..4 {
            session.poll_live_sources(Some(&mut listener), Some(&mut worker));
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(
            session.last_gsi_state.contains(state),
            "Expected state {state}"
        );
        assert!(
            session.draft.allies[3].is_some(),
            "Prior draft picks must stay preserved during {state}"
        );
    }

    send_state("DOTA_GAMERULES_STATE_HERO_SELECTION", 50);
    for _ in 0..40 {
        session.poll_live_sources(Some(&mut listener), Some(&mut worker));
        if session.draft.allies[3].is_none() && session.draft.allies.iter().flatten().count() == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        session.draft.allies[3].is_none(),
        "Transitioning to HERO_SELECTION must automatically clear stale prior-match picks"
    );
    assert_eq!(session.draft.allies.iter().flatten().count(), 1);
}

#[test]
fn session_and_cli_surface_capture_availability_and_allow_manual_picks_when_unavailable() {
    use truesight_adapters::vision::{CaptureAvailability, CaptureBackendKind};

    let mut session = AppSession::new().unwrap();
    let unavailable = CaptureAvailability::Unavailable {
        backend: CaptureBackendKind::WindowsNative,
        reason: "Native Windows capture backend is not implemented yet (#12); manual pick mode & GSI remain active.".into(),
    };
    session.set_vision_availability(&unavailable);
    assert_eq!(session.vision_status, "CAP: MANUAL");
    assert!(session.vision_detail.contains("#12"));
    assert!(!session.vision_available);

    // Manual draft entry & polling with vision_worker = None must remain completely functional.
    assert!(crate::execute_command(&mut session, "ally 1 anti-mage").is_ok());
    session.poll_live_sources(None, None);
    assert_eq!(
        session.draft.allies[0].map(|p| p.hero_id),
        Some(truesight_core::domain::HeroId(1))
    );

    let cli_out = crate::render_dashboard_string(&session, false);
    assert!(
        cli_out.contains("Vision: CAP: MANUAL"),
        "CLI header must surface capture status explicitly: {cli_out}"
    );

    let available = CaptureAvailability::Available {
        backend: CaptureBackendKind::X11,
        detail: "X11/XWayland (xprop + import)".into(),
    };
    session.set_vision_availability(&available);
    assert_eq!(session.vision_status, "CAP: X11");
    assert!(session.vision_available);

    type HealthStep =
        Result<Option<truesight_adapters::vision::RgbFrame>, truesight_core::ports::PortError>;
    type HealthQueue = std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<HealthStep>>>;

    // Runtime worker health updates UI/CLI status from WaitingForGame -> ERR -> LIVE cleanly.
    struct HealthSequence(HealthQueue);
    impl truesight_adapters::vision::ScreenFrameSource for HealthSequence {
        fn capture_frame(
            &mut self,
        ) -> Result<Option<truesight_adapters::vision::RgbFrame>, truesight_core::ports::PortError>
        {
            self.0.lock().unwrap().pop_front().unwrap_or_else(|| {
                Ok(Some(truesight_adapters::vision::RgbFrame::new(
                    1280,
                    60,
                    vec![0; 1280 * 60 * 3],
                )))
            })
        }
    }

    let steps = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from([
        Err(truesight_core::ports::PortError(
            "Windows capture failed: handle lost".into(),
        )),
    ])));
    let mut worker = truesight_adapters::vision::BoundedVisionWorker::spawn_with_source(
        HealthSequence(std::sync::Arc::clone(&steps)),
    )
    .unwrap();
    let win_avail = CaptureAvailability::Available {
        backend: CaptureBackendKind::WindowsNative,
        detail: "Windows Win32/GDI (xcap)".into(),
    };
    session.set_vision_availability(&win_avail);

    let err_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < err_deadline {
        session.poll_live_sources(None, Some(&mut worker));
        if session.vision_status == "WIN32: ERR" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(session.vision_status, "WIN32: ERR");
    assert!(!session.vision_available);
    assert!(session.vision_error);
    assert!(session.vision_detail.contains("handle lost"));
    assert!(
        session
            .vision_detail
            .to_ascii_lowercase()
            .contains("manual")
    );
    let cli_err1 = crate::render_dashboard_string(&session, false);
    assert!(
        cli_err1.contains("Vision: WIN32: ERR") && cli_err1.contains("handle lost"),
        "CLI dashboard must render failure reason alongside ERR badge: {cli_err1}"
    );

    // Changing the failure reason under the same `WIN32: ERR` badge must trigger redraw & update CLI output.
    steps
        .lock()
        .unwrap()
        .push_back(Err(truesight_core::ports::PortError(
            "Windows window enumeration failed: access denied".into(),
        )));
    worker.reset();
    let mut changed_on_second_err = false;
    let err2_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < err2_deadline {
        if session.poll_live_sources(None, Some(&mut worker))
            && session.vision_detail.contains("access denied")
        {
            changed_on_second_err = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        changed_on_second_err,
        "Changing error detail under same WIN32: ERR badge must trigger redraw"
    );
    let cli_err2 = crate::render_dashboard_string(&session, false);
    assert!(cli_err2.contains("access denied"));

    steps
        .lock()
        .unwrap()
        .push_back(Ok(Some(truesight_adapters::vision::RgbFrame::new(
            1280,
            60,
            vec![0; 1280 * 60 * 3],
        ))));
    worker.reset();
    let live_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < live_deadline {
        session.poll_live_sources(None, Some(&mut worker));
        if session.vision_status == "WIN32: LIVE" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(session.vision_status, "WIN32: LIVE");
    assert!(session.vision_available);
    assert!(crate::render_dashboard_string(&session, false).contains("Vision: WIN32: LIVE"));
}

#[cfg(unix)]
#[test]
fn linux_install_desktop_prefers_sibling_binary_and_escapes_ampersand_and_spaces() {
    use std::os::unix::fs::PermissionsExt;

    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");

    let base_tmp = std::env::temp_dir().join(format!(
        "ts_desktop_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let extract_dir = base_tmp.join("TrueSight & Dota");
    let fake_home = base_tmp.join("home");
    let fake_xdg = base_tmp.join("xdg_data");
    std::fs::create_dir_all(&extract_dir).unwrap();
    std::fs::create_dir_all(&fake_home).unwrap();
    std::fs::create_dir_all(&fake_xdg).unwrap();

    for (src_rel, dst_name) in [
        ("packaging/linux/install-desktop.sh", "install-desktop.sh"),
        (
            "packaging/linux/truesight-dota.desktop",
            "truesight-dota.desktop",
        ),
        ("packaging/icons/truesight-dota.png", "truesight-dota.png"),
    ] {
        std::fs::copy(repo_root.join(src_rel), extract_dir.join(dst_name)).unwrap();
    }
    let script_path = extract_dir.join("install-desktop.sh");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let sibling_bin = extract_dir.join("truesight-dota");
    std::fs::write(&sibling_bin, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&sibling_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Run without arguments and without truesight-dota on PATH: must resolve sibling binary intact.
    let out = std::process::Command::new(&script_path)
        .env("HOME", &fake_home)
        .env("XDG_DATA_HOME", &fake_xdg)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run install-desktop.sh");
    assert!(
        out.status.success(),
        "install-desktop.sh failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let desktop_path = fake_xdg.join("applications/truesight-dota.desktop");
    let desktop_content = std::fs::read_to_string(&desktop_path).unwrap();
    let expected_sibling = format!("Exec=\"{}\"", sibling_bin.display());
    assert!(
        desktop_content.lines().any(|l| l == expected_sibling),
        "Expected exact line `{expected_sibling}`, got:\n{desktop_content}"
    );
    assert!(
        fake_xdg
            .join("icons/hicolor/128x128/apps/truesight-dota.png")
            .exists()
    );

    // Explicit argument with spaces, `&`, literal `\`, `"`, `$`, and `%` must satisfy both
    // the Desktop Entry `string` escaping layer and `Exec` double-quoted escaping rules.
    let tricky_arg = extract_dir.join(r#"ts\dir & "$100%"#);
    std::fs::write(&tricky_arg, "#!/bin/sh\nexit 0\n").unwrap();
    let out2 = std::process::Command::new(&script_path)
        .arg(&tricky_arg)
        .env("HOME", &fake_home)
        .env("XDG_DATA_HOME", &fake_xdg)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run install-desktop.sh with explicit tricky path");
    assert!(
        out2.status.success(),
        "install-desktop.sh tricky path failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let desktop2 = std::fs::read_to_string(&desktop_path).unwrap();
    let expected_escaped = format!(
        r#"Exec="{}/ts\\\\dir & \\"\\$100%%""#,
        extract_dir.display()
    );
    assert!(
        desktop2.lines().any(|l| l == expected_escaped),
        "Expected escaped line `{expected_escaped}`, got:\n{desktop2}"
    );
    if std::path::Path::new("/usr/bin/desktop-file-validate").exists() {
        let val = std::process::Command::new("/usr/bin/desktop-file-validate")
            .arg(&desktop_path)
            .output()
            .expect("run desktop-file-validate");
        assert!(
            val.status.success(),
            "desktop-file-validate rejected generated .desktop:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&val.stdout),
            String::from_utf8_lossy(&val.stderr)
        );
    }

    // Embedded or trailing newline/CR in target path must fail before modifying existing launcher.
    for (kind, bad_arg) in [
        (
            "embedded_lf",
            format!("{}/bad\npath", extract_dir.display()),
        ),
        ("trailing_lf", "/tmp/app\n".to_string()),
        ("trailing_cr", "/tmp/app\r".to_string()),
    ] {
        let out_bad = std::process::Command::new(&script_path)
            .arg(&bad_arg)
            .env("HOME", &fake_home)
            .env("XDG_DATA_HOME", &fake_xdg)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run install-desktop.sh with invalid CR/LF path");
        assert!(
            !out_bad.status.success(),
            "install-desktop.sh must reject {kind} in Exec path"
        );
        assert_eq!(
            std::fs::read_to_string(&desktop_path).unwrap(),
            desktop2,
            "Rejected {kind} path must leave existing .desktop file unchanged"
        );
    }

    let _ = std::fs::remove_dir_all(&base_tmp);
}

#[test]
fn player_account_link_auto_syncs_stale_player_cache_from_gsi_and_cli() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::HeroId;
    use truesight_core::ports::DraftEvent;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_auto_sync_player_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let now = LocalCacheManager::now_unix();
    let fresh_meta = r#"[{"id": 1, "1_pick": 1000, "1_win": 610, "2_pick": 1000, "2_win": 610, "3_pick": 1000, "3_win": 610, "4_pick": 1000, "4_win": 610, "5_pick": 1000, "5_win": 610, "6_pick": 1000, "6_win": 610, "7_pick": 1000, "7_win": 610}]"#;

    let mut mgr = LocalCacheManager::with_dir(tmp_dir.clone(), 111_222_333);
    mgr.write_cached_files(Some(fresh_meta), None, now).unwrap();
    mgr.set_account_id(None, false);

    let mut session = AppSession::new().unwrap();
    session.cache_mgr = mgr;
    session.reload_repository();

    let sync_calls = Arc::new(AtomicUsize::new(0));
    let sync_calls_bg = Arc::clone(&sync_calls);
    let bg_tmp_dir = tmp_dir.clone();
    session.set_background_sync_runner(move |bg_mgr, force| {
        assert!(!force, "auto-sync on account link must pass force=false");
        let call_n = sync_calls_bg.fetch_add(1, Ordering::SeqCst) + 1;
        let acct = bg_mgr
            .player_account_id
            .expect("auto-sync only runs when an account is linked");
        let matches_json = format!(
            r#"[{{"hero_id": 1, "player_slot": 0, "radiant_win": true, "start_time": {now}}}]"#
        );
        let heroes_json =
            format!(r#"[{{"hero_id": 1, "last_played": {now}, "games": 25, "win": 20}}]"#);
        bg_mgr
            .write_cached_files(None, Some(&heroes_json), LocalCacheManager::now_unix())
            .map_err(|e| e.0)?;
        let matches_path = bg_tmp_dir
            .join("players")
            .join(acct.to_string())
            .join("player_matches.json");
        std::fs::write(matches_path, matches_json).map_err(|e| e.to_string())?;
        Ok(format!("Synced player #{acct} (call {call_n})"))
    });

    // 1. GSI PlayerSteamIdDetected transitions from None -> Some(steam32) and auto-syncs stale player cache.
    session.apply_event(DraftEvent::PlayerSteamIdDetected {
        steam32: 123_456_789,
    });
    assert_eq!(session.cache_mgr.player_account_id, Some(123_456_789));
    assert!(
        session.sync_in_flight,
        "PlayerSteamIdDetected must start background sync when new account cache is stale"
    );

    for _ in 0..100 {
        session.poll_live_sources(None, None);
        if !session.sync_in_flight {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!session.sync_in_flight);
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        session.matrix.hero(HeroId(1)).unwrap().personal_matches,
        25,
        "Comfort v3 personal matches must reload after GSI auto-sync"
    );
    assert!(
        session.matrix.hero(HeroId(1)).unwrap().role_winrates[0] > 0.59,
        "Existing fresh Meta cache must stay intact"
    );

    // 2. Repeated GSI packet for the same already-fresh account must NOT re-trigger sync.
    session.apply_event(DraftEvent::PlayerSteamIdDetected {
        steam32: 123_456_789,
    });
    assert!(!session.sync_in_flight);
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);

    // 3. CLI `account none` unlinks without starting a sync.
    assert!(execute_command(&mut session, "account none").unwrap());
    assert_eq!(session.cache_mgr.player_account_id, None);
    assert!(!session.sync_in_flight);
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);

    // 4. CLI `account <id>` links explicit account and auto-syncs its stale cache.
    assert!(execute_command(&mut session, "account 76561198061460213").unwrap());
    assert_eq!(session.cache_mgr.player_account_id, Some(101_194_485));
    assert!(
        session.sync_in_flight,
        "CLI account command must start background sync when player cache is stale"
    );
    for _ in 0..100 {
        session.poll_live_sources(None, None);
        if !session.sync_in_flight {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!session.sync_in_flight);
    assert_eq!(sync_calls.load(Ordering::SeqCst), 2);
    assert_eq!(session.matrix.hero(HeroId(1)).unwrap().personal_matches, 25);

    // 5. Explicit override still blocks GSI SteamID overwrites and does not trigger sync.
    session.apply_event(DraftEvent::PlayerSteamIdDetected {
        steam32: 555_666_777,
    });
    assert_eq!(session.cache_mgr.player_account_id, Some(101_194_485));
    assert!(!session.sync_in_flight);
    assert_eq!(sync_calls.load(Ordering::SeqCst), 2);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn reapplying_same_account_preserves_in_flight_sync_while_switching_cancels_it() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::Duration;
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::HeroId;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_reapply_same_acct_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let now = LocalCacheManager::now_unix();
    let fresh_meta = r#"[{"id": 1, "1_pick": 1000, "1_win": 610, "2_pick": 1000, "2_win": 610, "3_pick": 1000, "3_win": 610, "4_pick": 1000, "4_win": 610, "5_pick": 1000, "5_win": 610, "6_pick": 1000, "6_win": 610, "7_pick": 1000, "7_win": 610}]"#;

    let mut mgr = LocalCacheManager::with_dir(tmp_dir.clone(), 0);
    mgr.write_cached_files(Some(fresh_meta), None, now).unwrap();
    mgr.set_account_id(None, false);

    let mut session = AppSession::new().unwrap();
    session.cache_mgr = mgr;
    session.reload_repository();

    let (started_tx, started_rx) = mpsc::sync_channel::<()>(2);
    let (gate_tx, gate_rx) = mpsc::sync_channel::<()>(0);
    let gate_rx = Arc::new(Mutex::new(Some(gate_rx)));
    let sync_calls = Arc::new(AtomicUsize::new(0));
    let first_was_cancelled = Arc::new(AtomicBool::new(false));

    let sync_calls_bg = Arc::clone(&sync_calls);
    let first_was_cancelled_bg = Arc::clone(&first_was_cancelled);
    let bg_tmp_dir = tmp_dir.clone();
    session.set_background_sync_runner(move |bg_mgr, force| {
        assert!(!force);
        let call_n = sync_calls_bg.fetch_add(1, Ordering::SeqCst) + 1;
        let acct = bg_mgr.player_account_id.expect("linked account");
        if call_n == 1 {
            let _ = started_tx.send(());
            if let Some(rx) = gate_rx.lock().unwrap().take() {
                let _ = rx.recv();
            }
            if bg_mgr.is_cancelled() {
                first_was_cancelled_bg.store(true, Ordering::SeqCst);
                return Err("Sync cancelled".into());
            }
        }
        let heroes_json =
            format!(r#"[{{"hero_id": 1, "last_played": {now}, "games": 18, "win": 15}}]"#);
        let matches_json = format!(
            r#"[{{"hero_id": 1, "player_slot": 0, "radiant_win": true, "start_time": {now}}}]"#
        );
        bg_mgr
            .write_cached_files(None, Some(&heroes_json), LocalCacheManager::now_unix())
            .map_err(|e| e.0)?;
        let matches_path = bg_tmp_dir
            .join("players")
            .join(acct.to_string())
            .join("player_matches.json");
        std::fs::write(matches_path, matches_json).map_err(|e| e.to_string())?;
        Ok(format!("Synced #{acct} (call {call_n})"))
    });

    // 1. Start initial sync for Steam32 101_194_485 and hold it in flight.
    assert!(execute_command(&mut session, "account 101194485").unwrap());
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(session.sync_in_flight);
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);

    // 2. Reapply same account (via Steam64 and non-explicit mode): must keep first worker alive and not spawn a second.
    assert!(execute_command(&mut session, "account 76561198061460213").unwrap());
    session.set_account_id(Some(101_194_485), false);
    let _ = session.sync_if_player_stale();
    assert!(session.sync_in_flight);
    assert_eq!(session.db_status, "DB [SYNCING...]");
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);

    // 3. Release the held first worker and confirm it completes without cancellation and populates Comfort v3.
    gate_tx.send(()).unwrap();
    for _ in 0..100 {
        session.poll_live_sources(None, None);
        if !session.sync_in_flight {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!session.sync_in_flight);
    assert!(!first_was_cancelled.load(Ordering::SeqCst));
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);
    assert_eq!(session.matrix.hero(HeroId(1)).unwrap().personal_matches, 18);

    // 4. Switching to a different account while a sync is in flight must still cancel the prior generation.
    let (started2_tx, started2_rx) = mpsc::sync_channel::<()>(1);
    let (gate2_tx, gate2_rx) = mpsc::sync_channel::<()>(0);
    let gate2_rx = Arc::new(Mutex::new(Some(gate2_rx)));
    let second_cancelled = Arc::new(AtomicBool::new(false));
    let second_cancelled_bg = Arc::clone(&second_cancelled);
    let (done2_tx, done2_rx) = mpsc::sync_channel::<()>(1);
    session.set_background_sync_runner(move |bg_mgr, _force| {
        if bg_mgr.player_account_id == Some(222_333_444) {
            let _ = started2_tx.send(());
            if let Some(rx) = gate2_rx.lock().unwrap().take() {
                let _ = rx.recv();
            }
            second_cancelled_bg.store(bg_mgr.is_cancelled(), Ordering::SeqCst);
            let _ = done2_tx.send(());
            return Err("Cancelled".into());
        }
        Ok("Switched ok".into())
    });

    assert!(execute_command(&mut session, "account 222333444").unwrap());
    started2_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(execute_command(&mut session, "account none").unwrap());
    assert!(!session.sync_in_flight);
    gate2_tx.send(()).unwrap();
    done2_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        second_cancelled.load(Ordering::SeqCst),
        "Unlinking or switching account must cancel the prior in-flight sync"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn windows_packaging_separates_console_cli_and_console_free_gui_entrypoints() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");

    let app_toml =
        std::fs::read_to_string(manifest_dir.join("Cargo.toml")).expect("read app Cargo.toml");
    assert!(
        !app_toml.contains("winsafe"),
        "console/GUI entrypoint split must not require winsafe"
    );
    assert!(
        app_toml.contains("name = \"truesight-dota-gui\"")
            && app_toml.contains("src/bin/truesight-dota-gui.rs")
            && app_toml.contains("required-features = [\"gui\"]"),
        "truesight-app Cargo.toml must declare the gui-only truesight-dota-gui binary"
    );

    let main_rs =
        std::fs::read_to_string(manifest_dir.join("src/main.rs")).expect("read src/main.rs");
    assert!(
        !main_rs.contains("windows_subsystem"),
        "truesight-dota CLI/default entrypoint must stay in the Windows console subsystem"
    );
    assert!(
        main_rs.contains("truesight_app::run_app()"),
        "src/main.rs must delegate to shared truesight_app::run_app()"
    );

    let gui_rs = std::fs::read_to_string(manifest_dir.join("src/bin/truesight-dota-gui.rs"))
        .expect("read src/bin/truesight-dota-gui.rs");
    assert!(
        gui_rs.contains("#![cfg_attr(target_os = \"windows\", windows_subsystem = \"windows\")]"),
        "truesight-dota-gui must opt into the Windows GUI subsystem to suppress the console window"
    );
    assert!(
        gui_rs.contains("truesight_app::run_app()"),
        "src/bin/truesight-dota-gui.rs must delegate to shared truesight_app::run_app()"
    );

    let pkg_script =
        std::fs::read_to_string(repo_root.join("packaging/windows/build-release-packages.ps1"))
            .expect("read build-release-packages.ps1");
    assert!(
        pkg_script.contains("truesight-dota-gui.exe"),
        "Windows packaging script must stage truesight-dota-gui.exe alongside truesight-dota.exe"
    );

    let wxs = std::fs::read_to_string(repo_root.join("packaging/windows/Package.wxs"))
        .expect("read Package.wxs");
    assert!(
        wxs.contains("GuiExecutable")
            && wxs.contains("[INSTALLFOLDER]truesight-dota-gui.exe")
            && wxs.contains("LaunchTrueSight"),
        "MSI installer must ship truesight-dota-gui.exe and point the Start Menu shortcut to it"
    );
}

#[test]
fn bracket_auto_selects_from_rank_tier_and_persists_manual_selection() {
    use truesight_adapters::sync::{
        AccountLinkConfig, BracketConfig, LocalCacheManager, load_app_config_from,
        save_account_config_to,
    };
    use truesight_core::domain::Bracket;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_app_bracket_persist_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cache_dir = tmp_dir.join("cache");
    let cfg_path = tmp_dir.join("config.toml");

    // 1. Auto mode with cached rank_tier=73 (Divine) boots directly on Divine instead of Legend.
    let mgr = LocalCacheManager::with_dir(cache_dir.clone(), 127_255_741);
    mgr.write_cached_profile_json(r#"{"rank_tier":73}"#)
        .unwrap();
    let s1 = AppSession::with_cache_and_config(mgr.clone(), Some(cfg_path.clone())).unwrap();
    assert_eq!(s1.draft.bracket, Bracket::Divine);
    assert_eq!(s1.bracket_config, BracketConfig::Auto);

    // 2. Background sync completion with updated rank_tier=62 (Ancient) updates Auto bracket.
    let mut s_sync = AppSession::with_cache_and_config(
        LocalCacheManager::with_dir(cache_dir.clone(), 127_255_741),
        Some(cfg_path.clone()),
    )
    .unwrap();
    s_sync.set_background_sync_runner(|bg_mgr, _force| {
        bg_mgr
            .write_cached_profile_json(r#"{"rank_tier":62}"#)
            .map_err(|e| e.0)?;
        Ok("Synced player profile".to_string())
    });
    assert!(s_sync.start_background_sync(true));
    for _ in 0..100 {
        s_sync.poll_live_sources(None, None);
        if !s_sync.sync_in_flight {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(s_sync.draft.bracket, Bracket::Ancient);
    assert_eq!(s_sync.bracket_config, BracketConfig::Auto);

    // 3. Manual bracket override persists in config.toml alongside account_id and survives restart,
    // even when cached/synced OpenDota rank_tier reports a different medal or null.
    save_account_config_to(&cfg_path, AccountLinkConfig::Explicit(127_255_741)).unwrap();
    s_sync.set_bracket(Bracket::Immortal);
    assert_eq!(
        s_sync.bracket_config,
        BracketConfig::Explicit(Bracket::Immortal)
    );
    let persisted = load_app_config_from(&cfg_path);
    assert_eq!(
        persisted.account_link,
        AccountLinkConfig::Explicit(127_255_741)
    );
    assert_eq!(
        persisted.bracket,
        BracketConfig::Explicit(Bracket::Immortal)
    );

    let mut restarted = AppSession::with_cache_and_config(
        LocalCacheManager::with_dir(cache_dir.clone(), 127_255_741),
        Some(cfg_path.clone()),
    )
    .unwrap();
    assert_eq!(restarted.draft.bracket, Bracket::Immortal);
    assert_eq!(
        restarted.bracket_config,
        BracketConfig::Explicit(Bracket::Immortal)
    );

    // Explicit manual bracket must NOT be overwritten when sync reports rank_tier=33 or null.
    restarted.set_background_sync_runner(|bg_mgr, _force| {
        bg_mgr
            .write_cached_profile_json(r#"{"rank_tier":33}"#)
            .map_err(|e| e.0)?;
        Ok("Synced lower tier".to_string())
    });
    assert!(restarted.start_background_sync(true));
    for _ in 0..100 {
        restarted.poll_live_sources(None, None);
        if !restarted.sync_in_flight {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(restarted.draft.bracket, Bracket::Immortal);

    // 4. `bracket auto` CLI command clears explicit override and switches back to cached rank_tier.
    assert!(execute_command(&mut restarted, "bracket auto").unwrap());
    assert_eq!(restarted.bracket_config, BracketConfig::Auto);
    assert_eq!(restarted.draft.bracket, Bracket::Crusader);
    assert_eq!(load_app_config_from(&cfg_path).bracket, BracketConfig::Auto);

    // 5. Uncalibrated (rank_tier: null) falls back cleanly to Legend on startup.
    mgr.write_cached_profile_json(r#"{"rank_tier":null}"#)
        .unwrap();
    let uncal = AppSession::with_cache_and_config(mgr, Some(cfg_path.clone())).unwrap();
    assert_eq!(uncal.draft.bracket, Bracket::Legend);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
