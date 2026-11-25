use super::{
    AppSession, CliInputEvent, execute_command, parse_account_id_arg, render_dashboard_string,
    run_cli_loop, spawn_input_reader,
};

fn wait_for_sync_until<F>(session: &mut AppSession, mut ready: F)
where
    F: FnMut() -> bool,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        session.poll_live_sources(None, None);
        if !session.sync_in_flight && ready() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn wait_for_sync_idle(session: &mut AppSession) {
    wait_for_sync_until(session, || true);
}

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
    wait_for_sync_idle(&mut session);
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
    wait_for_sync_idle(&mut session);
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

    wait_for_sync_idle(&mut session);
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
    wait_for_sync_idle(&mut session);
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
        capture_strip_rect: None,
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

#[test]
fn vision_replaces_hero_in_same_portrait_slot() {
    use truesight_adapters::vision::{
        HeroVisionCatalog, RgbFrame, ScreenFrameSource, VisionCaptureAdapter,
        compute_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::HeroId;
    use truesight_core::ports::{DraftEventSource, PortError};

    struct Frames(std::collections::VecDeque<RgbFrame>);
    impl ScreenFrameSource for Frames {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(self.0.pop_front())
        }
    }
    let atlas = parse_hero_atlas().unwrap();
    let frame = |hero| {
        let (w, h) = (1920, 1080);
        let mut rgb = vec![0; w * h * 3];
        let (rects, _) = compute_topbar_slot_rects(w, h);
        for (slot, id) in [(0, hero), (1, 2)] {
            let rect = rects[slot];
            let portrait = &atlas.heroes[&HeroId(id)];
            for y in 0..rect.height {
                for x in 0..rect.width {
                    let src = ((y * atlas.hero_height / rect.height) * atlas.hero_width
                        + x * atlas.hero_width / rect.width)
                        * 3;
                    let dst = ((rect.y + y) * w + rect.x + x) * 3;
                    rgb[dst..dst + 3].copy_from_slice(&portrait[src..src + 3]);
                }
            }
        }
        RgbFrame::new(w, h, rgb)
    };
    let luna = frame(48);
    let nyx = frame(88);
    let blank = RgbFrame::new(1920, 1080, vec![0; 1920 * 1080 * 3]);
    let mut adapter = VisionCaptureAdapter::new(
        Frames([luna.clone(), luna, blank, nyx.clone(), nyx.clone(), nyx].into()),
        HeroVisionCatalog::from_embedded().unwrap(),
    );
    let mut session = AppSession::new().unwrap();
    for _ in 0..4 {
        session.apply_vision_events(adapter.poll_events().unwrap());
        assert!(
            session
                .draft
                .allies
                .iter()
                .flatten()
                .any(|p| p.hero_id == HeroId(48))
        );
        assert_eq!(session.draft.allies.iter().flatten().count(), 2);
    }
    session.apply_vision_events(adapter.poll_events().unwrap());
    let ids = || {
        session
            .draft
            .allies
            .iter()
            .flatten()
            .map(|p| p.hero_id)
            .collect::<Vec<_>>()
    };
    assert!(
        !ids().contains(&HeroId(48)),
        "Confirmed replacement must retire stale Luna"
    );
    assert!(ids().contains(&HeroId(88)));
    assert!(ids().contains(&HeroId(2)));
    assert_eq!(ids().len(), 2);
    let before = session.draft;
    session.apply_vision_events(adapter.poll_events().unwrap());
    assert_eq!(session.draft.allies, before.allies);
    session.reset_draft();
    adapter.reset_state();
    assert_eq!(session.draft.allies.iter().flatten().count(), 0);
}

#[test]
fn vision_cached_observations_do_not_undo_manual_replacement() {
    use truesight_core::domain::{HeroId, Side};
    let mut session = AppSession::new().unwrap();
    session.apply_vision_events(vec![visual_pick(Side::Allies, 0, 48)]);
    let slot = session
        .draft
        .allies
        .iter()
        .position(|p| p.is_some())
        .unwrap();
    session.set_slot_manual(Side::Allies, slot, HeroId(88));
    session.apply_vision_events(vec![]);
    assert!(
        !session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48)),
        "Empty batch resurrected manually replaced Luna"
    );
    session.apply_vision_events(vec![visual_pick(Side::Enemies, 0, 2)]);
    assert!(
        !session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48)),
        "Unrelated batch resurrected manually replaced Luna"
    );
    assert_eq!(session.draft.allies[slot].unwrap().hero_id, HeroId(88));
}

#[test]
fn vision_cached_observations_do_not_undo_gsi_side_correction() {
    use truesight_core::domain::{HeroId, Side};
    let mut session = AppSession::new().unwrap();
    session.apply_vision_events(vec![visual_pick(Side::Allies, 0, 48)]);
    session.apply_event(visual_pick(Side::Enemies, 0, 48));
    session.apply_vision_events(vec![]);
    assert!(
        session
            .draft
            .enemies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48)),
        "Empty batch undid GSI side correction"
    );
    session.apply_vision_events(vec![visual_pick(Side::Allies, 1, 2)]);
    assert!(
        session
            .draft
            .enemies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48)),
        "Unrelated batch undid GSI side correction"
    );
    assert!(
        !session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48))
    );
}

#[test]
fn vision_idle_poll_preserves_manually_selected_role_after_gsi_identity() {
    use truesight_core::domain::Role;
    use truesight_core::ports::DraftEvent;
    let mut session = AppSession::new().unwrap();
    session.apply_event(DraftEvent::LocalPlayerHeroDetected {
        hero_slug: "npc_dota_hero_night_stalker".into(),
    });
    assert_eq!(session.draft.target_role, Role::Offlane);
    session.draft.target_role = Role::Mid;
    session.apply_vision_events(vec![]);
    assert_eq!(session.draft.target_role, Role::Mid);
}

// Each capture is gated so faction changes occur at a known worker batch boundary.
struct VisionGatedSource {
    entered: std::sync::mpsc::Sender<()>,
    frames: std::sync::mpsc::Receiver<truesight_adapters::vision::RgbFrame>,
}

impl truesight_adapters::vision::ScreenFrameSource for VisionGatedSource {
    fn capture_frame(
        &mut self,
    ) -> Result<Option<truesight_adapters::vision::RgbFrame>, truesight_core::ports::PortError>
    {
        let _ = self.entered.send(());
        Ok(self
            .frames
            .recv_timeout(std::time::Duration::from_secs(5))
            .ok())
    }
}

fn vision_faction_frame(hero: u16) -> truesight_adapters::vision::RgbFrame {
    vision_remap_frame(Some(hero), None)
}

fn vision_remap_frame(
    first: Option<u16>,
    second: Option<u16>,
) -> truesight_adapters::vision::RgbFrame {
    use truesight_adapters::vision::{RgbFrame, compute_topbar_slot_rects, parse_hero_atlas};
    use truesight_core::domain::HeroId;
    let atlas = parse_hero_atlas().unwrap();
    let (w, h) = (1920, 1080);
    let mut rgb = vec![0; w * h * 3];
    let (rad, dire) = compute_topbar_slot_rects(w, h);
    for (rect, id) in [(rad[0], first), (rad[1], second), (dire[0], Some(2))] {
        let Some(id) = id else { continue };
        let portrait = &atlas.heroes[&HeroId(id)];
        for y in 0..rect.height {
            for x in 0..rect.width {
                let src = ((y * atlas.hero_height / rect.height) * atlas.hero_width
                    + x * atlas.hero_width / rect.width)
                    * 3;
                let dst = ((rect.y + y) * w + rect.x + x) * 3;
                rgb[dst..dst + 3].copy_from_slice(&portrait[src..src + 3]);
            }
        }
    }
    RgbFrame::new(w, h, rgb)
}

fn vision_unreadable_remap_scenario(
    prepare: impl FnOnce(&mut AppSession),
    check: impl FnOnce(
        &mut AppSession,
        &mut truesight_adapters::vision::BoundedVisionWorker,
        &dyn Fn(Option<u16>, Option<u16>),
    ),
) {
    use std::{sync::mpsc, time::Duration};
    use truesight_adapters::vision::BoundedVisionWorker;
    use truesight_core::domain::Faction;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (frame_tx, frame_rx) = mpsc::channel();
    let mut worker = BoundedVisionWorker::spawn_with_capacity(
        VisionGatedSource {
            entered: entered_tx,
            frames: frame_rx,
        },
        2,
    )
    .unwrap();
    let mut session = AppSession::new().unwrap();
    let next_capture = || entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let frame = |first, second| {
        frame_tx.send(vision_remap_frame(first, second)).unwrap();
        next_capture();
    };
    next_capture();
    frame(Some(48), None);
    session.poll_live_sources(None, Some(&mut worker));
    frame(Some(48), Some(60));
    session.poll_live_sources(None, Some(&mut worker));
    assert_eq!(worker.queue_stats().enqueued, 3);
    prepare(&mut session);
    session.set_ally_faction(Faction::Dire);
    // Propagate the flip before releasing the old in-flight capture.
    session.poll_live_sources(None, Some(&mut worker));
    frame(None, Some(60));
    check(&mut session, &mut worker, &frame);
    drop(frame_tx);
}

#[test]
fn vision_faction_remap_does_not_resurrect_unreadable_manual_replacement() {
    use truesight_core::domain::{HeroId, Side};
    let replace_luna = |session: &mut AppSession| {
        let slot = session
            .draft
            .allies
            .iter()
            .position(|p| p.is_some_and(|p| p.hero_id == HeroId(48)))
            .unwrap();
        session.set_slot_manual(Side::Allies, slot, HeroId(88));
    };
    vision_unreadable_remap_scenario(replace_luna, |session, worker, frame| {
        let before = session.draft.enemies;
        assert!(before.iter().flatten().any(|p| p.hero_id == HeroId(88)));
        frame(None, Some(60));
        session.poll_live_sources(None, Some(worker));
        assert!(
            !session
                .draft
                .enemies
                .iter()
                .flatten()
                .any(|p| p.hero_id == HeroId(48)),
            "Unreadable portrait resurrected manually replaced Luna"
        );
        assert_eq!(session.draft.enemies, before);
    });
}

#[test]
fn vision_faction_remap_skips_unreadable_slot_without_overflow_or_latch_loss() {
    use truesight_core::{domain::Side, ports::DraftEventSource};
    vision_unreadable_remap_scenario(
        |_| {},
        |_, worker, frame| {
            frame(None, Some(60));
            assert_eq!(
                worker.queue_stats().dropped,
                0,
                "Unreadable portrait overflowed the queue"
            );
            let second = truesight_core::ports::DraftEvent::PickDetected {
                side: Side::Enemies,
                slot: 1,
                hero_id: truesight_core::domain::HeroId(60),
                role: Some(truesight_core::domain::Role::ALL[1]),
            };
            assert_eq!(
                worker.poll_events().unwrap(),
                vec![visual_pick_with_role(Side::Allies, 2), second]
            );
            frame(Some(88), Some(60));
            assert!(
                worker.poll_events().unwrap().is_empty(),
                "One contradictory frame bypassed confirmation"
            );
            frame(Some(88), Some(60));
            assert_eq!(
                worker.poll_events().unwrap(),
                vec![visual_pick_with_role(Side::Enemies, 88)]
            );
        },
    );
}

#[test]
fn vision_worker_faction_flip_preserves_confirmed_portraits() {
    use std::sync::mpsc;
    use std::time::Duration;
    use truesight_adapters::vision::BoundedVisionWorker;
    use truesight_core::domain::{Faction, Side};
    use truesight_core::ports::DraftEventSource;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (frame_tx, frame_rx) = mpsc::channel();
    let mut worker = BoundedVisionWorker::spawn_with_source(VisionGatedSource {
        entered: entered_tx,
        frames: frame_rx,
    })
    .unwrap();
    let next_capture = || entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    next_capture();
    frame_tx.send(vision_faction_frame(48)).unwrap();
    next_capture();
    assert_eq!(worker.poll_events().unwrap().len(), 2);

    // A contradictory old-epoch capture must not count toward confirmation.
    worker.set_ally_faction(Faction::Dire);
    frame_tx.send(vision_faction_frame(88)).unwrap();
    next_capture();
    assert!(worker.poll_events().unwrap().is_empty());
    frame_tx.send(vision_faction_frame(88)).unwrap();
    next_capture();
    let events = worker.poll_events().unwrap();
    assert_eq!(
        events,
        vec![visual_pick_with_role(Side::Allies, 2)],
        "Contradictory evidence must neither replay Luna nor replace her with Nyx"
    );

    frame_tx.send(vision_faction_frame(88)).unwrap();
    next_capture();
    assert_eq!(
        worker.poll_events().unwrap(),
        vec![visual_pick_with_role(Side::Enemies, 88)]
    );
    drop(frame_tx);
}

fn visual_pick_with_role(
    side: truesight_core::domain::Side,
    id: u16,
) -> truesight_core::ports::DraftEvent {
    truesight_core::ports::DraftEvent::PickDetected {
        side,
        slot: 0,
        hero_id: truesight_core::domain::HeroId(id),
        role: Some(truesight_core::domain::Role::ALL[0]),
    }
}

#[test]
fn vision_worker_faction_flip_invalidates_queued_relative_picks() {
    use std::sync::mpsc;
    use std::time::Duration;
    use truesight_adapters::vision::BoundedVisionWorker;
    use truesight_core::domain::{Faction, HeroId};
    let (entered_tx, entered_rx) = mpsc::channel();
    let (frame_tx, frame_rx) = mpsc::channel();
    let mut worker = BoundedVisionWorker::spawn_with_source(VisionGatedSource {
        entered: entered_tx,
        frames: frame_rx,
    })
    .unwrap();
    let mut session = AppSession::new().unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    frame_tx.send(vision_faction_frame(48)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    session.poll_live_sources(None, Some(&mut worker));
    assert!(
        session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48))
    );
    // Two confirmed Nyx frames, still mapped against Radiant, remain queued.
    frame_tx.send(vision_faction_frame(88)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    frame_tx.send(vision_faction_frame(88)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(worker.queue_stats().enqueued, 3);
    session.set_ally_faction(Faction::Dire);
    session.poll_live_sources(None, Some(&mut worker));
    assert!(
        session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(2)),
        "Queued pre-flip Nyx retired unrelated Axe"
    );
    assert!(
        session
            .draft
            .enemies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48))
    );
    // Finish the old in-flight capture; the next epoch re-emits stationary portraits.
    frame_tx.send(vision_faction_frame(88)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    frame_tx.send(vision_faction_frame(88)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    session.poll_live_sources(None, Some(&mut worker));
    assert!(
        session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(2))
    );
    assert!(
        session
            .draft
            .enemies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(88))
    );
    assert!(
        !session
            .draft
            .enemies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48))
    );
    drop(frame_tx);
}

#[test]
fn vision_worker_faction_flip_rejects_in_flight_old_epoch() {
    use std::sync::mpsc;
    use std::time::Duration;
    use truesight_adapters::vision::BoundedVisionWorker;
    use truesight_core::domain::{Faction, HeroId, Side};
    use truesight_core::ports::DraftEventSource;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (frame_tx, frame_rx) = mpsc::channel();
    let mut worker = BoundedVisionWorker::spawn_with_source(VisionGatedSource {
        entered: entered_tx,
        frames: frame_rx,
    })
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    worker.set_ally_faction(Faction::Dire);
    // Use a different hero in the invalidated first capture: it must never
    // become an accepted identity that the first current-epoch frame must replace.
    frame_tx.send(vision_faction_frame(88)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        worker.poll_events().unwrap().is_empty(),
        "Old in-flight faction results accepted after flip"
    );
    frame_tx.send(vision_faction_frame(48)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let events = worker.poll_events().unwrap();
    assert!(events.iter().any(|ev| matches!(
        ev,
        truesight_core::ports::DraftEvent::PickDetected {
            side: Side::Enemies,
            hero_id: HeroId(48),
            ..
        }
    )));
    assert!(events.iter().any(|ev| matches!(
        ev,
        truesight_core::ports::DraftEvent::PickDetected {
            side: Side::Allies,
            hero_id: HeroId(2),
            ..
        }
    )));
    assert_eq!(events.len(), 2);
    drop(frame_tx);
}

fn visual_pick(
    side: truesight_core::domain::Side,
    slot: usize,
    id: u16,
) -> truesight_core::ports::DraftEvent {
    truesight_core::ports::DraftEvent::PickDetected {
        side,
        slot,
        hero_id: truesight_core::domain::HeroId(id),
        role: None,
    }
}

#[test]
fn vision_reconciles_full_drafts_movements_faction_and_reset() {
    use truesight_core::domain::{Faction, HeroId, Side};
    let mut session = AppSession::new().unwrap();
    let heroes = [48, 2, 5, 13, 27];
    session.apply_vision_events(
        heroes
            .into_iter()
            .enumerate()
            .map(|(s, id)| visual_pick(Side::Allies, s, id))
            .collect(),
    );
    // Full-team simultaneous reordering must not temporarily delete moved heroes.
    session.apply_vision_events(
        [2, 48, 5, 13, 27]
            .into_iter()
            .enumerate()
            .map(|(s, id)| visual_pick(Side::Allies, s, id))
            .collect(),
    );
    assert_eq!(session.draft.allies.iter().flatten().count(), 5);
    for id in heroes {
        assert!(
            session
                .draft
                .allies
                .iter()
                .flatten()
                .any(|p| p.hero_id == HeroId(id))
        );
    }
    session.set_ally_faction(Faction::Dire);
    session.apply_vision_events(vec![visual_pick(Side::Enemies, 1, 88)]);
    assert_eq!(session.draft.enemies.iter().flatten().count(), 5);
    assert!(
        !session
            .draft
            .enemies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48))
    );
    assert!(
        session
            .draft
            .enemies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(88))
    );
    let before = session.draft;
    session.apply_vision_events(vec![visual_pick(Side::Enemies, 5, 1)]);
    session.apply_vision_events(vec![]);
    assert_eq!(session.draft.enemies, before.enemies);
    session.reset_draft();
    session.apply_vision_events(vec![visual_pick(Side::Enemies, 1, 48)]);
    assert_eq!(session.draft.enemies.iter().flatten().count(), 1);
    assert_eq!(
        session
            .draft
            .enemies
            .iter()
            .flatten()
            .next()
            .unwrap()
            .hero_id,
        HeroId(48)
    );
}

#[test]
fn vision_replacement_protects_manual_edits_dismissals_and_gsi_authority() {
    use truesight_core::domain::{HeroId, Side};
    use truesight_core::ports::DraftEvent;
    let mut session = AppSession::new().unwrap();
    session.apply_vision_events(vec![visual_pick(Side::Allies, 0, 48)]);
    session.set_slot_manual(Side::Allies, 4, HeroId(48));
    session.apply_vision_events(vec![visual_pick(Side::Allies, 0, 88)]);
    assert_eq!(session.draft.allies[4].unwrap().hero_id, HeroId(48));
    session.clear_slot_manual(Side::Allies, 4);
    session.apply_vision_events(vec![visual_pick(Side::Allies, 0, 48)]);
    assert!(
        !session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48))
    );
    session.reset_draft();
    session.apply_vision_events(vec![visual_pick(Side::Allies, 0, 48)]);
    session.apply_event(DraftEvent::LocalPlayerHeroDetected {
        hero_slug: "npc_dota_hero_luna".into(),
    });
    session.apply_vision_events(vec![visual_pick(Side::Allies, 0, 88)]);
    assert_eq!(session.local_player_hero, Some(HeroId(48)));
    assert!(
        session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(48))
    );
    session.reset_draft();
    // A shared GSI PickDetected is not a visual slot overwrite and relinquishes
    // visual ownership even if it confirms an already present automatic hero.
    session.apply_vision_events(vec![visual_pick(Side::Allies, 0, 48)]);
    session.apply_event(visual_pick(Side::Allies, 0, 48));
    session.apply_event(visual_pick(Side::Allies, 0, 2));
    session.apply_vision_events(vec![visual_pick(Side::Allies, 0, 88)]);
    for id in [48, 2, 88] {
        assert!(
            session
                .draft
                .allies
                .iter()
                .flatten()
                .any(|p| p.hero_id == HeroId(id))
        );
    }
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
    wait_for_sync_idle(&mut session);

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

    wait_for_sync_idle(&mut session);
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
        capture_strip_rect: None,
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
        capture_strip_rect: None,
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

    wait_for_sync_idle(&mut session);
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
    wait_for_sync_idle(&mut session);
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
    wait_for_sync_idle(&mut session);
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
    wait_for_sync_idle(&mut s_sync);
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
    wait_for_sync_idle(&mut restarted);
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

#[test]
fn session_rejects_pre_match_menu_frames_and_transitions_cleanly_to_live_match() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use truesight_adapters::vision::{
        BoundedVisionWorker, RgbFrame, ScreenFrameSource, TopBarSlotRect,
        compute_ingame_topbar_slot_rects, compute_pickphase_topbar_slot_rects, parse_hero_atlas,
    };
    use truesight_core::domain::HeroId;
    use truesight_core::ports::DraftEvent;

    struct SharedFrameSource {
        frame: Arc<Mutex<RgbFrame>>,
        captures: Arc<AtomicUsize>,
    }
    impl ScreenFrameSource for SharedFrameSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, truesight_core::ports::PortError> {
            self.captures.fetch_add(1, Ordering::Release);
            Ok(Some(self.frame.lock().unwrap().clone()))
        }
    }

    fn paint_slot(
        frame: &mut [u8],
        frame_w: usize,
        rect: TopBarSlotRect,
        hero_rgb: &[u8],
        src_w: usize,
        src_h: usize,
        full_crop: bool,
    ) {
        let (sx0, sw0, sy0, sh0) = if full_crop {
            (0_usize, src_w, 0_usize, src_h)
        } else {
            (4_usize, src_w - 8, 2_usize, src_h - 4)
        };
        for dy in 0..rect.height {
            let sy = sy0 + (dy * sh0) / rect.height;
            for dx in 0..rect.width {
                let sx = sx0 + (dx * sw0) / rect.width;
                let si = (sy * src_w + sx) * 3;
                let di = ((rect.y + dy) * frame_w + (rect.x + dx)) * 3;
                frame[di..di + 3].copy_from_slice(&hero_rgb[si..si + 3]);
            }
        }
    }

    // Synthetic 2560x115 main-menu navigation chrome matching the measured menu tab geometry.
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

    let atlas = parse_hero_atlas().unwrap();
    let shared_frame = Arc::new(Mutex::new(RgbFrame::new(mw, mh, menu_rgb)));
    let captures = Arc::new(AtomicUsize::new(0));
    let mut worker = BoundedVisionWorker::spawn_with_source(SharedFrameSource {
        frame: Arc::clone(&shared_frame),
        captures: Arc::clone(&captures),
    })
    .unwrap();
    let mut session = AppSession::new().unwrap();

    // 1. Pre-match main menu with absent GSI (last_rules_state == None) must emit 0 phantom picks
    // across >= 2 captures (covering the 2-frame initial confirmation window).
    for _ in 0..120 {
        session.poll_live_sources(None, Some(&mut worker));
        if captures.load(Ordering::Acquire) >= 2 {
            // Drain any queued events from the second capture before asserting.
            std::thread::sleep(Duration::from_millis(50));
            session.poll_live_sources(None, Some(&mut worker));
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        captures.load(Ordering::Acquire) >= 2,
        "Expected >= 2 pre-match menu captures during Step 1"
    );
    assert_eq!(
        session.draft.allies.iter().flatten().count(),
        0,
        "Pre-match menu without GSI must leave allies empty, got: {:?}",
        session.draft.allies
    );
    assert_eq!(
        session.draft.enemies.iter().flatten().count(),
        0,
        "Pre-match menu without GSI must leave enemies empty, got: {:?}",
        session.draft.enemies
    );

    // 2. Match starts: WAIT_FOR_PLAYERS_TO_LOAD -> HERO_SELECTION with real PickPhase portraits.
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD".into(),
        game_time_secs: -90,
    });
    let (vw, vh) = (2560_usize, 1440_usize);
    let (rad_pick, dire_pick) = compute_pickphase_topbar_slot_rects(vw, vh);
    let mut pick_rgb = vec![15_u8; vw * 160 * 3];
    paint_slot(
        &mut pick_rgb,
        vw,
        rad_pick[0],
        &atlas.heroes[&HeroId(88)],
        atlas.hero_width,
        atlas.hero_height,
        false,
    );
    paint_slot(
        &mut pick_rgb,
        vw,
        dire_pick[0],
        &atlas.heroes[&HeroId(2)],
        atlas.hero_width,
        atlas.hero_height,
        false,
    );
    *shared_frame.lock().unwrap() = RgbFrame::with_source_height(vw, 160, vh, pick_rgb);
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
        game_time_secs: 45,
    });
    for _ in 0..40 {
        session.poll_live_sources(None, Some(&mut worker));
        let allies: Vec<u16> = session
            .draft
            .allies
            .iter()
            .flatten()
            .map(|p| p.hero_id.0)
            .collect();
        let enemies: Vec<u16> = session
            .draft
            .enemies
            .iter()
            .flatten()
            .map(|p| p.hero_id.0)
            .collect();
        if allies == [88] && enemies == [2] {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let allies_pick: Vec<u16> = session
        .draft
        .allies
        .iter()
        .flatten()
        .map(|p| p.hero_id.0)
        .collect();
    let enemies_pick: Vec<u16> = session
        .draft
        .enemies
        .iter()
        .flatten()
        .map(|p| p.hero_id.0)
        .collect();
    assert_eq!(allies_pick, vec![88]);
    assert_eq!(enemies_pick, vec![2]);

    // 3. Transition to GAME_IN_PROGRESS with InGame portraits (adding Anti-Mage=1 on allies).
    let (rad_ig, dire_ig) = compute_ingame_topbar_slot_rects(vw, vh);
    let mut ig_rgb = vec![15_u8; vw * 160 * 3];
    paint_slot(
        &mut ig_rgb,
        vw,
        rad_ig[0],
        &atlas.heroes[&HeroId(88)],
        atlas.hero_width,
        atlas.hero_height,
        true,
    );
    paint_slot(
        &mut ig_rgb,
        vw,
        rad_ig[1],
        &atlas.heroes[&HeroId(1)],
        atlas.hero_width,
        atlas.hero_height,
        true,
    );
    paint_slot(
        &mut ig_rgb,
        vw,
        dire_ig[0],
        &atlas.heroes[&HeroId(2)],
        atlas.hero_width,
        atlas.hero_height,
        true,
    );
    *shared_frame.lock().unwrap() = RgbFrame::with_source_height(vw, 160, vh, ig_rgb);
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 60,
    });
    for _ in 0..160 {
        session.poll_live_sources(None, Some(&mut worker));
        let allies_cnt = session.draft.allies.iter().flatten().count();
        let enemies_cnt = session.draft.enemies.iter().flatten().count();
        if allies_cnt == 2 && enemies_cnt == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let mut allies_ig: Vec<u16> = session
        .draft
        .allies
        .iter()
        .flatten()
        .map(|p| p.hero_id.0)
        .collect();
    allies_ig.sort_unstable();
    let enemies_ig: Vec<u16> = session
        .draft
        .enemies
        .iter()
        .flatten()
        .map(|p| p.hero_id.0)
        .collect();
    assert_eq!(allies_ig, vec![1, 88]);
    assert_eq!(enemies_ig, vec![2]);
}

fn encode_base64url_no_pad(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut i = 0;
    while i < input.len() {
        let b0 = input[i];
        let b1 = if i + 1 < input.len() { input[i + 1] } else { 0 };
        let b2 = if i + 2 < input.len() { input[i + 2] } else { 0 };
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if i + 1 < input.len() {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        }
        if i + 2 < input.len() {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        }
        i += 3;
    }
    out
}

fn make_test_jwt(payload_json: &str) -> String {
    let hdr = encode_base64url_no_pad(br#"{"alg":"RS256","typ":"JWT"}"#);
    let pay = encode_base64url_no_pad(payload_json.as_bytes());
    format!("{hdr}.{pay}.c2ln")
}

#[test]
fn stratz_jwt_claims_decoder_extracts_steam_id_and_days_remaining() {
    use crate::session::parse_stratz_jwt_claims;

    let now = truesight_adapters::sync::LocalCacheManager::now_unix();
    let exp = now + 365 * 86_400;
    let token_str_id = make_test_jwt(&format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521469","iat":{now},"exp":{exp}}}"#
    ));
    let info = parse_stratz_jwt_claims(&token_str_id, now).expect("valid STRATZ JWT");
    assert_eq!(info.steam32_id, Some(127_255_741));
    assert_eq!(info.days_remaining, 365);
    assert_eq!(info.exp, exp);
    assert_eq!(info.iat, Some(now));

    let token_num_id = make_test_jwt(&format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":76561198087521469,"exp":{exp}}}"#
    ));
    let info_num = parse_stratz_jwt_claims(&token_num_id, now).expect("numeric SteamId JWT");
    assert_eq!(info_num.steam32_id, Some(127_255_741));
    assert_eq!(info_num.days_remaining, 365);

    assert!(
        parse_stratz_jwt_claims(&token_str_id, exp).is_err(),
        "expired token at exp == now must be rejected"
    );
    assert!(
        parse_stratz_jwt_claims("not.a.jwt.with.five.parts", now).is_err(),
        "malformed segment count must be rejected"
    );
    let wrong_iss = make_test_jwt(&format!(r#"{{"iss":"https://example.com","exp":{exp}}}"#));
    assert!(
        parse_stratz_jwt_claims(&wrong_iss, now).is_err(),
        "non-stratz issuer must be rejected"
    );
}

#[test]
fn ui_config_persists_onboarding_dismissed_and_stratz_token_with_private_permissions() {
    use crate::session::{UiConfig, load_ui_config_from_dir, save_ui_config_to_dir};

    let tmp_dir =
        std::env::temp_dir().join(format!("truesight_test_ui_cfg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    assert!(!load_ui_config_from_dir(&tmp_dir).onboarding_dismissed);

    let cfg = UiConfig {
        onboarding_dismissed: true,
        stratz_api_token: Some("hdr.pay.sig".into()),
        stratz_token_exp: Some(1_731_536_000),
        ..UiConfig::default()
    };
    save_ui_config_to_dir(&tmp_dir, &cfg).expect("save ui_config.toml");

    let loaded = load_ui_config_from_dir(&tmp_dir);
    assert_eq!(loaded, cfg);

    let leftover_tmps: Vec<_> = std::fs::read_dir(&tmp_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftover_tmps.is_empty(), "expected no leftover tmp files");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(tmp_dir.join("ui_config.toml")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn save_ui_config_failure_surfaces_warning_and_keeps_in_memory_state() {
    let tmp_dir =
        std::env::temp_dir().join(format!("truesight_test_ui_cfg_fail_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();
    // Block config directory creation by placing a regular file where the dir should be.
    let blocking_file = tmp_dir.join("blocked_dir");
    std::fs::write(&blocking_file, b"not a directory").unwrap();
    let cfg_path = blocking_file.join("config.toml");

    let mgr = truesight_adapters::sync::LocalCacheManager::with_optional_account(
        tmp_dir.join("cache"),
        None,
    );
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path)).unwrap();

    let res = session.set_onboarding_dismissed(true);
    assert!(res.is_err(), "expected save failure on blocked dir");
    assert!(
        session.onboarding_dismissed,
        "in-memory state must remain updated"
    );
    assert!(
        session
            .last_sync_notice
            .as_deref()
            .is_some_and(|n| n.contains("failed to save ui_config.toml")),
        "expected warning in last_sync_notice, got: {:?}",
        session.last_sync_notice
    );

    let now = 1_700_000_000_u64;
    let exp = now + 30 * 86_400;
    let token = make_test_jwt(&format!(
        r#"{{"iss":"https://api.stratz.com","iat":{now},"exp":{exp}}}"#
    ));
    session.last_sync_notice = None;
    let info = session
        .apply_stratz_token(&token, now)
        .expect("apply_stratz_token should succeed in-memory even if save fails");
    assert_eq!(info.exp, exp);
    assert_eq!(session.stratz_api_token.as_deref(), Some(token.as_str()));
    assert!(
        session
            .last_sync_notice
            .as_deref()
            .is_some_and(|n| n.contains("failed to save ui_config.toml")),
        "expected save failure warning in last_sync_notice, got: {:?}",
        session.last_sync_notice
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn apply_stratz_token_auto_links_steam32_and_updates_session() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_test_stratz_apply_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cfg_path = tmp_dir.join("config.toml");
    let mgr = truesight_adapters::sync::LocalCacheManager::with_optional_account(
        tmp_dir.join("cache"),
        None,
    );
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path)).unwrap();

    let now = truesight_adapters::sync::LocalCacheManager::now_unix();
    let exp = now + 365 * 86_400;
    let token = make_test_jwt(&format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521469","iat":{now},"exp":{exp}}}"#
    ));
    let info = session
        .apply_stratz_token(&token, now)
        .expect("apply_stratz_token should succeed");
    assert_eq!(info.steam32_id, Some(127_255_741));
    assert_eq!(info.days_remaining, 365);
    assert_eq!(session.cache_mgr.player_account_id, Some(127_255_741));
    assert_eq!(session.stratz_token_days_remaining_at(now), Some(365));
    assert!(!session.is_stratz_token_expired_at(now));
    assert_eq!(
        session.cache_mgr.stratz_api_token.as_deref(),
        Some(token.as_str())
    );
    assert_eq!(
        session.cache_mgr.effective_stratz_token().as_deref(),
        Some(token.as_str())
    );

    let reloaded_mgr = truesight_adapters::sync::LocalCacheManager::with_optional_account(
        tmp_dir.join("cache"),
        None,
    );
    let mut deferred = AppSession::with_cache_and_config(reloaded_mgr, None).unwrap();
    assert_eq!(deferred.cache_mgr.stratz_api_token, None);
    deferred.enable_config_persistence_for_path(tmp_dir.join("config.toml"));
    assert_eq!(
        deferred.cache_mgr.stratz_api_token.as_deref(),
        Some(token.as_str())
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn apply_stratz_token_preserves_different_explicit_steam_link_until_confirmed() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_test_stratz_explicit_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cfg_path = tmp_dir.join("config.toml");
    let mgr = truesight_adapters::sync::LocalCacheManager::with_optional_account(
        tmp_dir.join("cache"),
        Some(111_222_333),
    );
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path.clone())).unwrap();
    session.set_account_id(Some(111_222_333), true);

    let now = 1_700_000_000_u64;
    let exp = now + 30 * 86_400;
    let token = make_test_jwt(&format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521469","iat":{now},"exp":{exp}}}"#
    ));
    session.apply_stratz_token(&token, now).unwrap();

    // STRATZ token is connected, but explicit Steam account 111_222_333 is preserved.
    assert_eq!(
        session.cache_mgr.stratz_api_token.as_deref(),
        Some(token.as_str())
    );
    assert_eq!(session.cache_mgr.player_account_id, Some(111_222_333));
    assert_eq!(session.pending_stratz_steam32_link, Some(127_255_741));

    // Dismissing keeps the existing explicit account.
    session.dismiss_pending_stratz_steam_link();
    assert_eq!(session.pending_stratz_steam32_link, None);
    assert_eq!(session.cache_mgr.player_account_id, Some(111_222_333));

    // Re-applying and confirming replaces the explicit account and persists it.
    session.apply_stratz_token(&token, now).unwrap();
    assert_eq!(session.pending_stratz_steam32_link, Some(127_255_741));
    assert_eq!(
        session.confirm_pending_stratz_steam_link(),
        Some(127_255_741)
    );
    assert_eq!(session.pending_stratz_steam32_link, None);
    assert_eq!(session.cache_mgr.player_account_id, Some(127_255_741));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn stratz_token_expiry_derives_from_absolute_exp_and_rejects_expired_on_reload() {
    use crate::session::{UiConfig, save_ui_config_to_dir};

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_test_stratz_expiry_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cfg_path = tmp_dir.join("config.toml");

    let now = 1_700_000_000_u64;
    let exp = now + 10 * 86_400;
    let token = make_test_jwt(&format!(
        r#"{{"iss":"https://api.stratz.com","iat":{now},"exp":{exp}}}"#
    ));

    let mgr = truesight_adapters::sync::LocalCacheManager::with_optional_account(
        tmp_dir.join("cache"),
        None,
    );
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path.clone())).unwrap();
    session.apply_stratz_token(&token, now).unwrap();

    assert_eq!(session.stratz_token_days_remaining_at(now), Some(10));
    assert_eq!(
        session.stratz_token_days_remaining_at(now + 4 * 86_400),
        Some(6)
    );
    assert!(!session.is_stratz_token_expired_at(now + 4 * 86_400));
    assert_eq!(session.stratz_token_days_remaining_at(exp), None);
    assert!(session.is_stratz_token_expired_at(exp));

    // Persist an already-expired token and verify reload does not wire it into cache_mgr.
    let expired_token = make_test_jwt(r#"{"iss":"https://api.stratz.com","iat":1000,"exp":2000}"#);
    save_ui_config_to_dir(
        &tmp_dir,
        &UiConfig {
            onboarding_dismissed: false,
            stratz_api_token: Some(expired_token),
            stratz_token_exp: Some(2000),
            ..UiConfig::default()
        },
    )
    .unwrap();

    let reloaded_mgr = truesight_adapters::sync::LocalCacheManager::with_optional_account(
        tmp_dir.join("cache"),
        None,
    );
    let reloaded = AppSession::with_cache_and_config(reloaded_mgr, Some(cfg_path)).unwrap();
    assert_eq!(reloaded.cache_mgr.stratz_api_token, None);
    assert!(
        reloaded
            .is_stratz_token_expired_at(truesight_adapters::sync::LocalCacheManager::now_unix())
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn effective_stratz_token_enforces_expiry_during_open_session_with_env_override_precedence() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_test_stratz_sync_exp_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cfg_path = tmp_dir.join("config.toml");

    let t0 = 1_700_000_000_u64;
    let exp = t0 + 3_600;
    let token = make_test_jwt(&format!(
        r#"{{"iss":"https://api.stratz.com","iat":{t0},"exp":{exp}}}"#
    ));

    let mgr = truesight_adapters::sync::LocalCacheManager::with_optional_account(
        tmp_dir.join("cache"),
        None,
    );
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path)).unwrap();
    session.apply_stratz_token(&token, t0).unwrap();

    assert_eq!(
        session.cache_mgr.effective_stratz_token_at(t0).as_deref(),
        Some(token.as_str()),
        "valid token must be returned before exp"
    );
    assert_eq!(
        session.cache_mgr.effective_stratz_token_at(exp + 1),
        None,
        "expired persisted token must NOT be returned after exp during the same open session"
    );
    assert_eq!(
        session
            .cache_mgr
            .effective_stratz_token_with_env_at(Some("env_override_token"), exp + 1)
            .as_deref(),
        Some("env_override_token"),
        "explicit STRATZ_API_TOKEN env override must still take precedence over expired persisted token"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn session_transitions_to_post_draft_build_ticks_owned_items_and_swaps_variants() {
    use truesight_core::ports::{DraftEvent, ItemMetaRepository};
    let mut session = AppSession::new().unwrap();
    assert!(
        !session.is_post_draft_build_view(),
        "initial session must default to draft/picks view"
    );

    // Hero selection keeps picks view.
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
        game_time_secs: 0,
    });
    assert!(!session.is_post_draft_build_view());

    // Pick Anti-Mage (Carry) as local player hero and an enemy with Evasion (Phantom Assassin).
    assert!(execute_command(&mut session, "ally 1 anti-mage").unwrap());
    assert!(execute_command(&mut session, "enemy 1 phantom assassin").unwrap());

    // Transition to PRE_GAME -> post-draft item build view.
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_PRE_GAME".into(),
        game_time_secs: 15,
    });
    assert!(session.is_post_draft_build_view());
    assert_eq!(session.game_time_secs, 15);

    // Transition to GAME_IN_PROGRESS -> stays in post-draft item build view.
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 900,
    });
    assert!(session.is_post_draft_build_view());
    assert_eq!(session.game_time_secs, 900);

    let build_before = session
        .compute_recommended_build()
        .expect("picked allied hero must produce a recommended build");
    assert!(!build_before.steps.is_empty());
    let first_step = &build_before.steps[0];
    let first_slug = session
        .item_repo
        .item_catalog(first_step.item_id)
        .expect("step item must exist in catalog")
        .slug;

    // InventoryUpdated populates owned_items and advances next_unowned_index.
    session.apply_event(DraftEvent::InventoryUpdated {
        hero_slug: None,
        item_slugs: vec![format!("item_{first_slug}")],
    });
    assert!(session.owned_items.contains(&first_step.item_id));

    let build_after_own = session.compute_recommended_build().unwrap();
    assert!(build_after_own.steps[0].owned);
    assert_ne!(build_after_own.next_unowned_index, Some(0));

    // Variant swap replaces the primary item at that step and keeps the old primary in variants.
    if let Some((step_idx, var_id)) = build_after_own
        .steps
        .iter()
        .enumerate()
        .find_map(|(idx, s)| s.variants.first().map(|v| (idx, v.item_id)))
    {
        let old_primary_id = build_after_own.steps[step_idx].item_id;
        session.select_build_variant(step_idx, var_id);
        let swapped = session.compute_recommended_build().unwrap();
        assert_eq!(swapped.steps[step_idx].item_id, var_id);
        assert!(
            swapped.steps[step_idx]
                .variants
                .iter()
                .any(|v| v.item_id == old_primary_id),
            "old primary item must remain in variants so user can switch back"
        );
    }

    // Stale patch detection.
    session.active_game_patch = truesight_core::domain::counter_trait_table_patch().to_string();
    assert!(!session.is_counter_table_patch_stale());
    session.active_game_patch = "7.41".to_string();
    assert!(session.is_counter_table_patch_stale());
}

#[test]
fn overlay_geometry_clamps_to_monitor_and_avoids_topbar_capture_strip() {
    let clamped = AppSession::clamp_overlay_geometry(
        crate::session::OverlayGeometry {
            x: -50.0,
            y: 10.0,
            width: 100.0,
            height: 999.0,
            click_through: true,
        },
        1920.0,
        1080.0,
    );
    assert_eq!(clamped.width, 280.0);
    assert_eq!(clamped.height, 720.0);
    assert_eq!(clamped.x, 0.0);
    assert!(
        clamped.y >= 96.0,
        "overlay must stay below top 8% Dota 2 portrait capture strip (y >= 96.0)"
    );
    assert!(clamped.click_through);

    let clamped_far = AppSession::clamp_overlay_geometry(
        crate::session::OverlayGeometry {
            x: 5000.0,
            y: 5000.0,
            width: 400.0,
            height: 240.0,
            click_through: false,
        },
        1920.0,
        1080.0,
    );
    assert_eq!(clamped_far.x, 1520.0);
    assert_eq!(clamped_far.y, 840.0);

    // Persistence in ui_config.toml
    let tmp_dir =
        std::env::temp_dir().join(format!("truesight_test_overlay_cfg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cfg_path = tmp_dir.join("config.toml");

    let mgr = truesight_adapters::sync::LocalCacheManager::with_optional_account(
        tmp_dir.join("cache"),
        None,
    );
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path.clone())).unwrap();
    session.set_overlay_geometry(clamped_far);
    session.set_overlay_mode(true);

    let reloaded_mgr = truesight_adapters::sync::LocalCacheManager::with_optional_account(
        tmp_dir.join("cache"),
        None,
    );
    let reloaded = AppSession::with_cache_and_config(reloaded_mgr, Some(cfg_path)).unwrap();
    assert!(reloaded.overlay_mode);
    assert_eq!(reloaded.overlay_geometry, clamped_far);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn overlay_geometry_clamps_against_dynamic_monitor_and_capture_strip() {
    assert_eq!(AppSession::top_bar_capture_bottom_y(1080.0, None), 96.0);
    assert_eq!(AppSession::top_bar_capture_bottom_y(1440.0, None), 116.0);
    assert_eq!(AppSession::top_bar_capture_bottom_y(2160.0, None), 173.0);
    assert_eq!(
        AppSession::top_bar_capture_bottom_y(1440.0, Some((100.0, 200.0, 1280.0, 720.0))),
        258.0
    );

    let clamped_1440 = AppSession::clamp_overlay_geometry(
        crate::session::OverlayGeometry {
            x: 200.0,
            y: 50.0,
            width: 380.0,
            height: 230.0,
            click_through: false,
        },
        2560.0,
        1440.0,
    );
    assert!(
        clamped_1440.y >= 116.0,
        "1440p monitor must clamp overlay below 8% top-bar strip (y >= 116.0), got {}",
        clamped_1440.y
    );

    // Overlapping a custom Dota 2 capture strip pushes overlay below cy + ch.
    let clamped_capture = AppSession::clamp_overlay_geometry_against_capture(
        crate::session::OverlayGeometry {
            x: 150.0,
            y: 210.0,
            width: 380.0,
            height: 230.0,
            click_through: false,
        },
        2560.0,
        1440.0,
        Some((100.0, 200.0, 1280.0, 58.0)),
    );
    assert!(
        clamped_capture.y >= 258.0,
        "overlapping capture strip must push overlay below cy + ch (258.0), got {}",
        clamped_capture.y
    );

    // Non-overlapping horizontal span only clamps against monitor top-bar floor (116.0).
    let clamped_outside = AppSession::clamp_overlay_geometry_against_capture(
        crate::session::OverlayGeometry {
            x: 1500.0,
            y: 120.0,
            width: 380.0,
            height: 230.0,
            click_through: false,
        },
        2560.0,
        1440.0,
        Some((100.0, 200.0, 1280.0, 58.0)),
    );
    assert_eq!(clamped_outside.y, 120.0);
}

#[test]
fn variant_selection_persists_across_step_reorder_by_original_item_id() {
    use truesight_core::ports::{DraftEvent, ItemMetaRepository};

    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 anti-mage").unwrap());
    assert!(execute_command(&mut session, "enemy 1 phantom assassin").unwrap());
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 900,
    });

    let build_before = session
        .compute_recommended_build()
        .expect("picked ally must produce a build");

    // Pick step 1 (which has a variant), then own step 0, step 0's variant, and step 1's original
    // primary item so all three are kept in `steps`, shifting step 1 from index 1 to index 2.
    let (orig_idx, orig_primary_id, var_id) = build_before
        .steps
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(idx, s)| s.variants.first().map(|v| (idx, s.item_id, v.item_id)))
        .expect("expected a later step to have a variant");

    let earlier_variant_id = build_before.steps[0]
        .variants
        .first()
        .map(|v| v.item_id)
        .expect("expected first step to have a variant");
    let mut owned_slugs = Vec::new();
    for item_id in [
        build_before.steps[0].item_id,
        earlier_variant_id,
        orig_primary_id,
    ] {
        let slug = session
            .item_repo
            .item_catalog(item_id)
            .expect("earlier item must exist in catalog")
            .slug;
        owned_slugs.push(format!("item_{slug}"));
    }

    // Select the variant on `orig_primary_id`.
    session.select_build_variant_for_item(orig_primary_id, var_id);
    let swapped = session.compute_recommended_build().unwrap();
    assert_eq!(swapped.steps[orig_idx].item_id, var_id);

    // Owning the first two steps plus the first step's variant shifts `orig_idx` to a new index.
    session.apply_event(DraftEvent::InventoryUpdated {
        hero_slug: None,
        item_slugs: owned_slugs,
    });

    let reordered = session.compute_recommended_build().unwrap();
    let new_idx = reordered
        .steps
        .iter()
        .position(|s| s.item_id == var_id)
        .expect("selected variant must remain present after step reorder");
    assert_ne!(
        new_idx, orig_idx,
        "owning earlier items must reorder step away from index {orig_idx}"
    );
    assert!(
        reordered.steps[new_idx]
            .variants
            .iter()
            .any(|v| v.item_id == orig_primary_id),
        "original primary item must remain in variants at the new index {new_idx}"
    );

    // Clicking the original primary item back removes the override and restores `orig_primary_id`.
    session.select_build_variant_for_item(var_id, orig_primary_id);
    let restored = session.compute_recommended_build().unwrap();
    assert_eq!(restored.steps[new_idx].item_id, orig_primary_id);
}

#[test]
fn clamp_overlay_geometry_in_monitor_preserves_secondary_monitor_coordinates() {
    let right_monitor = crate::session::MonitorRect::new(1920.0, 0.0, 2560.0, 1440.0);
    let clamped_right = AppSession::clamp_overlay_geometry_in_monitor(
        crate::session::OverlayGeometry {
            x: 2200.0,
            y: 30.0,
            width: 380.0,
            height: 230.0,
            click_through: false,
        },
        right_monitor,
        None,
    );
    assert_eq!(clamped_right.x, 2200.0);
    assert!(
        clamped_right.y >= 116.0,
        "1440p secondary monitor must clamp below 8% top-bar strip (y >= 116.0), got {}",
        clamped_right.y
    );

    let clamped_right_overflow = AppSession::clamp_overlay_geometry_in_monitor(
        crate::session::OverlayGeometry {
            x: 5000.0,
            y: 1600.0,
            width: 380.0,
            height: 230.0,
            click_through: false,
        },
        right_monitor,
        None,
    );
    assert_eq!(clamped_right_overflow.x, 1920.0 + 2560.0 - 380.0);
    assert_eq!(clamped_right_overflow.y, 1440.0 - 230.0);

    let left_monitor = crate::session::MonitorRect::new(-1920.0, 0.0, 1920.0, 1080.0);
    let clamped_left = AppSession::clamp_overlay_geometry_in_monitor(
        crate::session::OverlayGeometry {
            x: -1600.0,
            y: 40.0,
            width: 380.0,
            height: 230.0,
            click_through: true,
        },
        left_monitor,
        None,
    );
    assert_eq!(clamped_left.x, -1600.0);
    assert_eq!(clamped_left.y, 96.0);
    assert!(clamped_left.click_through);
}

#[test]
fn clamp_overlay_geometry_in_monitor_avoids_offset_windowed_dota_capture_strip() {
    let primary_monitor = crate::session::MonitorRect::new(0.0, 0.0, 1920.0, 1080.0);
    // Dota 2 running in an offset window whose top-bar capture strip spans y = 180..252.
    let offset_strip = Some((200.0, 180.0, 1600.0, 72.0));
    let clamped = AppSession::clamp_overlay_geometry_in_monitor(
        crate::session::OverlayGeometry {
            x: 240.0,
            y: 160.0,
            width: 380.0,
            height: 230.0,
            click_through: false,
        },
        primary_monitor,
        offset_strip,
    );
    assert_eq!(clamped.x, 240.0);
    assert_eq!(
        clamped.y, 252.0,
        "overlay overlapping offset capture strip (y=180..252) must be pushed below it to y=252.0"
    );
}

#[test]
fn picking_allied_hero_updates_cache_manager_target_item_heroes() {
    use truesight_core::domain::{HeroId, Side};
    let mut session = AppSession::new().unwrap();
    session.set_slot_manual(Side::Allies, 0, HeroId(74));
    assert!(
        session
            .cache_mgr
            .effective_target_item_heroes()
            .contains(&HeroId(74)),
        "picking allied hero 74 must update cache_mgr.effective_target_item_heroes()"
    );
}

#[test]
fn resolve_monitor_and_optional_clamp_respect_heterogeneous_monitors_and_unavailable_fallback() {
    let mut session = AppSession::new().unwrap();
    session.set_known_monitors(vec![
        crate::session::MonitorRect::new(0.0, 0.0, 1920.0, 1080.0),
        crate::session::MonitorRect::new(1920.0, 0.0, 1280.0, 720.0),
    ]);

    let on_secondary = crate::session::OverlayGeometry {
        x: 2450.0,
        y: 140.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let resolved = session.resolve_monitor_for_overlay(on_secondary);
    assert_eq!(
        resolved,
        Some(crate::session::MonitorRect::new(1920.0, 0.0, 1280.0, 720.0))
    );
    let clamped = AppSession::clamp_overlay_geometry_with_optional_monitor(
        on_secondary,
        resolved,
        Some((1280.0, 720.0)),
        None,
    );
    assert_eq!(clamped.x, 2450.0);
    assert_eq!(clamped.y, 140.0);

    // Without known_monitors or active_monitor_rect, x=2450 is preserved and offset capture strip is still avoided.
    session.set_known_monitors(Vec::new());
    assert_eq!(session.resolve_monitor_for_overlay(on_secondary), None);
    let fallback_clamped = AppSession::clamp_overlay_geometry_with_optional_monitor(
        crate::session::OverlayGeometry {
            x: 2450.0,
            y: 160.0,
            width: 380.0,
            height: 230.0,
            click_through: false,
        },
        None,
        Some((1280.0, 720.0)),
        Some((2000.0, 180.0, 1200.0, 72.0)),
    );
    assert_eq!(fallback_clamped.x, 2450.0);
    assert_eq!(fallback_clamped.y, 252.0);
}

#[test]
fn overlay_geometry_mixed_dpi_normalizes_points_to_desktop_pixels_for_monitor_selection_and_capture_overlap()
 {
    let mut session = AppSession::new().unwrap();
    session.set_known_monitors(vec![
        crate::session::MonitorRect::new(0.0, 0.0, 1920.0, 1080.0),
        crate::session::MonitorRect::new(1920.0, 0.0, 2560.0, 1440.0),
    ]);

    let ppp = 1.5;
    let logical_pts = crate::session::OverlayGeometry {
        x: 1400.0,
        y: 120.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let physical_px = logical_pts.scale_to_desktop_pixels(ppp);
    assert_eq!(physical_px.x, 2100.0);
    assert_eq!(physical_px.y, 180.0);
    assert_eq!(physical_px.width, 570.0);
    assert_eq!(physical_px.height, 345.0);

    let resolved = session.resolve_monitor_for_overlay(physical_px);
    assert_eq!(
        resolved,
        Some(crate::session::MonitorRect::new(
            1920.0, 0.0, 2560.0, 1440.0
        ))
    );

    let clamped_px = AppSession::clamp_overlay_geometry_with_optional_monitor_scaled(
        physical_px,
        resolved,
        Some((2560.0, 1440.0)),
        Some((2100.0, 180.0, 1600.0, 72.0)),
        ppp,
    );
    assert_eq!(clamped_px.x, 2100.0);
    assert_eq!(clamped_px.y, 252.0);
    assert_eq!(clamped_px.width, 570.0);
    assert_eq!(clamped_px.height, 345.0);

    let clamped_pts = clamped_px.scale_to_egui_points(ppp);
    assert_eq!(clamped_pts.x, 1400.0);
    assert_eq!(clamped_pts.y, 168.0);
    assert_eq!(clamped_pts.width, 380.0);
    assert_eq!(clamped_pts.height, 230.0);
}

#[test]
fn resolve_effective_pixels_per_point_honors_native_scale_and_app_zoom() {
    assert!(
        (AppSession::resolve_effective_pixels_per_point(Some(1.5), 1.0, 1.2) - 1.8).abs() < 1e-5
    );
    assert!(
        (AppSession::resolve_effective_pixels_per_point(Some(1.5), 1.5, 1.0) - 1.5).abs() < 1e-5
    );
    assert!((AppSession::resolve_effective_pixels_per_point(None, 1.25, 1.25) - 1.25).abs() < 1e-5);
    assert!(
        (AppSession::resolve_effective_pixels_per_point(Some(0.0), 1.4, 1.0) - 1.4).abs() < 1e-5
    );
    assert!(
        (AppSession::resolve_effective_pixels_per_point(Some(1.5), 1.0, f32::NAN) - 1.5).abs()
            < 1e-5
    );
}

#[test]
fn ui_config_persists_ui_language_and_defaults_to_english_when_missing() {
    use crate::session::{UiConfig, UiLanguage, load_ui_config_from_dir, save_ui_config_to_dir};

    let tmp_dir =
        std::env::temp_dir().join(format!("truesight_ui_lang_cfg_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Missing config file defaults safely to English.
    let missing = load_ui_config_from_dir(&tmp_dir);
    assert_eq!(missing.ui_language, UiLanguage::En);

    // Unsupported language value falls back to English.
    std::fs::write(tmp_dir.join("ui_config.toml"), "ui_language = \"fr\"\n").unwrap();
    let invalid = load_ui_config_from_dir(&tmp_dir);
    assert_eq!(invalid.ui_language, UiLanguage::En);

    for lang in [UiLanguage::Es, UiLanguage::Ru, UiLanguage::En] {
        let cfg = UiConfig {
            ui_language: lang,
            ..UiConfig::default()
        };
        save_ui_config_to_dir(&tmp_dir, &cfg).unwrap();
        let reloaded = load_ui_config_from_dir(&tmp_dir);
        assert_eq!(
            reloaded.ui_language, lang,
            "ui_language {lang:?} must round-trip through ui_config.toml"
        );
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn active_build_hero_evicts_stale_local_hero_and_persists_independent_build_overlay_geometry() {
    use crate::session::{OverlayGeometry, load_ui_config_from_dir};
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{HeroId, Role, Side};

    let tmp_dir =
        std::env::temp_dir().join(format!("truesight_build_geom_cfg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // 1. Legacy ui_config.toml without build_overlay_* seeds build X/Y from Picks and uses 390x136 default.
    std::fs::write(
        tmp_dir.join("ui_config.toml"),
        "overlay_x = 140\noverlay_y = 220\noverlay_width = 430\noverlay_height = 290\n",
    )
    .unwrap();
    let legacy = load_ui_config_from_dir(&tmp_dir);
    assert_eq!(legacy.overlay_geometry.width, 430.0);
    assert_eq!(legacy.overlay_geometry.height, 290.0);
    assert_eq!(legacy.build_overlay_geometry.x, 140.0);
    assert_eq!(legacy.build_overlay_geometry.y, 220.0);
    assert_eq!(legacy.build_overlay_geometry.width, 420.0);
    assert_eq!(legacy.build_overlay_geometry.height, 168.0);

    // 2. Resizing Picks vs Build geometries persists independently via set_active_overlay_geometry.
    let cfg_path = tmp_dir.join("config.toml");
    let mgr = LocalCacheManager::with_optional_account(tmp_dir.join("cache"), None);
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path.clone())).unwrap();
    let picks_geom = OverlayGeometry {
        x: 50.0,
        y: 120.0,
        width: 440.0,
        height: 270.0,
        click_through: false,
    };
    session.set_active_overlay_geometry(picks_geom);
    session.toggle_post_draft_build_view();
    assert!(session.is_post_draft_build_view());
    let build_geom = OverlayGeometry {
        x: 90.0,
        y: 150.0,
        width: 520.0,
        height: 128.0,
        click_through: false,
    };
    session.set_active_overlay_geometry(build_geom);

    let reloaded_mgr = LocalCacheManager::with_optional_account(tmp_dir.join("cache"), None);
    let reloaded = AppSession::with_cache_and_config(reloaded_mgr, Some(cfg_path)).unwrap();
    assert_eq!(reloaded.overlay_geometry, picks_geom);
    assert_eq!(reloaded.build_overlay_geometry, build_geom);
    assert!(
        (reloaded.overlay_opacity - crate::session::DEFAULT_OVERLAY_OPACITY).abs() < 1e-4,
        "default overlay opacity must initialize to {}",
        crate::session::DEFAULT_OVERLAY_OPACITY
    );

    // 3. Custom overlay glass opacity persists and clamps out-of-range / NaN values safely.
    let mut cfg_session = reloaded;
    cfg_session.set_overlay_opacity(0.52);
    assert!((cfg_session.overlay_opacity - 0.52).abs() < 1e-4);
    cfg_session.set_overlay_opacity(0.10);
    assert!((cfg_session.overlay_opacity - crate::session::MIN_OVERLAY_OPACITY).abs() < 1e-4);
    cfg_session.set_overlay_opacity(1.50);
    assert!((cfg_session.overlay_opacity - crate::session::MAX_OVERLAY_OPACITY).abs() < 1e-4);
    cfg_session.set_overlay_opacity(f32::NAN);
    assert!((cfg_session.overlay_opacity - crate::session::DEFAULT_OVERLAY_OPACITY).abs() < 1e-4);
    cfg_session.set_overlay_opacity(0.54);
    let persisted = load_ui_config_from_dir(&tmp_dir);
    assert!((persisted.overlay_opacity - 0.54).abs() < 1e-4);

    // 4. Stale local_player_hero must update when slot hero is replaced or evicted, never staying stuck.
    session.set_slot_manual(Side::Allies, 3, HeroId(121)); // Grimstroke Pos 4
    session.local_player_hero = Some(HeroId(121));
    assert_eq!(
        session.active_build_hero(),
        Some((HeroId(121), Role::SoftSupport))
    );
    session.set_slot_manual(Side::Allies, 3, HeroId(50)); // Replaced with Dazzle Pos 4
    assert_eq!(
        session.active_build_hero(),
        Some((HeroId(50), Role::SoftSupport)),
        "replacing manual local hero slot must update active_build_hero instead of keeping Grimstroke"
    );

    // Even if local_player_hero pointed to a hero not in allies, active_build_hero falls back cleanly.
    session.local_player_hero = Some(HeroId(121));
    session.draft.target_role = Role::Carry;
    session.set_slot_manual(Side::Allies, 0, HeroId(95)); // Troll Warlord Pos 1
    assert_eq!(
        session.active_build_hero(),
        Some((HeroId(95), Role::Carry)),
        "evicted local_player_hero not in draft must fall back to active target_role hero"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn build_view_browsing_preserves_vision_replacement_without_setting_local_identity() {
    use truesight_core::domain::{HeroId, Side};
    use truesight_core::ports::DraftEvent;

    let mut session = AppSession::new().unwrap();
    // Vision detects two allies with zero GSI local-player identity.
    session.apply_vision_events(vec![
        DraftEvent::PickDetected {
            side: Side::Allies,
            slot: 0,
            hero_id: HeroId(1), // Anti-Mage
            role: None,
        },
        DraftEvent::PickDetected {
            side: Side::Allies,
            slot: 1,
            hero_id: HeroId(88), // Nyx Assassin
            role: None,
        },
    ]);
    assert_eq!(session.local_player_hero, None);

    // Browse the second ally's build; this must NOT hijack local_player_hero.
    session.select_build_view_hero(HeroId(88));
    assert_eq!(session.build_view_hero, Some(HeroId(88)));
    assert_eq!(session.local_player_hero, None);
    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(88))
    );

    // Subsequent confirmed vision replacement in portrait slot 1 replaces HeroId(88) with HeroId(50).
    session.apply_vision_events(vec![DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 1,
        hero_id: HeroId(50), // Dazzle
        role: None,
    }]);
    assert!(
        !session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(88)),
        "browsing a vision-owned ally must not shield it from confirmed vision replacement"
    );
    assert!(
        session
            .draft
            .allies
            .iter()
            .flatten()
            .any(|p| p.hero_id == HeroId(50)),
        "confirmed vision replacement hero (Dazzle) must occupy the allied draft"
    );
    assert_eq!(session.build_view_hero, None);
}

#[test]
fn item_build_refresh_triggers_on_gsi_entry_and_queues_while_sync_in_flight() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::Duration;
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role, Side};
    use truesight_core::ports::{DraftEvent, ItemMetaRepository};

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_item_refresh_queue_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cache_dir = tmp_dir.join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let cfg_path = tmp_dir.join("config.toml");

    let mgr = LocalCacheManager::with_optional_account(cache_dir.clone(), None);
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path)).unwrap();
    session.set_slot_manual(Side::Allies, 3, HeroId(88)); // Nyx Assassin (uncached)
    session.select_build_view_hero(HeroId(88));

    let calls = Arc::new(AtomicUsize::new(0));
    let recorded_targets: Arc<Mutex<Vec<Vec<HeroId>>>> = Arc::new(Mutex::new(Vec::new()));
    let (gate_tx, gate_rx) = mpsc::sync_channel::<()>(1);
    let gate_rx = Arc::new(Mutex::new(gate_rx));
    let block_next_call = Arc::new(Mutex::new(false));
    let (blocked_started_tx, blocked_started_rx) = mpsc::sync_channel::<()>(2);
    let fail_next_call = Arc::new(Mutex::new(false));

    let calls_bg = Arc::clone(&calls);
    let targets_bg = Arc::clone(&recorded_targets);
    let cache_dir_bg = cache_dir.clone();
    let gate_rx_bg = Arc::clone(&gate_rx);
    let block_bg = Arc::clone(&block_next_call);
    let blocked_started_bg = blocked_started_tx.clone();
    let fail_bg = Arc::clone(&fail_next_call);

    session.set_background_sync_runner(move |bg_mgr, _force| {
        calls_bg.fetch_add(1, Ordering::SeqCst);
        targets_bg
            .lock()
            .unwrap()
            .push(bg_mgr.target_item_hero_ids.clone());
        let should_block = {
            let mut flag = block_bg.lock().unwrap();
            let v = *flag;
            *flag = false;
            v
        };
        if should_block {
            let _ = blocked_started_bg.send(());
            let _ = gate_rx_bg.lock().unwrap().recv();
        }
        let should_fail = bg_mgr.target_item_hero_ids.contains(&HeroId(121)) && {
            let mut flag = fail_bg.lock().unwrap();
            let v = *flag;
            *flag = false;
            v
        };
        if should_fail {
            return Err("Simulated offline failure".into());
        }
        let mut entries = Vec::new();
        for hid in &bg_mgr.target_item_hero_ids {
            entries.push(format!(
                r#"{{"heroId":{},"itemStartingPurchase":[],"itemFullPurchase":[{{"itemId":1,"time":13,"matchCount":3210,"winCount":1850}}]}}"#,
                hid.0
            ));
        }
        let existing_path = cache_dir_bg.join("hero_item_stats.json");
        let existing_inner = std::fs::read_to_string(&existing_path)
            .ok()
            .map(|s| {
                s.trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string()
            })
            .filter(|s| !s.is_empty());
        if let Some(prev) = existing_inner {
            entries.insert(0, prev);
        }
        std::fs::write(&existing_path, format!("[{}]", entries.join(",")))
            .map_err(|e| e.to_string())?;
        Ok("Item cache synced".into())
    });

    // 1. Automatic GSI transition into PRE_GAME must trigger item refresh for uncached active hero.
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_PRE_GAME".into(),
        game_time_secs: -30,
    });
    assert!(
        session.sync_in_flight,
        "GSI transition to PRE_GAME must trigger item refresh for uncached active build hero"
    );
    wait_for_sync_idle(&mut session);
    assert!(!session.sync_in_flight);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        recorded_targets.lock().unwrap()[0].contains(&HeroId(88)),
        "first sync must target active build hero Nyx Assassin (88)"
    );
    let nyx_stats = session
        .item_repo
        .hero_item_stats(HeroId(88), Role::SoftSupport, Bracket::Ancient)
        .unwrap();
    assert!(
        nyx_stats
            .iter()
            .any(|s| s.item_id == ItemId(1) && s.match_count == 3210),
        "poll_sync_completion must reload session.item_repo with synced stats: {nyx_stats:?}"
    );

    // Repeating GAME_IN_PROGRESS once already in Build view & cached must not spam extra syncs.
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 5,
    });
    assert!(!session.sync_in_flight);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // 2. Manual Build toggle while another sync is in flight must stay pending and run once on completion.
    session.toggle_post_draft_build_view(); // Switch back to Picks
    assert!(!session.is_post_draft_build_view());
    *block_next_call.lock().unwrap() = true;
    assert!(session.start_background_sync(true));
    blocked_started_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    session.set_slot_manual(Side::Allies, 4, HeroId(50)); // Dazzle (uncached)
    session.select_build_view_hero(HeroId(50));
    session.toggle_post_draft_build_view(); // Manual entry into Build view during in-flight sync
    assert!(session.is_post_draft_build_view());
    gate_tx.send(()).unwrap();

    wait_for_sync_until(&mut session, || calls.load(Ordering::SeqCst) >= 3);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "manual Build entry during in-flight sync must queue and drain exactly one follow-up sync"
    );
    assert!(
        recorded_targets.lock().unwrap()[2].contains(&HeroId(50)),
        "queued follow-up sync must target Dazzle (50)"
    );

    // 3. GSI-driven Build entry during an in-flight sync must also queue cleanly and never loop on error.
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
        game_time_secs: 0,
    });
    assert!(!session.is_post_draft_build_view());
    *block_next_call.lock().unwrap() = true;
    assert!(session.start_background_sync(true));
    blocked_started_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();

    session.set_slot_manual(Side::Allies, 1, HeroId(121)); // Grimstroke (uncached)
    session.select_build_view_hero(HeroId(121));
    *fail_next_call.lock().unwrap() = true; // Next follow-up fails to verify no retry loop
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 120,
    });
    assert!(session.is_post_draft_build_view());
    gate_tx.send(()).unwrap();

    wait_for_sync_until(&mut session, || calls.load(Ordering::SeqCst) >= 5);
    assert!(!session.sync_in_flight);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "GSI Build entry during in-flight sync must launch one queued refresh and stop on error"
    );
    assert!(
        recorded_targets.lock().unwrap()[4].contains(&HeroId(121)),
        "GSI queued refresh must target Grimstroke (121)"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn local_inventory_applies_only_to_local_hero_build_not_browsed_teammate() {
    use truesight_core::domain::{HeroId, ItemId};
    use truesight_core::ports::{DraftEvent, ItemMetaRepository};

    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 anti-mage").unwrap());
    assert!(execute_command(&mut session, "ally 4 nyx assassin").unwrap());
    assert!(execute_command(&mut session, "enemy 1 phantom assassin").unwrap());
    assert!(execute_command(&mut session, "enemy 4 riki").unwrap());

    session.apply_event(DraftEvent::LocalPlayerHeroDetected {
        hero_slug: "antimage".into(),
    });
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 900,
    });
    assert!(session.is_post_draft_build_view());
    assert_eq!(session.active_build_hero().map(|(h, _)| h), Some(HeroId(1)));

    let am_base = session
        .compute_recommended_build()
        .expect("local Anti-Mage must produce a build");
    let am_step0_id = am_base.steps[0].item_id;

    // Inspect Nyx build first (before acquiring items) to collect real Nyx step, variant, start, and optional IDs.
    session.select_build_view_hero(HeroId(88));
    let nyx_base = session
        .compute_recommended_build()
        .expect("teammate Nyx must produce a build");
    let nyx_step0_id = nyx_base.steps[0].item_id;
    let nyx_variant_pair = nyx_base
        .steps
        .iter()
        .find_map(|s| s.variants.first().map(|v| (s.item_id, v.item_id)));
    let nyx_start_id = nyx_base.starting_items.first().copied();
    let nyx_opt_id = nyx_base.optional_items.first().map(|o| o.item_id);

    // Clear explicit browse selection back to local hero and equip overlapping items from both builds.
    session.build_view_hero = None;
    let mut owned_ids: Vec<ItemId> = vec![am_step0_id, nyx_step0_id];
    if let Some((_, var_id)) = nyx_variant_pair {
        owned_ids.push(var_id);
    }
    if let Some(sid) = nyx_start_id {
        owned_ids.push(sid);
    }
    if let Some(oid) = nyx_opt_id {
        owned_ids.push(oid);
    }
    let item_slugs: Vec<String> = owned_ids
        .into_iter()
        .filter_map(|id| {
            session
                .item_repo
                .item_catalog(id)
                .map(|e| format!("item_{}", e.slug))
        })
        .collect();
    session.apply_event(DraftEvent::InventoryUpdated {
        hero_slug: Some("antimage".into()),
        item_slugs,
    });

    // 1. On the local player hero (Anti-Mage), owned_items apply normally.
    assert_eq!(
        session.active_build_owned_items(),
        session.owned_items.as_slice()
    );
    let am_owned = session.compute_recommended_build().unwrap();
    assert!(am_owned.steps[0].owned);
    assert_ne!(am_owned.next_unowned_index, Some(0));

    // 2. Switching to teammate Nyx must NOT apply local inventory to primary, variant, start, or optional items.
    session.select_build_view_hero(HeroId(88));
    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(88))
    );
    assert!(
        session.active_build_owned_items().is_empty(),
        "browsing teammate Nyx must return empty active_build_owned_items"
    );
    let nyx_browsed = session.compute_recommended_build().unwrap();
    assert!(
        nyx_browsed
            .steps
            .iter()
            .all(|s| !s.owned && s.owned_components == 0),
        "teammate build steps must never be marked owned from local inventory: {:?}",
        nyx_browsed.steps
    );
    assert_eq!(nyx_browsed.next_unowned_index, Some(0));
    assert!(
        nyx_browsed.optional_items.iter().all(|o| !o.owned),
        "teammate optional items must never be marked owned from local inventory"
    );

    // Even after swapping to a variant whose item exists in local owned_items, teammate ownership stays false.
    if let Some((primary_id, var_id)) = nyx_variant_pair {
        session.select_build_variant_for_item(primary_id, var_id);
        let nyx_swapped = session.compute_recommended_build().unwrap();
        let swapped_step = nyx_swapped
            .steps
            .iter()
            .find(|s| s.item_id == var_id)
            .expect("swapped variant must appear as step item");
        assert!(
            !swapped_step.owned && swapped_step.owned_components == 0,
            "swapped variant on teammate build must remain unowned"
        );
    }

    // 3. Switching explicitly back to the local hero restores owned_items application.
    session.select_build_view_hero(HeroId(1));
    assert_eq!(
        session.active_build_owned_items(),
        session.owned_items.as_slice()
    );
    let am_restored = session.compute_recommended_build().unwrap();
    assert!(am_restored.steps[0].owned);
    assert_ne!(am_restored.next_unowned_index, Some(0));
}

#[test]
fn manual_slot_replacement_in_build_view_requests_or_queues_item_refresh() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::Duration;
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role, Side};
    use truesight_core::ports::ItemMetaRepository;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_manual_replace_refresh_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cache_dir = tmp_dir.join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let cfg_path = tmp_dir.join("config.toml");

    let mgr = LocalCacheManager::with_optional_account(cache_dir.clone(), None);
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path)).unwrap();
    session.set_slot_manual(Side::Allies, 0, HeroId(1)); // Anti-Mage

    let calls = Arc::new(AtomicUsize::new(0));
    let recorded_targets: Arc<Mutex<Vec<Vec<HeroId>>>> = Arc::new(Mutex::new(Vec::new()));
    let (gate_tx, gate_rx) = mpsc::sync_channel::<()>(1);
    let gate_rx = Arc::new(Mutex::new(gate_rx));
    let block_next_call = Arc::new(Mutex::new(false));
    let (blocked_started_tx, blocked_started_rx) = mpsc::sync_channel::<()>(2);

    let calls_bg = Arc::clone(&calls);
    let targets_bg = Arc::clone(&recorded_targets);
    let cache_dir_bg = cache_dir.clone();
    let gate_rx_bg = Arc::clone(&gate_rx);
    let block_bg = Arc::clone(&block_next_call);
    let blocked_started_bg = blocked_started_tx.clone();

    session.set_background_sync_runner(move |bg_mgr, _force| {
        calls_bg.fetch_add(1, Ordering::SeqCst);
        targets_bg
            .lock()
            .unwrap()
            .push(bg_mgr.target_item_hero_ids.clone());
        let should_block = {
            let mut flag = block_bg.lock().unwrap();
            let v = *flag;
            *flag = false;
            v
        };
        if should_block {
            let _ = blocked_started_bg.send(());
            let _ = gate_rx_bg.lock().unwrap().recv();
        }
        let mut entries = Vec::new();
        for hid in &bg_mgr.target_item_hero_ids {
            entries.push(format!(
                r#"{{"heroId":{},"itemStartingPurchase":[],"itemFullPurchase":[{{"itemId":1,"time":12,"matchCount":4321,"winCount":2400}}]}}"#,
                hid.0
            ));
        }
        let existing_path = cache_dir_bg.join("hero_item_stats.json");
        let existing_inner = std::fs::read_to_string(&existing_path)
            .ok()
            .map(|s| {
                s.trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string()
            })
            .filter(|s| !s.is_empty());
        if let Some(prev) = existing_inner {
            entries.insert(0, prev);
        }
        std::fs::write(&existing_path, format!("[{}]", entries.join(",")))
            .map_err(|e| e.to_string())?;
        Ok("Item cache synced".into())
    });

    // Open Build view and drain initial sync for Anti-Mage.
    session.toggle_post_draft_build_view();
    assert!(session.is_post_draft_build_view());
    assert!(session.sync_in_flight);
    wait_for_sync_idle(&mut session);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // 1. Replace active slot while Build view is open -> immediate background refresh for Nyx (88).
    session.set_slot_manual(Side::Allies, 0, HeroId(88));
    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(88))
    );
    assert!(
        session.sync_in_flight,
        "replacing active build ally in Build view must start background refresh when uncached"
    );
    wait_for_sync_idle(&mut session);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(recorded_targets.lock().unwrap()[1].contains(&HeroId(88)));
    let nyx_stats = session
        .item_repo
        .hero_item_stats(HeroId(88), Role::Carry, Bracket::Ancient)
        .unwrap();
    assert!(
        nyx_stats
            .iter()
            .any(|s| s.item_id == ItemId(1) && s.match_count == 4321),
        "replacement sync must populate item stats for replaced hero: {nyx_stats:?}"
    );

    // 2. Replace browsed ally while another sync is in flight -> must queue pending refresh for Dazzle (50).
    session.select_build_view_hero(HeroId(88));
    *block_next_call.lock().unwrap() = true;
    assert!(session.start_background_sync(true));
    blocked_started_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    session.set_slot_manual(Side::Allies, 0, HeroId(50));
    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(50))
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "replacing active build ally during an in-flight sync must wait for active sync instead of spawning duplicate"
    );
    gate_tx.send(()).unwrap();
    wait_for_sync_until(&mut session, || calls.load(Ordering::SeqCst) >= 4);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert!(recorded_targets.lock().unwrap()[3].contains(&HeroId(50)));

    // 3. Clearing the active slot while Build is open falls back to remaining uncached ally (Grimstroke 121).
    session.set_slot_manual(Side::Allies, 3, HeroId(121)); // inactive slot -> no spurious sync
    assert!(!session.sync_in_flight);
    session.clear_slot_manual(Side::Allies, 0);
    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(121))
    );
    assert!(
        session.sync_in_flight,
        "clearing active slot in Build view must refresh newly fallen-back uncached hero"
    );
    wait_for_sync_idle(&mut session);
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    assert!(recorded_targets.lock().unwrap()[4].contains(&HeroId(121)));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn clearing_confirmed_local_hero_keeps_inventory_bound_and_does_not_leak_to_fallback_ally() {
    use truesight_core::domain::{HeroId, ItemId, Side};
    use truesight_core::ports::{DraftEvent, ItemMetaRepository};

    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 anti-mage").unwrap());
    assert!(execute_command(&mut session, "ally 4 nyx assassin").unwrap());
    assert!(execute_command(&mut session, "enemy 1 phantom assassin").unwrap());
    assert!(execute_command(&mut session, "enemy 4 riki").unwrap());

    session.apply_event(DraftEvent::LocalPlayerHeroDetected {
        hero_slug: "antimage".into(),
    });
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 900,
    });
    assert!(session.is_post_draft_build_view());
    assert_eq!(session.build_view_hero, None);
    assert_eq!(session.active_build_hero().map(|(h, _)| h), Some(HeroId(1)));

    let am_base = session
        .compute_recommended_build()
        .expect("local Anti-Mage must produce a build");
    let am_step0_id = am_base.steps[0].item_id;

    // Collect Nyx step, variant, starting, and optional IDs before restoring default focus.
    session.select_build_view_hero(HeroId(88));
    let nyx_base = session
        .compute_recommended_build()
        .expect("fallback ally Nyx must produce a build");
    let nyx_step0_id = nyx_base.steps[0].item_id;
    let nyx_variant_pair = nyx_base
        .steps
        .iter()
        .find_map(|s| s.variants.first().map(|v| (s.item_id, v.item_id)));
    let nyx_start_id = nyx_base.starting_items.first().copied();
    let nyx_opt_id = nyx_base.optional_items.first().map(|o| o.item_id);
    session.build_view_hero = None;

    let mut owned_ids: Vec<ItemId> = vec![am_step0_id, nyx_step0_id];
    if let Some((_, var_id)) = nyx_variant_pair {
        owned_ids.push(var_id);
    }
    if let Some(sid) = nyx_start_id {
        owned_ids.push(sid);
    }
    if let Some(oid) = nyx_opt_id {
        owned_ids.push(oid);
    }
    let item_slugs: Vec<String> = owned_ids
        .into_iter()
        .filter_map(|id| {
            session
                .item_repo
                .item_catalog(id)
                .map(|e| format!("item_{}", e.slug))
        })
        .collect();
    session.apply_event(DraftEvent::InventoryUpdated {
        hero_slug: Some("antimage".into()),
        item_slugs,
    });

    assert_eq!(session.owned_items_owner, Some(HeroId(1)));
    assert_eq!(
        session.active_build_owned_items(),
        session.owned_items.as_slice()
    );
    let am_owned = session.compute_recommended_build().unwrap();
    assert!(am_owned.steps[0].owned);

    // Click '×' on confirmed local hero Anti-Mage with no explicit browse selection:
    // fallback ally Nyx becomes active_build_hero, while inventory stays bound to Anti-Mage.
    session.clear_slot_manual(Side::Allies, 0);
    assert_eq!(session.local_player_hero, None);
    assert_eq!(session.build_view_hero, None);
    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(88)),
        "clearing Ally 1 must fall back active_build_hero to Ally 4 Nyx"
    );
    assert!(
        !session.owned_items.is_empty(),
        "inventory remains recorded while bound to its confirmed owner"
    );
    assert_eq!(session.owned_items_owner, Some(HeroId(1)));
    assert!(
        session.active_build_owned_items().is_empty(),
        "fallback ally must NOT inherit cleared local hero's inventory"
    );

    let nyx_fallback = session.compute_recommended_build().unwrap();
    assert!(
        nyx_fallback
            .steps
            .iter()
            .all(|s| !s.owned && s.owned_components == 0),
        "fallback ally steps must remain unowned after clearing local hero: {:?}",
        nyx_fallback.steps
    );
    assert_eq!(nyx_fallback.next_unowned_index, Some(0));
    assert!(
        nyx_fallback.optional_items.iter().all(|o| !o.owned),
        "fallback ally optional items must remain unowned after clearing local hero"
    );

    // Re-adding Anti-Mage manually restores ownership recognition for Anti-Mage's build.
    session.set_slot_manual(Side::Allies, 0, HeroId(1));
    assert_eq!(
        session.active_build_owned_items(),
        session.owned_items.as_slice()
    );
    let am_readded = session.compute_recommended_build().unwrap();
    assert!(am_readded.steps[0].owned);

    // Resetting the draft clears both inventory items and their bound owner.
    session.reset_draft();
    assert!(session.owned_items.is_empty());
    assert_eq!(session.owned_items_owner, None);
}

#[test]
fn vision_populating_active_build_hero_in_build_view_triggers_item_refresh() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role, Side};
    use truesight_core::ports::{DraftEvent, ItemMetaRepository};

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_vision_build_refresh_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cache_dir = tmp_dir.join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let cfg_path = tmp_dir.join("config.toml");

    let mgr = LocalCacheManager::with_optional_account(cache_dir.clone(), None);
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path)).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let recorded_targets: Arc<Mutex<Vec<Vec<HeroId>>>> = Arc::new(Mutex::new(Vec::new()));
    let calls_bg = Arc::clone(&calls);
    let targets_bg = Arc::clone(&recorded_targets);
    let cache_dir_bg = cache_dir.clone();

    session.set_background_sync_runner(move |bg_mgr, _force| {
        calls_bg.fetch_add(1, Ordering::SeqCst);
        targets_bg
            .lock()
            .unwrap()
            .push(bg_mgr.target_item_hero_ids.clone());
        let mut entries = Vec::new();
        for hid in &bg_mgr.target_item_hero_ids {
            entries.push(format!(
                r#"{{"heroId":{},"itemStartingPurchase":[],"itemFullPurchase":[{{"itemId":1,"time":11,"matchCount":5120,"winCount":2900}}]}}"#,
                hid.0
            ));
        }
        std::fs::write(
            cache_dir_bg.join("hero_item_stats.json"),
            format!("[{}]", entries.join(",")),
        )
        .map_err(|e| e.to_string())?;
        Ok("Item cache synced".into())
    });

    // Open Build view while no allies are picked yet.
    session.toggle_post_draft_build_view();
    assert!(session.is_post_draft_build_view());
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // Vision detects an uncached allied hero while Build view is already open.
    session.apply_vision_events(vec![DraftEvent::PickDetected {
        side: Side::Allies,
        slot: 0,
        hero_id: HeroId(88),
        role: None,
    }]);

    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(88))
    );
    assert!(
        session.sync_in_flight,
        "vision populating active build hero in Build view must start item refresh"
    );

    wait_for_sync_idle(&mut session);
    assert!(!session.sync_in_flight);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(recorded_targets.lock().unwrap()[0].contains(&HeroId(88)));
    let nyx_stats = session
        .item_repo
        .hero_item_stats(HeroId(88), Role::Carry, Bracket::Ancient)
        .unwrap();
    assert!(
        nyx_stats
            .iter()
            .any(|s| s.item_id == ItemId(1) && s.match_count == 5120),
        "vision-triggered item refresh must populate item_repo for Ally Nyx: {nyx_stats:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn unlinking_steam_account_preserves_and_drains_queued_item_refresh() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::Duration;
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{Bracket, HeroId, ItemId, Role, Side};
    use truesight_core::ports::ItemMetaRepository;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_unlink_preserves_queue_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let cache_dir = tmp_dir.join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let cfg_path = tmp_dir.join("config.toml");

    let mgr = LocalCacheManager::with_optional_account(cache_dir.clone(), Some(12345));
    let mut session = AppSession::with_cache_and_config(mgr, Some(cfg_path)).unwrap();
    session.set_slot_manual(Side::Allies, 0, HeroId(1)); // Anti-Mage

    let calls = Arc::new(AtomicUsize::new(0));
    let recorded_targets: Arc<Mutex<Vec<Vec<HeroId>>>> = Arc::new(Mutex::new(Vec::new()));
    let (gate_tx, gate_rx) = mpsc::sync_channel::<()>(1);
    let gate_rx = Arc::new(Mutex::new(gate_rx));
    let block_next_call = Arc::new(Mutex::new(false));
    let (blocked_started_tx, blocked_started_rx) = mpsc::sync_channel::<()>(2);
    let file_lock = Arc::new(Mutex::new(()));

    let calls_bg = Arc::clone(&calls);
    let targets_bg = Arc::clone(&recorded_targets);
    let cache_dir_bg = cache_dir.clone();
    let gate_rx_bg = Arc::clone(&gate_rx);
    let block_bg = Arc::clone(&block_next_call);
    let blocked_started_bg = blocked_started_tx.clone();
    let file_lock_bg = Arc::clone(&file_lock);

    session.set_background_sync_runner(move |bg_mgr, _force| {
        let call_num = calls_bg.fetch_add(1, Ordering::SeqCst) + 1;
        targets_bg
            .lock()
            .unwrap()
            .push(bg_mgr.target_item_hero_ids.clone());
        let should_block = {
            let mut flag = block_bg.lock().unwrap();
            let v = *flag;
            *flag = false;
            v
        };
        if should_block {
            let _ = blocked_started_bg.send(());
            let _ = gate_rx_bg.lock().unwrap().recv();
        }
        let _guard = file_lock_bg.lock().unwrap();
        let mut entries = Vec::new();
        for hid in &bg_mgr.target_item_hero_ids {
            entries.push(format!(
                r#"{{"heroId":{},"itemStartingPurchase":[],"itemFullPurchase":[{{"itemId":1,"time":15,"matchCount":6789,"winCount":3600}}]}}"#,
                hid.0
            ));
        }
        let existing_path = cache_dir_bg.join("hero_item_stats.json");
        let existing_inner = std::fs::read_to_string(&existing_path)
            .ok()
            .map(|s| {
                s.trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string()
            })
            .filter(|s| !s.is_empty());
        if let Some(prev) = existing_inner {
            entries.insert(0, prev);
        }
        let tmp_path = cache_dir_bg.join(format!("hero_item_stats.{call_num}.tmp"));
        std::fs::write(&tmp_path, format!("[{}]", entries.join(","))).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp_path, &existing_path).map_err(|e| e.to_string())?;
        Ok("Item cache synced".into())
    });

    // Open Build view and drain initial sync for Anti-Mage (call #1).
    session.toggle_post_draft_build_view();
    assert!(session.is_post_draft_build_view());
    wait_for_sync_idle(&mut session);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Start a blocked sync (call #2), then replace active ally with uncached Nyx (88) -> queues refresh.
    *block_next_call.lock().unwrap() = true;
    assert!(session.start_background_sync(true));
    blocked_started_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    session.set_slot_manual(Side::Allies, 0, HeroId(88));
    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(88))
    );

    // Unlink Steam account while sync #2 is in flight and queued refresh is pending.
    session.set_account_id(None, true);
    assert!(
        session.sync_in_flight,
        "unlinking Steam account must immediately drain pending queued item refresh into a fresh sync"
    );

    // Wait for the fresh post-unlink sync #3 to complete and reload before unblocking cancelled sync #2.
    wait_for_sync_until(&mut session, || calls.load(Ordering::SeqCst) >= 3);
    assert!(!session.sync_in_flight);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(
        recorded_targets.lock().unwrap()[2].contains(&HeroId(88)),
        "post-unlink queued refresh must target replaced hero Nyx (88): {:?}",
        *recorded_targets.lock().unwrap()
    );
    let nyx_stats = session
        .item_repo
        .hero_item_stats(HeroId(88), Role::Carry, Bracket::Ancient)
        .unwrap();
    assert!(
        nyx_stats
            .iter()
            .any(|s| s.item_id == ItemId(1) && s.match_count == 6789),
        "post-unlink item refresh must reload item_repo with synced Nyx stats: {nyx_stats:?}"
    );

    gate_tx.send(()).unwrap();
    drop(session);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn identity_change_without_inventory_event_does_not_reassign_stored_inventory() {
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{Faction, HeroId, ItemId};
    use truesight_core::ports::{DraftEvent, ItemMetaRepository};

    let tmp_dir =
        std::env::temp_dir().join(format!("truesight_identity_no_inv_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    let mut session = AppSession::with_cache_and_config(mgr, None).unwrap();
    assert!(execute_command(&mut session, "ally 1 anti-mage").unwrap());
    assert!(execute_command(&mut session, "ally 4 nyx assassin").unwrap());
    assert!(execute_command(&mut session, "enemy 1 phantom assassin").unwrap());
    assert!(execute_command(&mut session, "enemy 4 riki").unwrap());

    // Confirm Hero A (Anti-Mage) and enter post-draft Build view.
    session.apply_event(DraftEvent::LocalPlayerHeroDetected {
        hero_slug: "antimage".into(),
    });
    session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 900,
    });
    assert!(session.is_post_draft_build_view());

    let am_base = session
        .compute_recommended_build()
        .expect("Anti-Mage must produce a build");
    let am_step0_id = am_base.steps[0].item_id;

    // Collect Nyx items before clearing back to default focus.
    session.select_build_view_hero(HeroId(88));
    let nyx_base = session
        .compute_recommended_build()
        .expect("Nyx Assassin must produce a build");
    let nyx_step0_id = nyx_base.steps[0].item_id;
    let nyx_variant_id = nyx_base
        .steps
        .iter()
        .find_map(|s| s.variants.first().map(|v| v.item_id));
    let nyx_start_id = nyx_base.starting_items.first().copied();
    let nyx_opt_id = nyx_base.optional_items.first().map(|o| o.item_id);
    session.build_view_hero = None;

    let mut owned_ids: Vec<ItemId> = vec![am_step0_id, nyx_step0_id];
    if let Some(vid) = nyx_variant_id {
        owned_ids.push(vid);
    }
    if let Some(sid) = nyx_start_id {
        owned_ids.push(sid);
    }
    if let Some(oid) = nyx_opt_id {
        owned_ids.push(oid);
    }
    let to_slugs = |s: &AppSession, ids: &[ItemId]| -> Vec<String> {
        ids.iter()
            .filter_map(|id| {
                s.item_repo
                    .item_catalog(*id)
                    .map(|e| format!("item_{}", e.slug))
            })
            .collect()
    };

    let mixed_slugs = to_slugs(&session, &owned_ids);
    session.apply_event(DraftEvent::InventoryUpdated {
        hero_slug: Some("antimage".into()),
        item_slugs: mixed_slugs,
    });
    assert_eq!(session.owned_items_owner, Some(HeroId(1)));
    assert!(session.compute_recommended_build().unwrap().steps[0].owned);

    // 1. Identity event for Hero B (Nyx) arrives WITHOUT a new InventoryUpdated:
    // Nyx must NOT inherit Anti-Mage's stored purchases or advance [NEXT].
    session.apply_event(DraftEvent::LocalPlayerHeroDetected {
        hero_slug: "nyx_assassin".into(),
    });
    assert_eq!(session.local_player_hero, Some(HeroId(88)));
    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(88))
    );
    assert!(
        session.active_build_owned_items().is_empty(),
        "Hero B must not inherit Hero A's stored inventory after LocalPlayerHeroDetected without InventoryUpdated"
    );
    let nyx_before_inv = session.compute_recommended_build().unwrap();
    assert!(
        nyx_before_inv
            .steps
            .iter()
            .all(|s| !s.owned && s.owned_components == 0),
        "Hero B build steps must stay unowned before Hero B's InventoryUpdated: {:?}",
        nyx_before_inv.steps
    );
    assert_eq!(nyx_before_inv.next_unowned_index, Some(0));
    assert!(
        nyx_before_inv.optional_items.iter().all(|o| !o.owned),
        "Hero B optional items must stay unowned before Hero B's InventoryUpdated"
    );

    // Once Nyx's own InventoryUpdated arrives, Nyx becomes the owner and marks its item owned.
    let nyx_slug = to_slugs(&session, &[nyx_step0_id]);
    session.apply_event(DraftEvent::InventoryUpdated {
        hero_slug: Some("nyx_assassin".into()),
        item_slugs: nyx_slug,
    });
    assert_eq!(session.owned_items_owner, Some(HeroId(88)));
    let nyx_after_inv = session.compute_recommended_build().unwrap();
    assert!(nyx_after_inv.steps[0].owned);
    assert_ne!(nyx_after_inv.next_unowned_index, Some(0));

    // 2. PlayerHeroMapped path: account links and emits Hero A (Anti-Mage) WITHOUT InventoryUpdated:
    // Anti-Mage must NOT inherit Nyx's stored purchases either.
    session.set_account_id(Some(4242), true);
    session.apply_event(DraftEvent::PlayerHeroMapped {
        steam32: 4242,
        faction: Faction::Radiant,
        hero_slug: "antimage".into(),
    });
    assert_eq!(session.local_player_hero, Some(HeroId(1)));
    assert_eq!(session.active_build_hero().map(|(h, _)| h), Some(HeroId(1)));
    assert!(
        session.active_build_owned_items().is_empty(),
        "PlayerHeroMapped identity switch without InventoryUpdated must not inherit previous hero's inventory"
    );
    let am_after_map = session.compute_recommended_build().unwrap();
    assert!(
        am_after_map
            .steps
            .iter()
            .all(|s| !s.owned && s.owned_components == 0),
        "Anti-Mage steps must stay unowned until fresh InventoryUpdated arrives: {:?}",
        am_after_map.steps
    );
    assert_eq!(am_after_map.next_unowned_index, Some(0));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn gsi_queue_coalescing_and_overflow_never_assign_previous_hero_inventory_to_next_hero() {
    use truesight_adapters::gsi::{BoundedDraftQueue, parse_gsi_json_events};
    use truesight_core::domain::{HeroId, ItemId};
    use truesight_core::ports::ItemMetaRepository;

    // 1. Live queue coalescing: Anti-Mage with Magic Wand -> Nyx Assassin without items before poll.
    let mut session = AppSession::new().unwrap();
    let queue = BoundedDraftQueue::new(16);

    let am_with_wand = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 600 },
        "player": { "team_name": "radiant" },
        "hero": { "name": "npc_dota_hero_antimage" },
        "items": { "slot0": { "name": "item_magic_wand" } }
    }"#;
    let nyx_without_items = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 601 },
        "player": { "team_name": "radiant" },
        "hero": { "name": "npc_dota_hero_nyx_assassin" }
    }"#;

    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(am_with_wand)));
    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(nyx_without_items)));

    for ev in queue.drain_and_reconcile_gsi() {
        session.apply_event(ev);
    }

    assert_eq!(session.local_player_hero, Some(HeroId(88)));
    assert_eq!(
        session.active_build_hero().map(|(h, _)| h),
        Some(HeroId(88))
    );
    assert!(
        session.active_build_owned_items().is_empty(),
        "queued coalescing from Anti-Mage + wand -> Nyx without items must not assign Anti-Mage's wand to Nyx"
    );
    let nyx_before = session.compute_recommended_build().unwrap();
    assert!(
        nyx_before
            .steps
            .iter()
            .all(|s| !s.owned && s.owned_components == 0),
        "Nyx build steps must stay unowned when previous queued hero's items were discarded: {:?}",
        nyx_before.steps
    );
    assert_eq!(nyx_before.next_unowned_index, Some(0));
    assert!(nyx_before.optional_items.iter().all(|o| !o.owned));

    // Fresh inventory for Nyx arrives through the queue and applies normally.
    let nyx_step0_id = nyx_before.steps[0].item_id;
    let nyx_step0_slug = session
        .item_repo
        .item_catalog(nyx_step0_id)
        .expect("Nyx step0 item must exist in catalog")
        .slug;
    let nyx_with_own_item = format!(
        r#"{{
            "map": {{ "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 605 }},
            "player": {{ "team_name": "radiant" }},
            "hero": {{ "name": "npc_dota_hero_nyx_assassin" }},
            "items": {{ "slot0": {{ "name": "item_{nyx_step0_slug}" }} }}
        }}"#
    );
    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(&nyx_with_own_item)));
    for ev in queue.drain_and_reconcile_gsi() {
        session.apply_event(ev);
    }
    assert_eq!(session.owned_items_owner, Some(HeroId(88)));
    assert_eq!(session.active_build_owned_items(), &[nyx_step0_id]);
    let nyx_after = session.compute_recommended_build().unwrap();
    assert!(nyx_after.steps[0].owned);
    assert_ne!(nyx_after.next_unowned_index, Some(0));

    // 2. Overflow recovery path: Axe with Blink Dagger -> overflowed Earthshaker without items.
    let mut overflow_session = AppSession::new().unwrap();
    let small_queue = BoundedDraftQueue::new(3);
    let axe_with_blink = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 900 },
        "player": { "team_name": "radiant" },
        "hero": { "name": "npc_dota_hero_axe" },
        "items": { "slot0": { "name": "item_blink" } },
        "draft": { "team2": { "ban0_id": 3, "ban1_id": 4, "ban2_id": 6, "ban3_id": 7 } }
    }"#;
    let es_without_items = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 902 },
        "player": { "team_name": "radiant" },
        "hero": { "name": "npc_dota_hero_earthshaker" }
    }"#;
    assert!(small_queue.record_and_push_gsi_events(parse_gsi_json_events(axe_with_blink)));
    assert!(small_queue.record_and_push_gsi_events(parse_gsi_json_events(es_without_items)));

    for ev in small_queue.drain_and_reconcile_gsi() {
        overflow_session.apply_event(ev);
    }
    let q_stats = small_queue.stats();
    assert!(
        q_stats.dropped > 0 && q_stats.recovered_resyncs == 1,
        "expected overflow reconciliation to run: {q_stats:?}"
    );
    let es_id = overflow_session.find_hero("earthshaker").unwrap();
    assert_eq!(overflow_session.local_player_hero, Some(es_id));
    assert!(
        overflow_session.active_build_owned_items().is_empty(),
        "overflow recovery must not replay Axe's Blink Dagger onto Earthshaker"
    );
    let es_before = overflow_session.compute_recommended_build().unwrap();
    assert!(
        es_before
            .steps
            .iter()
            .all(|s| !s.owned && s.owned_components == 0),
        "Earthshaker steps must stay unowned after overflow resync: {:?}",
        es_before.steps
    );

    // Fresh Blink Dagger event for Earthshaker applies cleanly after overflow recovery.
    let es_with_blink = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 910 },
        "player": { "team_name": "radiant" },
        "hero": { "name": "npc_dota_hero_earthshaker" },
        "items": { "slot0": { "name": "item_blink" } }
    }"#;
    assert!(small_queue.record_and_push_gsi_events(parse_gsi_json_events(es_with_blink)));
    for ev in small_queue.drain_and_reconcile_gsi() {
        overflow_session.apply_event(ev);
    }
    assert_eq!(overflow_session.owned_items_owner, Some(es_id));
    assert_eq!(overflow_session.active_build_owned_items(), &[ItemId(1)]);
}

#[test]
fn gsi_queue_preserves_items_only_update_when_same_hero_identity_repeats() {
    use truesight_adapters::gsi::{BoundedDraftQueue, parse_gsi_json_events};
    use truesight_core::domain::HeroId;

    let mut session = AppSession::new().unwrap();
    let wand_id = session
        .item_repo
        .item_by_slug("magic_wand")
        .expect("magic_wand must exist in catalog")
        .id;
    let queue = BoundedDraftQueue::new(16);

    // 1. Confirm Anti-Mage and drain the queue.
    let am_initial = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 400 },
        "player": { "team_name": "radiant" },
        "hero": { "name": "npc_dota_hero_antimage" }
    }"#;
    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(am_initial)));
    for ev in queue.drain_and_reconcile_gsi() {
        session.apply_event(ev);
    }
    assert_eq!(session.local_player_hero, Some(HeroId(1)));

    // 2. Push items-only payload (no "hero" field), followed before drain by repeated Anti-Mage identity without items.
    let items_only_wand = r#"{
        "player": { "team_name": "radiant" },
        "items": { "slot0": { "name": "item_magic_wand" } }
    }"#;
    let am_repeated_no_items = r#"{
        "map": { "game_state": "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS", "clock_time": 405 },
        "player": { "team_name": "radiant" },
        "hero": { "name": "npc_dota_hero_antimage" }
    }"#;
    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(items_only_wand)));
    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(am_repeated_no_items)));

    for ev in queue.drain_and_reconcile_gsi() {
        session.apply_event(ev);
    }

    assert_eq!(session.local_player_hero, Some(HeroId(1)));
    assert_eq!(session.owned_items_owner, Some(HeroId(1)));
    assert!(
        session.active_build_owned_items().contains(&wand_id),
        "repeated same-hero identity without items must preserve prior items-only update in queue: {:?}",
        session.active_build_owned_items()
    );
}

#[test]
fn gsi_overflow_reconcile_emits_hero_identity_before_fresh_inventory() {
    use truesight_adapters::gsi::{BoundedDraftQueue, GSI_QUEUE_CAPACITY, parse_gsi_json_events};
    use truesight_core::domain::HeroId;
    use truesight_core::ports::DraftEvent;

    let mut session = AppSession::new().unwrap();
    let wand_id = session
        .item_repo
        .item_by_slug("magic_wand")
        .expect("magic_wand must exist in catalog")
        .id;
    let queue = BoundedDraftQueue::new(GSI_QUEUE_CAPACITY);

    // 1. Confirm Anti-Mage first and drain.
    let am_confirm = r#"{
        "player": {},
        "hero": { "name": "npc_dota_hero_antimage" }
    }"#;
    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(am_confirm)));
    for ev in queue.drain_and_reconcile_gsi() {
        session.apply_event(ev);
    }
    assert_eq!(session.local_player_hero, Some(HeroId(1)));

    // 2. Queue Nyx + Magic Wand (2 events), fill remaining 14 slots with bans, then send critical FactionDetected
    // to evict Nyx's identity from index 0 while leaving Magic Wand queued.
    let nyx_with_wand = r#"{
        "player": {},
        "hero": { "name": "npc_dota_hero_nyx_assassin" },
        "items": { "slot0": { "name": "item_magic_wand" } }
    }"#;
    let fourteen_bans = r#"{
        "draft": {
            "team2": { "ban0_id": 3, "ban1_id": 4, "ban2_id": 6, "ban3_id": 7, "ban4_id": 8, "ban5_id": 9, "ban6_id": 10 },
            "team3": { "ban0_id": 11, "ban1_id": 12, "ban2_id": 14, "ban3_id": 15, "ban4_id": 16, "ban5_id": 18, "ban6_id": 19 }
        }
    }"#;
    let critical_faction = r#"{
        "player": { "team_name": "dire" }
    }"#;
    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(nyx_with_wand)));
    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(fourteen_bans)));
    assert!(queue.record_and_push_gsi_events(parse_gsi_json_events(critical_faction)));

    let reconciled = queue.drain_and_reconcile_gsi();
    let id_pos = reconciled
        .iter()
        .position(|e| {
            matches!(
                e,
                DraftEvent::LocalPlayerHeroDetected { hero_slug }
                    if hero_slug == "npc_dota_hero_nyx_assassin"
            )
        })
        .expect("reconciled batch must include Nyx identity");
    let inv_pos = reconciled
        .iter()
        .position(|e| {
            matches!(
                e,
                DraftEvent::InventoryUpdated {
                    hero_slug: Some(slug),
                    ..
                } if slug == "npc_dota_hero_nyx_assassin"
            )
        })
        .expect("reconciled batch must include Nyx inventory");
    assert!(
        id_pos < inv_pos,
        "reconciled batch must emit Nyx identity ({id_pos}) before Nyx inventory ({inv_pos}): {reconciled:?}"
    );

    for ev in reconciled {
        session.apply_event(ev);
    }
    assert_eq!(session.local_player_hero, Some(HeroId(88)));
    assert_eq!(session.owned_items_owner, Some(HeroId(88)));
    assert!(
        session.active_build_owned_items().contains(&wand_id),
        "Nyx must retain Magic Wand after overflow reconciliation: {:?}",
        session.active_build_owned_items()
    );
}

#[test]
fn sync_target_item_heroes_tracks_role_pairs_and_refreshes_on_role_change() {
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{HeroId, Role, Side};

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_role_swap_refresh_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();
    std::fs::write(
        tmp_dir.join("hero_item_stats.json"),
        r#"[{"heroId":32,"role":"Mid","bracket":"HERALD_GUARDIAN","itemStartingPurchase":[],"itemFullPurchase":[{"itemId":174,"phase":"Core","timeSecs":1080,"instance":0,"matchCount":1000,"winCount":520}]},{"heroId":32,"role":"Mid","bracket":"CRUSADER_ARCHON","itemStartingPurchase":[],"itemFullPurchase":[{"itemId":174,"phase":"Core","timeSecs":960,"instance":0,"matchCount":1000,"winCount":540}]},{"heroId":32,"role":"Mid","bracket":"LEGEND_ANCIENT","itemStartingPurchase":[],"itemFullPurchase":[{"itemId":174,"phase":"Core","timeSecs":900,"instance":0,"matchCount":1000,"winCount":550}]},{"heroId":32,"role":"Mid","bracket":"DIVINE_IMMORTAL","itemStartingPurchase":[],"itemFullPurchase":[{"itemId":174,"phase":"Core","timeSecs":840,"instance":0,"matchCount":1000,"winCount":570}]}]"#,
    )
    .unwrap();

    let token = "eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string();
    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.stratz_api_token = Some(token.clone());
    let mut session =
        AppSession::with_cache_and_config(mgr, Some(tmp_dir.join("config.toml"))).unwrap();
    session.cache_mgr.stratz_api_token = Some(token);

    assert!(execute_command(&mut session, "ally 2 riki").unwrap());
    assert!(
        session
            .cache_mgr
            .target_item_hero_roles
            .contains(&(HeroId(32), Role::Mid)),
        "ally 2 Riki must track (HeroId(32), Role::Mid) in cache_mgr: {:?}",
        session.cache_mgr.target_item_hero_roles
    );
    assert!(session.cache_mgr.target_item_hero_ids.contains(&HeroId(32)));

    session.toggle_post_draft_build_view();
    assert!(session.is_post_draft_build_view());

    // Wait for any initial background check to settle cleanly.
    wait_for_sync_idle(&mut session);

    // Swapping Riki from Pos 2 (Mid) -> Pos 1 (Carry) changes active_build_hero().role
    // and must trigger background refresh for uncached Role::Carry.
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_bg = std::sync::Arc::clone(&calls);
    session.set_background_sync_runner(move |_mgr, _force| {
        calls_bg.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("ok".into())
    });

    session.swap_slots(Side::Allies, 1, 0);
    assert!(
        session
            .cache_mgr
            .target_item_hero_roles
            .contains(&(HeroId(32), Role::Carry)),
        "swap_slots must update target_item_hero_roles to Carry: {:?}",
        session.cache_mgr.target_item_hero_roles
    );
    assert!(
        calls.load(std::sync::atomic::Ordering::SeqCst) >= 1 || session.sync_in_flight,
        "changing active build hero role from Mid -> Carry must trigger uncached item refresh"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn select_build_variant_for_direct_upgrade_preserves_owned_scepter_and_next_pointer() {
    use truesight_adapters::item_repo::SeededItemMetaRepository;
    use truesight_core::domain::{HeroId, HeroItemStat, ItemCatalogEntry, ItemId, ItemPhase};
    use truesight_core::ports::DraftEvent;

    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 5 crystal maiden").unwrap());
    session.local_player_hero = Some(HeroId(5));
    session.item_repo = SeededItemMetaRepository {
        custom_catalog: vec![
            ItemCatalogEntry {
                id: ItemId(214),
                slug: "tranquil_boots".into(),
                display_name: "Tranquil Boots".into(),
                cost: 900,
                components: vec![],
                is_recipe: false,
                is_consumable_drop: false,
            },
            ItemCatalogEntry {
                id: ItemId(180),
                slug: "arcane_boots".into(),
                display_name: "Arcane Boots".into(),
                cost: 1400,
                components: vec![],
                is_recipe: false,
                is_consumable_drop: false,
            },
            ItemCatalogEntry {
                id: ItemId(108),
                slug: "ultimate_scepter".into(),
                display_name: "Aghanim's Scepter".into(),
                cost: 4200,
                components: vec![ItemId(60), ItemId(23), ItemId(21), ItemId(22)],
                is_recipe: false,
                is_consumable_drop: false,
            },
            ItemCatalogEntry {
                id: ItemId(271),
                slug: "ultimate_scepter_2".into(),
                display_name: "Aghanim's Blessing".into(),
                cost: 5800,
                components: vec![ItemId(108)],
                is_recipe: false,
                is_consumable_drop: false,
            },
            ItemCatalogEntry {
                id: ItemId(256),
                slug: "aeon_disk".into(),
                display_name: "Aeon Disk".into(),
                cost: 3000,
                components: vec![],
                is_recipe: false,
                is_consumable_drop: false,
            },
        ],
        custom_hero_stats: vec![(
            HeroId(5),
            None,
            None,
            vec![
                HeroItemStat {
                    item_id: ItemId(214),
                    phase: ItemPhase::Early,
                    avg_time_mins: 8.0,
                    match_count: 1800,
                    win_count: 940,
                },
                HeroItemStat {
                    item_id: ItemId(180),
                    phase: ItemPhase::Early,
                    avg_time_mins: 9.0,
                    match_count: 1600,
                    win_count: 830,
                },
                HeroItemStat {
                    item_id: ItemId(108),
                    phase: ItemPhase::Late,
                    avg_time_mins: 32.0,
                    match_count: 1000,
                    win_count: 520,
                },
                HeroItemStat {
                    item_id: ItemId(271),
                    phase: ItemPhase::Late,
                    avg_time_mins: 48.0,
                    match_count: 300,
                    win_count: 222,
                },
                HeroItemStat {
                    item_id: ItemId(256),
                    phase: ItemPhase::Late,
                    avg_time_mins: 36.0,
                    match_count: 900,
                    win_count: 500,
                },
            ],
        )],
        ..Default::default()
    };

    session.apply_event(DraftEvent::InventoryUpdated {
        hero_slug: Some("npc_dota_hero_crystal_maiden".into()),
        item_slugs: vec!["item_tranquil_boots".into(), "item_ultimate_scepter".into()],
    });

    let initial = session.compute_recommended_build().unwrap();
    let scepter_idx = initial
        .steps
        .iter()
        .position(|s| s.item_id == ItemId(108))
        .expect("Scepter (108) must be primary");
    assert!(initial.steps[scepter_idx].owned);
    assert_eq!(
        initial.steps[scepter_idx]
            .variants
            .first()
            .map(|v| v.item_id),
        Some(ItemId(271))
    );
    assert_eq!(
        initial
            .next_unowned_index
            .map(|idx| initial.steps[idx].item_id),
        Some(ItemId(256)),
        "NEXT must initially point to unowned Aeon Disk (256)"
    );

    // Invoking direct-upgrade selection by item ID or step index must be a no-op so owned Scepter
    // stays intact and NEXT remains on Aeon Disk.
    session.select_build_variant_for_item(ItemId(108), ItemId(271));
    session.select_build_variant(scepter_idx, ItemId(271));

    let after_upgrade_click = session.compute_recommended_build().unwrap();
    let scepter_after = after_upgrade_click
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(108))
        .expect("owned Scepter card must NOT be replaced by unowned Blessing");
    assert!(scepter_after.owned);
    assert_eq!(
        scepter_after.variants.first().map(|v| v.item_id),
        Some(ItemId(271)),
        "Blessing (271) must stay in its child upgrade position under Scepter"
    );
    assert_eq!(
        after_upgrade_click
            .next_unowned_index
            .map(|idx| after_upgrade_click.steps[idx].item_id),
        Some(ItemId(256)),
        "NEXT must remain on Aeon Disk (256), not move onto Blessing"
    );

    // Ordinary non-upgrade alternatives (e.g. Arcane Boots under Tranquil Boots) must still swap cleanly.
    session.select_build_variant_for_item(ItemId(214), ItemId(180));
    let swapped = session.compute_recommended_build().unwrap();
    let arcane_step = swapped
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(180))
        .expect("ordinary alternative Arcane Boots (180) must still swap into primary slot");
    assert!(
        arcane_step
            .variants
            .iter()
            .any(|v| v.item_id == ItemId(214)),
        "Tranquil Boots (214) should move to variants after swapping to Arcane Boots"
    );

    // Clicking the original primary item restores Tranquil Boots (214).
    session.select_build_variant_for_item(ItemId(180), ItemId(214));
    let restored = session.compute_recommended_build().unwrap();
    assert!(
        restored.steps.iter().any(|s| s.item_id == ItemId(214)),
        "switching back to Tranquil Boots (214) must restore original primary"
    );
}

#[test]
fn set_ally_faction_clears_stale_build_view_hero_and_refreshes_uncached_team_items() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use truesight_adapters::sync::LocalCacheManager;
    use truesight_core::domain::{Faction, HeroId, Role};

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_faction_flip_refresh_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Cache TA (46, Carry) and Hoodwink (123, SoftSupport) across all 4 STRATZ buckets;
    // leave Dire heroes Juggernaut (8, Carry) and Invoker (74, SoftSupport) uncached.
    let make_hero_entries = |hero_id: u16, role: &str, item_id: u16| {
        [
            "HERALD_GUARDIAN",
            "CRUSADER_ARCHON",
            "LEGEND_ANCIENT",
            "DIVINE_IMMORTAL",
        ]
        .into_iter()
        .map(|b| {
            format!(
                r#"{{"heroId":{hero_id},"role":"{role}","bracket":"{b}","itemStartingPurchase":[],"itemFullPurchase":[{{"itemId":{item_id},"phase":"Core","timeSecs":960,"instance":0,"matchCount":1000,"winCount":540}}]}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",")
    };
    let cached_json = format!(
        "[{},{}]",
        make_hero_entries(46, "Carry", 236),
        make_hero_entries(123, "SoftSupport", 232)
    );
    std::fs::write(tmp_dir.join("hero_item_stats.json"), cached_json).unwrap();

    let token = "eyJhbGciOinone.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string();
    let mut mgr = LocalCacheManager::with_optional_account(tmp_dir.clone(), None);
    mgr.stratz_api_token = Some(token.clone());
    let mut session =
        AppSession::with_cache_and_config(mgr, Some(tmp_dir.join("config.toml"))).unwrap();
    session.cache_mgr.stratz_api_token = Some(token);

    assert!(execute_command(&mut session, "ally 1 templar assassin").unwrap());
    assert!(execute_command(&mut session, "ally 4 hoodwink").unwrap());
    assert!(execute_command(&mut session, "enemy 1 juggernaut").unwrap());
    assert!(execute_command(&mut session, "enemy 4 invoker").unwrap());

    session.select_build_view_hero(HeroId(123)); // Explicitly inspect ally Hoodwink on Radiant.
    session.toggle_post_draft_build_view();
    assert!(session.is_post_draft_build_view());
    wait_for_sync_idle(&mut session);

    type RecordedRolesLog = Arc<Mutex<Vec<Vec<(HeroId, Role)>>>>;
    let calls = Arc::new(AtomicUsize::new(0));
    let recorded_roles: RecordedRolesLog = Arc::new(Mutex::new(Vec::new()));
    let calls_bg = Arc::clone(&calls);
    let recorded_bg = Arc::clone(&recorded_roles);
    session.set_background_sync_runner(move |mgr, _force| {
        calls_bg.fetch_add(1, Ordering::SeqCst);
        recorded_bg
            .lock()
            .unwrap()
            .push(mgr.target_item_hero_roles.clone());
        Ok("ok".into())
    });

    // Flipping faction Radiant -> Dire swaps allies/enemies:
    // Hoodwink moved to enemies so `build_view_hero` must clear, target_item_hero_roles
    // must switch to Dire allies (Juggernaut + Invoker), and an item refresh must launch.
    session.set_ally_faction(Faction::Dire);

    assert_eq!(
        session.build_view_hero, None,
        "stale build_view_hero moved to Enemies must clear on faction flip"
    );
    assert!(
        session
            .cache_mgr
            .target_item_hero_roles
            .contains(&(HeroId(8), Role::Carry))
            && session
                .cache_mgr
                .target_item_hero_roles
                .contains(&(HeroId(74), Role::SoftSupport))
            && !session
                .cache_mgr
                .target_item_hero_roles
                .iter()
                .any(|&(h, _)| h == HeroId(123)),
        "target_item_hero_roles must update to Dire allies after faction flip: {:?}",
        session.cache_mgr.target_item_hero_roles
    );

    wait_for_sync_until(&mut session, || calls.load(Ordering::SeqCst) >= 1);
    let batches = recorded_roles.lock().unwrap();
    assert!(
        batches
            .iter()
            .any(|b| b.contains(&(HeroId(74), Role::SoftSupport))),
        "faction flip into Build view must trigger sync for uncached Dire heroes: {batches:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
