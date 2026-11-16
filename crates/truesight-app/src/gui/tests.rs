use super::*;
use crate::execute_command;
use eframe::egui::pos2;
use truesight_adapters::vision::parse_hero_atlas;
use truesight_core::domain::{HeroId, Role};

#[test]
fn embedded_hero_atlas_decodes_all_127_heroes_and_gem_icon() {
    let session = AppSession::new().unwrap();
    let atlas = parse_hero_atlas().expect("embedded hero atlas must decode");
    assert_eq!(atlas.gem_width, 32);
    assert_eq!(atlas.gem_height, 24);
    assert_eq!(atlas.gem_rgb.len(), 32 * 24 * 3);
    assert_eq!(atlas.hero_width, 64);
    assert_eq!(atlas.hero_height, 36);
    assert_eq!(atlas.heroes.len(), 127);
    for hero in session.matrix.heroes() {
        let rgb = atlas
            .heroes
            .get(&hero.id)
            .unwrap_or_else(|| panic!("missing embedded portrait for {:?}", hero.id));
        assert_eq!(rgb.len(), 64 * 36 * 3);
    }
}

#[test]
fn v4_slot_breakdowns_matrix_averages_and_comp_diagnostics_match_engine() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
    assert!(execute_command(&mut session, "ally 3 magnus").unwrap());
    assert!(execute_command(&mut session, "ally 4 shadow demon").unwrap());
    assert!(execute_command(&mut session, "ally 5 crystal maiden").unwrap());
    assert!(execute_command(&mut session, "enemy 1 medusa").unwrap());
    assert!(execute_command(&mut session, "enemy 2 storm spirit").unwrap());
    assert!(execute_command(&mut session, "enemy 3 axe").unwrap());
    assert!(execute_command(&mut session, "enemy 4 tusk").unwrap());
    assert!(execute_command(&mut session, "enemy 5 witch doctor").unwrap());

    let (analysis, _) = session.evaluate();

    for i in [0usize, 2, 3, 4] {
        let b = compute_slot_breakdown(&session, &analysis, Side::Allies, i).unwrap();
        let net = analysis.ally_slot_net_delta[i].unwrap();
        assert!(
            ((b.meta + b.cnt + b.syn) - net).abs() < 1e-6,
            "Ally slot {i} breakdown must sum to ally_slot_net_delta"
        );
        assert!((b.cnt - analysis.ally_counter_avg[i].unwrap()).abs() < 1e-6);
    }

    for j in 0..TEAM_SIZE {
        let b = compute_slot_breakdown(&session, &analysis, Side::Enemies, j).unwrap();
        let net = analysis.enemy_slot_net_delta[j].unwrap();
        assert!(
            ((b.meta + b.cnt + b.syn) - net).abs() < 1e-6,
            "Enemy slot {j} breakdown must sum to enemy_slot_net_delta"
        );
        let col_avg = enemy_counter_col_avg(&analysis, j).unwrap();
        assert!(
            (col_avg + b.cnt).abs() < 1e-6,
            "Matrix bottom AVG row must be opposite sign of enemy perspective cnt"
        );
    }

    assert_eq!(fmt_val(0.0), "±0.0");
    assert_eq!(fmt_val(-0.02), "±0.0");
    assert_eq!(fmt_pct(0.01), "±0.0%");
    assert_eq!(tone_for_user(1.5, -1.0), Tone::Bad);
    assert_eq!(tone_for_user(-0.6, -1.0), Tone::Good);
    assert_eq!(tone_for_user(1.5, 1.0), Tone::Good);

    let ally_comp = compute_comp_diagnostic(&session.draft.allies, &session.matrix);
    let enemy_comp = compute_comp_diagnostic(&session.draft.enemies, &session.matrix);
    assert!(
        ((ally_comp.score - enemy_comp.score) - analysis.total_composition_edge).abs() < 1e-6,
        "COMP tab score difference must match total_composition_edge"
    );
}

fn make_5v5_gui_session() -> AppSession {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
    assert!(execute_command(&mut session, "ally 2 crystal maiden").unwrap());
    assert!(execute_command(&mut session, "ally 3 shadow demon").unwrap());
    assert!(execute_command(&mut session, "ally 4 magnus").unwrap());
    assert!(execute_command(&mut session, "ally 5 jakiro").unwrap());
    assert!(execute_command(&mut session, "enemy 1 medusa").unwrap());
    assert!(execute_command(&mut session, "enemy 2 storm spirit").unwrap());
    assert!(execute_command(&mut session, "enemy 3 axe").unwrap());
    assert!(execute_command(&mut session, "enemy 4 tusk").unwrap());
    assert!(execute_command(&mut session, "enemy 5 witch doctor").unwrap());
    session.local_player_hero = session.draft.allies[2].map(|p| p.hero_id);
    session.draft.target_role = Role::Mid;
    session
}

fn render_at_viewport(
    app: &mut TrueSightGuiApp,
    ctx: &egui::Context,
    viewport: Vec2,
) -> LayoutRects {
    let raw_input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
        ..Default::default()
    };
    let mut layout = None;
    for _ in 0..2 {
        let _ = ctx.run(raw_input.clone(), |ctx| {
            layout = Some(app.render_frame(ctx));
        });
    }
    layout.expect("layout rects captured")
}

#[test]
fn v4_layout_rects_fit_viewport_and_align_cards() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
    assert!(execute_command(&mut session, "ally 3 magnus").unwrap());
    assert!(execute_command(&mut session, "ally 4 shadow demon").unwrap());
    assert!(execute_command(&mut session, "ally 5 crystal maiden").unwrap());
    assert!(execute_command(&mut session, "enemy 1 medusa").unwrap());
    assert!(execute_command(&mut session, "enemy 2 storm spirit").unwrap());
    assert!(execute_command(&mut session, "enemy 3 axe").unwrap());
    assert!(execute_command(&mut session, "enemy 4 tusk").unwrap());
    assert!(execute_command(&mut session, "enemy 5 witch doctor").unwrap());
    session.draft.target_role = Role::Mid;
    session.set_vision_availability(&truesight_adapters::vision::CaptureAvailability::Unavailable {
        backend: truesight_adapters::vision::CaptureBackendKind::WindowsNative,
        reason: "Native Windows capture backend is not implemented yet (#12); manual pick mode & GSI remain active.".into(),
    });

    let mut app = TrueSightGuiApp::new(session, None, None);
    assert!(
        app.status_banner.contains("#12"),
        "Unavailable capture must explain fallback in banner"
    );
    app.picker_target = Some(PickerTarget::Slot(Side::Allies, 1));

    for viewport in [
        vec2(1433.0, 952.0),
        vec2(1468.0, 960.0),
        vec2(2560.0, 1440.0),
    ] {
        let ctx = egui::Context::default();
        for tab in [MatrixTab::Counter, MatrixTab::Synergy, MatrixTab::Comp] {
            app.matrix_tab = tab;
            let r = render_at_viewport(&mut app, &ctx, viewport);

            assert!(
                r.top_bar.height() > 24.0
                    && r.top_bar.height() < 65.0
                    && r.top_bar.max.x <= viewport.x,
                "viewport={viewport:?} tab={tab:?} top_bar out of range: {:?}",
                r.top_bar
            );
            assert!(r.allies.min.y > r.top_bar.max.y);
            assert!(
                (r.allies.min.y - r.oracle.min.y).abs() <= 1.5,
                "viewport={viewport:?} tab={tab:?} allies={:?} oracle={:?}",
                r.allies,
                r.oracle
            );
            assert!(
                (r.allies.min.y - r.enemies.min.y).abs() <= 1.5,
                "viewport={viewport:?} tab={tab:?} allies={:?} enemies={:?}",
                r.allies,
                r.enemies
            );
            assert!(
                (r.allies.max.y - r.oracle.max.y).abs() <= 2.0,
                "viewport={viewport:?} tab={tab:?} allies={:?} oracle={:?}",
                r.allies,
                r.oracle
            );
            assert!(
                (r.allies.max.y - r.enemies.max.y).abs() <= 2.0,
                "viewport={viewport:?} tab={tab:?} allies={:?} enemies={:?}",
                r.allies,
                r.enemies
            );
            assert!(
                r.enemies.max.x <= viewport.x,
                "viewport={viewport:?} tab={tab:?} enemies overflowed right: {:?}",
                r.enemies
            );

            assert!(r.suggested.min.y > r.allies.max.y);
            assert!(
                (r.suggested.min.y - r.matrix.min.y).abs() <= 1.5,
                "viewport={viewport:?} tab={tab:?} suggested={:?} matrix={:?}",
                r.suggested,
                r.matrix
            );
            assert!(
                (r.suggested.max.y - r.matrix.max.y).abs() <= 2.0,
                "viewport={viewport:?} tab={tab:?} suggested={:?} matrix={:?}",
                r.suggested,
                r.matrix
            );
            assert!(
                r.matrix.max.x <= viewport.x && r.matrix.max.y <= viewport.y,
                "viewport={viewport:?} tab={tab:?} bottom overflow: {:?}",
                r.matrix
            );
        }
    }
}

#[test]
fn v4_slot_internal_geometry_prevents_badge_percentage_and_clear_overlap() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    let ctx = egui::Context::default();
    let viewport = vec2(1433.0, 952.0);
    let r = render_at_viewport(&mut app, &ctx, viewport);

    for (side_name, card_rect, slots) in [
        ("allies", r.allies, &r.ally_slots),
        ("enemies", r.enemies, &r.enemy_slots),
    ] {
        for i in 0..TEAM_SIZE {
            let s = slots[i];
            let ratio = s.portrait.width() / s.portrait.height().max(1.0);
            assert!(
                (ratio - 16.0 / 9.0).abs() < 0.05,
                "{side_name} slot {i} portrait aspect ratio {ratio} != 16:9 ({:?})",
                s.portrait
            );
            assert!(
                (s.row.height() - 56.0).abs() <= 1.0,
                "{side_name} slot {i} height={} != 56.0",
                s.row.height()
            );
            if i > 0 {
                let gap = s.row.min.y - slots[i - 1].row.max.y;
                assert!(
                    (gap - 8.0).abs() <= 1.5,
                    "{side_name} slot {i} vertical gap={gap} != 8.0"
                );
            }
            assert!(
                s.portrait.max.x < s.mid.min.x,
                "{side_name} slot {i} portrait overlapped mid: {:?} vs {:?}",
                s.portrait,
                s.mid
            );
            assert!(
                s.mid.max.x < s.net.min.x,
                "{side_name} slot {i} mid overlapped net: {:?} vs {:?}",
                s.mid,
                s.net
            );
            let clear_rect = s
                .clear
                .unwrap_or_else(|| panic!("{side_name} slot {i} missing clear button rect"));
            assert!(
                s.net.max.x < clear_rect.min.x,
                "{side_name} slot {i} net overlapped clear: {:?} vs {:?}",
                s.net,
                clear_rect
            );
            assert!(
                clear_rect.max.x <= s.row.max.x,
                "{side_name} slot {i} clear exceeded row"
            );
        }
        let bottom_void = card_rect.max.y - slots[TEAM_SIZE - 1].row.max.y;
        assert!(
            bottom_void <= 16.0,
            "{side_name} bottom empty void too large: {bottom_void}px"
        );
    }

    // Simulate a real pointer click on Ally P1's clear button (`×`)
    let clear_pos = r.ally_slots[0].clear.unwrap().center();
    for pressed in [true, false] {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
            events: vec![
                egui::Event::PointerMoved(clear_pos),
                egui::Event::PointerButton {
                    pos: clear_pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            app.render_frame(ctx);
        });
    }
    assert!(
        app.session.draft.allies[0].is_none(),
        "clicking Ally P1 clear button rect {:?} did not clear slot",
        clear_pos
    );
    assert!(app.picker_target.is_none());
}

#[test]
fn v4_matrix_square_aspect_ratio_and_suggested_table_fill() {
    for viewport in [
        vec2(1433.0, 952.0),
        vec2(1468.0, 960.0),
        vec2(2560.0, 1440.0),
    ] {
        let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
        app.matrix_tab = MatrixTab::Counter;
        let ctx = egui::Context::default();
        let r = render_at_viewport(&mut app, &ctx, viewport);

        assert!(
            (r.matrix_cell_size.x - r.matrix_cell_size.y).abs() <= 0.5,
            "viewport={viewport:?} matrix cells not square: {:?}",
            r.matrix_cell_size
        );
        assert!(
            (r.matrix_h_gap - r.matrix_v_gap).abs() <= 2.0,
            "viewport={viewport:?} matrix gaps asymmetric: h={} v={}",
            r.matrix_h_gap,
            r.matrix_v_gap
        );
        assert!(
            r.matrix_bottom_y <= r.matrix.max.y - 6.0,
            "viewport={viewport:?} matrix caption clipped/touching border: bottom_y={} card_max_y={}",
            r.matrix_bottom_y,
            r.matrix.max.y
        );
        assert!(
            r.suggested_last_col_max_x >= r.suggested.max.x - 36.0
                && r.suggested_last_col_max_x <= r.suggested.max.x - 6.0,
            "viewport={viewport:?} suggested table last_x={} vs card max_x={}",
            r.suggested_last_col_max_x,
            r.suggested.max.x
        );
    }
}

#[test]
fn v4_manual_locks_swap_and_clear_preserve_card_alignment() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
    assert!(execute_command(&mut session, "ally 2 crystal maiden").unwrap());
    assert!(execute_command(&mut session, "ally 3 night stalker").unwrap());
    assert!(execute_command(&mut session, "ally 4 shadow demon").unwrap());
    assert!(execute_command(&mut session, "ally 5 jakiro").unwrap());
    assert!(execute_command(&mut session, "enemy 1 medusa").unwrap());
    assert!(execute_command(&mut session, "enemy 2 storm spirit").unwrap());
    assert!(execute_command(&mut session, "enemy 3 beastmaster").unwrap());
    assert!(execute_command(&mut session, "enemy 4 spirit breaker").unwrap());
    assert!(execute_command(&mut session, "enemy 5 witch doctor").unwrap());

    session.local_player_hero = session.draft.allies[2].map(|p| p.hero_id);
    session.swap_slots(Side::Allies, 2, 1);
    assert!(session.ally_locked_slots[1]);
    assert!(session.ally_locked_slots[2]);

    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();
    let r = render_at_viewport(&mut app, &ctx, vec2(1468.0, 960.0));
    assert!(
        (r.allies.max.y - r.oracle.max.y).abs() <= 2.0,
        "allies={:?} oracle={:?}",
        r.allies,
        r.oracle
    );
    assert!(
        (r.allies.max.y - r.enemies.max.y).abs() <= 2.0,
        "allies={:?} enemies={:?}",
        r.allies,
        r.enemies
    );

    app.session.clear_slot_manual(Side::Allies, 1);
    assert!(app.session.draft.allies[1].is_none());
    let r2 = render_at_viewport(&mut app, &ctx, vec2(1468.0, 960.0));
    assert!(
        (r2.allies.max.y - r2.oracle.max.y).abs() <= 2.0,
        "after clear: allies={:?} oracle={:?}",
        r2.allies,
        r2.oracle
    );
}

#[test]
fn v4_inline_picker_and_matrix_tabs_update_session_and_preserve_geometry() {
    let mut app = TrueSightGuiApp::new(AppSession::new().unwrap(), None, None);
    let ctx = egui::Context::default();

    let puck_id = HeroId(13);
    let am_id = HeroId(1);
    app.apply_picker_choice(PickerTarget::Slot(Side::Allies, 1), puck_id, "Puck");
    assert_eq!(
        app.session.draft.allies[1].map(|p| p.hero_id),
        Some(puck_id)
    );
    assert!(app.session.ally_locked_slots[1]);
    assert_eq!(app.picker_target, None);

    app.apply_picker_choice(PickerTarget::Ban, am_id, "Anti-Mage");
    assert!(app.session.draft.bans.contains(&Some(am_id)));

    for tab in [MatrixTab::Counter, MatrixTab::Synergy, MatrixTab::Comp] {
        app.matrix_tab = tab;
        let r = render_at_viewport(&mut app, &ctx, vec2(1468.0, 960.0));
        assert!(
            (r.allies.max.y - r.oracle.max.y).abs() <= 2.0,
            "tab={tab:?} allies={:?} oracle={:?}",
            r.allies,
            r.oracle
        );
        assert!(
            (r.suggested.max.y - r.matrix.max.y).abs() <= 2.0,
            "tab={tab:?} suggested={:?} matrix={:?}",
            r.suggested,
            r.matrix
        );
    }

    assert!(app.session.start_background_sync_with(false, |_, _| {
        Ok("Async background refresh ready.".into())
    }));
    for _ in 0..100 {
        let _ = render_at_viewport(&mut app, &ctx, vec2(1468.0, 960.0));
        if app.status_banner == "Async background refresh ready." {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(app.status_banner, "Async background refresh ready.");
}

#[test]
fn v4_enemy_scout_strip_renders_overlaid_portraits_switches_roles_and_yields_to_picker() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
    assert!(execute_command(&mut session, "ally 3 magnus").unwrap());
    assert!(execute_command(&mut session, "enemy 1 medusa").unwrap());
    assert!(execute_command(&mut session, "enemy 4 tusk").unwrap());

    let mut app = TrueSightGuiApp::new(session, None, None);
    assert_eq!(app.active_enemy_scout_role(), Role::Mid);

    let ctx = egui::Context::default();
    for viewport in [vec2(1433.0, 952.0), vec2(1468.0, 960.0)] {
        let r = render_at_viewport(&mut app, &ctx, viewport);
        assert!(
            (r.allies.max.y - r.oracle.max.y).abs() <= 2.0,
            "viewport={viewport:?} allies={:?} oracle={:?}",
            r.allies,
            r.oracle
        );
        assert!(
            r.enemy_scout_cards.len() >= 4,
            "viewport={viewport:?} expected >=4 enemy scout portrait cards, got {}",
            r.enemy_scout_cards.len()
        );
        for (idx, &card) in r.enemy_scout_cards.iter().enumerate() {
            assert!(
                r.oracle.contains_rect(card),
                "viewport={viewport:?} card {idx} {card:?} outside oracle {:?}",
                r.oracle
            );
            let ratio = card.width() / card.height().max(1.0);
            assert!(
                (ratio - 16.0 / 9.0).abs() < 0.08,
                "viewport={viewport:?} card {idx} aspect ratio {ratio} != 16:9 ({card:?})"
            );
            if idx > 0 {
                assert!(
                    r.enemy_scout_cards[idx - 1].max.x < card.min.x,
                    "viewport={viewport:?} cards {idx} overlap: {:?} vs {:?}",
                    r.enemy_scout_cards[idx - 1],
                    card
                );
            }
        }
    }

    app.enemy_scout_role = Some(Role::Offlane);
    assert_eq!(app.active_enemy_scout_role(), Role::Offlane);

    app.picker_target = Some(PickerTarget::Slot(Side::Allies, 1));
    let r_picking = render_at_viewport(&mut app, &ctx, vec2(1433.0, 952.0));
    assert!(
        r_picking.enemy_scout_cards.is_empty(),
        "Enemy scout strip must yield cleanly while inline manual picker is open"
    );
    assert!(
        (r_picking.allies.max.y - r_picking.oracle.max.y).abs() <= 2.0,
        "picking mode: allies={:?} oracle={:?}",
        r_picking.allies,
        r_picking.oracle
    );
}

#[test]
fn viewport_sets_low_res_bounds_wayland_app_id_and_gem_icon() {
    let icon = theme::build_gem_window_icon().expect("gem window icon must build from atlas");
    assert_eq!(icon.width, 64);
    assert_eq!(icon.height, 64);
    assert_eq!(icon.rgba.len(), 64 * 64 * 4);
    // Center region must contain bright Gem green channel energy rather than flat solid fill
    let center_green_max = icon.rgba.chunks_exact(4).map(|px| px[1]).max().unwrap_or(0);
    assert!(
        center_green_max > 140,
        "expected visible Gem green luminance, max green={center_green_max}"
    );

    let vb = build_viewport_builder();
    assert_eq!(vb.min_inner_size, Some(vec2(640.0, 440.0)));
    assert_eq!(vb.inner_size, Some(vec2(1180.0, 700.0)));
    assert_eq!(vb.app_id.as_deref(), Some("truesight-dota"));
    assert!(vb.icon.is_some(), "viewport must carry the Gem window icon");
}

#[test]
fn responsive_layout_fits_low_resolution_and_half_screen_viewports() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    for viewport in [vec2(960.0, 680.0), vec2(1024.0, 720.0), vec2(1280.0, 720.0)] {
        let ctx = egui::Context::default();
        for tab in [MatrixTab::Counter, MatrixTab::Synergy, MatrixTab::Comp] {
            app.matrix_tab = tab;
            let r = render_at_viewport(&mut app, &ctx, viewport);

            assert!(
                r.top_bar.max.x <= viewport.x && r.top_bar.height() < 80.0,
                "viewport={viewport:?} tab={tab:?} top_bar overflowed: {:?}",
                r.top_bar
            );
            assert!(
                r.top_bar.contains_rect(r.options_button),
                "viewport={viewport:?} OPTIONS button {:?} outside top_bar {:?}",
                r.options_button,
                r.top_bar
            );
            assert!(
                r.allies.max.x < r.oracle.min.x
                    && r.oracle.max.x < r.enemies.min.x
                    && r.enemies.max.x <= viewport.x,
                "viewport={viewport:?} tab={tab:?} top row overlap/overflow: allies={:?} oracle={:?} enemies={:?}",
                r.allies,
                r.oracle,
                r.enemies
            );
            assert!(
                (r.allies.max.y - r.oracle.max.y).abs() <= 2.0
                    && (r.allies.max.y - r.enemies.max.y).abs() <= 2.0,
                "viewport={viewport:?} tab={tab:?} top row misaligned",
            );
            assert!(
                r.suggested.max.x < r.matrix.min.x
                    && r.matrix.max.x <= viewport.x
                    && r.matrix.max.y <= viewport.y,
                "viewport={viewport:?} tab={tab:?} bottom row overflow: suggested={:?} matrix={:?}",
                r.suggested,
                r.matrix
            );
            assert!(
                (r.suggested.max.y - r.matrix.max.y).abs() <= 2.0,
                "viewport={viewport:?} tab={tab:?} bottom row height misaligned: suggested={:?} matrix={:?}",
                r.suggested,
                r.matrix
            );
            assert!(
                (r.matrix_cell_size.x - r.matrix_cell_size.y).abs() <= 0.5,
                "viewport={viewport:?} matrix cells not square: {:?}",
                r.matrix_cell_size
            );
            if tab != MatrixTab::Comp {
                assert!(
                    r.matrix_bottom_y <= r.matrix.max.y,
                    "viewport={viewport:?} tab={tab:?} matrix bottom {} exceeded {:?}",
                    r.matrix_bottom_y,
                    r.matrix
                );
            }
            for (side_name, slots) in [("allies", &r.ally_slots), ("enemies", &r.enemy_slots)] {
                for (i, s) in slots.iter().enumerate() {
                    let ratio = s.portrait.width() / s.portrait.height().max(1.0);
                    assert!(
                        (ratio - 16.0 / 9.0).abs() < 0.06,
                        "viewport={viewport:?} {side_name} P{} portrait ratio {ratio}",
                        i + 1
                    );
                    assert!(s.portrait.max.x < s.mid.min.x && s.mid.max.x < s.net.min.x);
                }
            }
        }
    }

    // Inline picker open at minimum viewport (960x680) must also preserve non-overlapping bounds
    app.picker_target = Some(PickerTarget::Slot(Side::Allies, 1));
    let ctx = egui::Context::default();
    let r_pick = render_at_viewport(&mut app, &ctx, vec2(960.0, 680.0));
    assert!(
        r_pick.allies.max.x < r_pick.oracle.min.x
            && r_pick.oracle.max.x < r_pick.enemies.min.x
            && r_pick.enemies.max.x <= 960.0,
        "960x680 picker top row overflowed: allies={:?} oracle={:?} enemies={:?}",
        r_pick.allies,
        r_pick.oracle,
        r_pick.enemies
    );
}

#[test]
fn top_bar_moves_dev_telemetry_and_actions_into_options_panel() {
    let mut app = TrueSightGuiApp::new(AppSession::new().unwrap(), None, None);
    let ctx = egui::Context::default();
    let viewport = vec2(960.0, 680.0);

    let r0 = render_at_viewport(&mut app, &ctx, viewport);
    assert!(!app.options_open);
    assert!(r0.options_window.is_none());
    assert!(r0.options_button.width() > 30.0);
    assert!(
        (r0.options_button.center().y - r0.reset_button.center().y).abs() <= 1.0
            && (r0.options_button.center().y - r0.bracket_bar.center().y).abs() <= 1.0
            && (r0.options_button.height() - r0.reset_button.height()).abs() <= 1.5,
        "OPTIONS/RESET/brackets misaligned: opts={:?} reset={:?} brackets={:?}",
        r0.options_button,
        r0.reset_button,
        r0.bracket_bar
    );
    assert!(
        r0.options_button.min.x - r0.bracket_bar.max.x >= 10.0,
        "OPTIONS too close to brackets: gap={}",
        r0.options_button.min.x - r0.bracket_bar.max.x
    );

    let click_pos = r0.options_button.center();
    for pressed in [true, false] {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
            events: vec![
                egui::Event::PointerMoved(click_pos),
                egui::Event::PointerButton {
                    pos: click_pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            app.render_frame(ctx);
        });
    }
    assert!(
        app.options_open,
        "clicking OPTIONS button must open OPTIONS panel"
    );

    let r1 = render_at_viewport(&mut app, &ctx, viewport);
    let win_rect = r1
        .options_window
        .expect("OPTIONS window rect must be reported");
    assert!(
        win_rect.max.x <= viewport.x && win_rect.max.y <= viewport.y,
        "OPTIONS window overflowed viewport: {win_rect:?}"
    );
    let demo_rect = r1
        .options_demo_button
        .expect("DEMO 5V5 button must live inside OPTIONS panel");
    let demo_pos = demo_rect.center();
    for pressed in [true, false] {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
            events: vec![
                egui::Event::PointerMoved(demo_pos),
                egui::Event::PointerButton {
                    pos: demo_pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            app.render_frame(ctx);
        });
    }
    assert!(
        app.session.draft.allies[0].is_some(),
        "DEMO 5V5 inside OPTIONS must populate demo draft"
    );
}

#[test]
fn pinned_pick_column_and_centered_matrix_hold_at_low_res() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    let ctx = egui::Context::default();
    let mut pick_w_960 = 0.0_f32;
    let mut pick_w_1468 = 0.0_f32;

    for viewport in [vec2(960.0, 680.0), vec2(1024.0, 720.0), vec2(1468.0, 960.0)] {
        for tab in [MatrixTab::Counter, MatrixTab::Synergy] {
            app.matrix_tab = tab;
            let r = render_at_viewport(&mut app, &ctx, viewport);
            assert!(
                r.suggested_last_col_max_x >= r.suggested.max.x - 36.0
                    && r.suggested_last_col_max_x <= r.suggested.max.x - 4.0,
                "viewport={viewport:?} PICK column must remain pinned at right edge of suggested card: last_x={} vs suggested={:?}",
                r.suggested_last_col_max_x,
                r.suggested
            );
            assert!(
                !r.suggested_row_rects.is_empty()
                    && r.suggested_row_rects.len() == r.suggested_pick_rects.len(),
                "viewport={viewport:?} row/pick rect mismatch"
            );
            for (idx, (row_r, pick_r)) in r
                .suggested_row_rects
                .iter()
                .zip(r.suggested_pick_rects.iter())
                .enumerate()
            {
                assert!(
                    (pick_r.center().y - row_r.center().y).abs() <= 1.2,
                    "viewport={viewport:?} row {idx} PICK misaligned: pick={:?} row={:?}",
                    pick_r,
                    row_r
                );
                assert!(
                    pick_r.height() <= 22.0,
                    "viewport={viewport:?} row {idx} PICK stretched vertically: {:?}",
                    pick_r
                );
            }
            if tab == MatrixTab::Counter && viewport == vec2(960.0, 680.0) {
                pick_w_960 = r.suggested_pick_rects[0].width();
                assert!(
                    r.suggested_comfort_hdr_max_x <= r.suggested_pick_rects[0].min.x,
                    "960x680 COMFORT column ({}) should scale to fit left of PICK ({})",
                    r.suggested_comfort_hdr_max_x,
                    r.suggested_pick_rects[0].min.x
                );
            }
            if tab == MatrixTab::Counter && viewport == vec2(1468.0, 960.0) {
                pick_w_1468 = r.suggested_pick_rects[0].width();
            }
            let left_pad = r.matrix_grid_rect.min.x - r.matrix.min.x;
            let right_pad = r.matrix.max.x - r.matrix_grid_rect.max.x;
            assert!(
                (left_pad - right_pad).abs() <= 3.0,
                "viewport={viewport:?} tab={tab:?} matrix grid not centered: left_pad={left_pad} right_pad={right_pad} grid={:?} card={:?}",
                r.matrix_grid_rect,
                r.matrix
            );
        }
    }
    assert!(
        pick_w_960 < pick_w_1468,
        "PICK button must scale down in compact viewport (960={pick_w_960}, 1468={pick_w_1468})"
    );
}

#[test]
fn tall_window_docks_symmetrical_composition_below_matrix() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    let ctx = egui::Context::default();

    let r_short = render_at_viewport(&mut app, &ctx, vec2(1468.0, 960.0));
    assert!(
        r_short.docked_comp.is_none(),
        "960px tall window should keep tabbed COMP instead of docking"
    );
    app.matrix_tab = MatrixTab::Comp;
    let r_comp = render_at_viewport(&mut app, &ctx, vec2(1468.0, 960.0));
    assert!(
        (r_comp.comp_enemies_hdr.max.x - (r_comp.matrix.max.x - 10.0)).abs() <= 6.0,
        "COMP ENEMIES header must align flush right: hdr={:?} card={:?}",
        r_comp.comp_enemies_hdr,
        r_comp.matrix
    );

    app.matrix_tab = MatrixTab::Counter;
    let r_tall = render_at_viewport(&mut app, &ctx, vec2(1440.0, 1320.0));
    let docked = r_tall
        .docked_comp
        .expect("tall 1440x1320 window must dock COMPOSITION below active matrix");
    assert!(
        docked.min.y > r_tall.matrix.min.y + 300.0
            && docked.max.x <= 1440.0
            && (docked.max.y - r_tall.suggested.max.y).abs() <= 2.0,
        "docked COMP bounds invalid: docked={docked:?} matrix={:?} suggested={:?}",
        r_tall.matrix,
        r_tall.suggested
    );
    assert!(
        (r_tall.comp_enemies_hdr.max.x - (docked.max.x - 10.0)).abs() <= 6.0,
        "docked COMP ENEMIES header must align flush right: hdr={:?} docked={docked:?}",
        r_tall.comp_enemies_hdr
    );
}

#[test]
fn dpi_scaled_low_resolution_viewport_uses_scrollable_fallback_without_overlap() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);

    // Non-scaled 960x680 must NOT trigger outer scrollable fallback.
    let ctx_1x = egui::Context::default();
    let r_full = render_at_viewport(&mut app, &ctx_1x, vec2(960.0, 680.0));
    assert!(!r_full.scrollable_fallback);

    // Simulate 720p @ 125% (1024x576 logical), 768p @ 150% (910x512 logical), and min_inner_size (640x440).
    for (ppp, logical_vp) in [
        (1.25_f32, vec2(1024.0, 576.0)),
        (1.5_f32, vec2(910.0, 512.0)),
        (1.5_f32, vec2(640.0, 440.0)),
    ] {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(ppp);
        let r = render_at_viewport(&mut app, &ctx, logical_vp);
        assert!(
            r.scrollable_fallback,
            "ppp={ppp} logical={logical_vp:?} must activate scrollable fallback"
        );
        assert!(
            r.allies.max.x < r.oracle.min.x
                && r.oracle.max.x < r.enemies.min.x
                && r.suggested.max.x < r.matrix.min.x,
            "ppp={ppp} logical={logical_vp:?} cards overlapped under fallback"
        );
        assert!(
            !r.suggested_row_rects.is_empty()
                && (r.suggested_pick_rects[0].center().y - r.suggested_row_rects[0].center().y)
                    .abs()
                    <= 1.2
        );
    }
}
