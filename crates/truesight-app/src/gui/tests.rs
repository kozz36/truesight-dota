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

fn render_at_viewport_with_output(
    app: &mut TrueSightGuiApp,
    ctx: &egui::Context,
    viewport: Vec2,
) -> (LayoutRects, egui::FullOutput) {
    let raw_input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
        ..Default::default()
    };
    let mut layout = None;
    let mut last_output = None;
    for _ in 0..2 {
        let output = ctx.run(raw_input.clone(), |ctx| {
            layout = Some(app.render_frame(ctx));
        });
        last_output = Some(output);
    }
    (
        layout.expect("layout rects captured"),
        last_output.expect("full output captured"),
    )
}

fn render_at_viewport(
    app: &mut TrueSightGuiApp,
    ctx: &egui::Context,
    viewport: Vec2,
) -> LayoutRects {
    render_at_viewport_with_output(app, ctx, viewport).0
}

fn collect_painted_rect_shapes(
    shape: &egui::epaint::Shape,
    out: &mut Vec<egui::epaint::RectShape>,
) {
    match shape {
        egui::epaint::Shape::Rect(rect_shape) => out.push(rect_shape.clone()),
        egui::epaint::Shape::Vec(shapes) => {
            for inner in shapes {
                collect_painted_rect_shapes(inner, out);
            }
        }
        _ => {}
    }
}

fn collect_painted_rects(shape: &egui::epaint::Shape, out: &mut Vec<(Rect, egui::Color32)>) {
    let mut shapes = Vec::new();
    collect_painted_rect_shapes(shape, &mut shapes);
    out.extend(shapes.into_iter().map(|s| (s.rect, s.fill)));
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

    // Simulate 720p @ 125% (1024x576 logical), 720p @ 150% (853x480 logical),
    // 768p @ 150% (910x512 logical), and min_inner_size (640x440).
    for (ppp, logical_vp) in [
        (1.25_f32, vec2(1024.0, 576.0)),
        (1.5_f32, vec2(853.0, 480.0)),
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
        if logical_vp.x >= 853.0 {
            assert!(
                r.top_bar.max.x <= logical_vp.x
                    && r.top_bar.contains_rect(r.options_button)
                    && r.top_bar.contains_rect(r.reset_button)
                    && r.enemies.max.x <= logical_vp.x
                    && r.matrix.max.x <= logical_vp.x,
                "ppp={ppp} logical={logical_vp:?} must fit horizontally without clipping: top={:?} enemies={:?} matrix={:?}",
                r.top_bar,
                r.enemies,
                r.matrix
            );
        }
        assert!(
            !r.suggested_row_rects.is_empty()
                && (r.suggested_pick_rects[0].center().y - r.suggested_row_rects[0].center().y)
                    .abs()
                    <= 1.2
        );
    }
}

#[test]
fn gui_account_link_apply_triggers_background_sync_and_updates_comfort() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use truesight_adapters::sync::{AccountLinkConfig, LocalCacheManager};
    use truesight_core::domain::HeroId;

    let tmp_dir = std::env::temp_dir().join(format!(
        "truesight_gui_account_sync_test_{}",
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
        assert!(!force, "GUI APPLY auto-sync must pass force=false");
        sync_calls_bg.fetch_add(1, Ordering::SeqCst);
        let acct = bg_mgr.player_account_id.expect("linked player account");
        let heroes_json =
            format!(r#"[{{"hero_id": 1, "last_played": {now}, "games": 30, "win": 24}}]"#);
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
        Ok(format!("Synced GUI account #{acct}"))
    });

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.account_input = "76561198061460213".into();
    assert_eq!(
        app.apply_account_input(),
        Some(AccountLinkConfig::Explicit(101_194_485))
    );
    assert_eq!(app.session.cache_mgr.player_account_id, Some(101_194_485));
    assert!(
        app.session.sync_in_flight,
        "GUI APPLY must trigger non-blocking player sync when cache is stale"
    );
    app.account_input = "101194485".into();
    assert_eq!(
        app.apply_account_input(),
        Some(AccountLinkConfig::Explicit(101_194_485))
    );
    assert!(app.session.sync_in_flight);
    assert!(app.status_banner.contains("syncing player cache"));

    let ctx = egui::Context::default();
    for _ in 0..100 {
        let _ = render_at_viewport(&mut app, &ctx, vec2(1280.0, 800.0));
        if !app.session.sync_in_flight {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!app.session.sync_in_flight);
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        app.session.matrix.hero(HeroId(1)).unwrap().personal_matches,
        30
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn responsive_900px_topbar_slot_headroom_and_non_overlapping_modals() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    let ctx = egui::Context::default();

    // 1. Live Windows 915x579 (125% DPI resize) and 900x660 minimum desktop viewport:
    //    top_bar must fit reset_button + options_button + bracket_bar without horizontal overflow.
    for vp in [vec2(900.0, 660.0), vec2(915.0, 579.0), vec2(960.0, 680.0)] {
        let r = render_at_viewport(&mut app, &ctx, vp);
        assert!(
            r.top_bar.max.x <= vp.x
                && r.top_bar.contains_rect(r.options_button)
                && r.top_bar.contains_rect(r.reset_button)
                && r.top_bar.contains_rect(r.bracket_bar),
            "vp={vp:?} top_bar clipped controls: top_bar={:?} opts={:?} reset={:?} brackets={:?}",
            r.top_bar,
            r.options_button,
            r.reset_button,
            r.bracket_bar
        );
        assert!(
            r.enemies.max.x <= vp.x && r.matrix.max.x <= vp.x,
            "vp={vp:?} workspace clipped horizontally: enemies={:?} matrix={:?}",
            r.enemies,
            r.matrix
        );
        // Slot mid_rect must have >= 46px height in compact mode and hold all 3 lines unclipped.
        for (i, s) in r.ally_slots.iter().chain(r.enemy_slots.iter()).enumerate() {
            assert!(
                s.mid.height() >= 46.0 && s.mid_content_bottom <= s.mid.max.y,
                "vp={vp:?} slot {i} mid_rect clipped 3-line telemetry: mid={:?} content_bottom={}",
                s.mid,
                s.mid_content_bottom
            );
        }
    }

    // 2. Normal-height viewport (1468x960) slot mid_rect headroom >= 52px and unclipped.
    let r_norm = render_at_viewport(&mut app, &ctx, vec2(1468.0, 960.0));
    for (i, s) in r_norm
        .ally_slots
        .iter()
        .chain(r_norm.enemy_slots.iter())
        .enumerate()
    {
        assert!(
            s.mid.height() >= 52.0 && s.mid_content_bottom <= s.mid.max.y,
            "1468x960 slot {i} mid_rect clipped 3-line telemetry: mid={:?} content_bottom={}",
            s.mid,
            s.mid_content_bottom
        );
    }

    // 3. Open OPTIONS (intentional top-right overlay with square HUD frame) + STEAM ACCOUNT LINK (bottom-left):
    // Verify both single-row (960x680) and two-row (860x680) top-bar layouts so OPTIONS sits cleanly below
    // top_bar without covering header controls, keeps square corners + palette, keeps DEMO 5V5 accessible,
    // and never collides with STEAM ACCOUNT LINK.
    app.options_open = true;
    app.account_editor_open = true;
    let mut one_row_bar_h = 0.0_f32;
    let mut two_row_bar_h = 0.0_f32;
    for (vp, is_two_row) in [(vec2(960.0, 680.0), false), (vec2(860.0, 680.0), true)] {
        let (r_modals, modals_out) = render_at_viewport_with_output(&mut app, &ctx, vp);
        if is_two_row {
            two_row_bar_h = r_modals.top_bar.height();
        } else {
            one_row_bar_h = r_modals.top_bar.height();
        }
        let opts_win = r_modals.options_window.expect("OPTIONS window open");
        let demo_btn = r_modals
            .options_demo_button
            .expect("DEMO 5V5 button must stay accessible inside OPTIONS");
        let acct_win = r_modals
            .account_window
            .expect("STEAM ACCOUNT LINK window open");
        assert!(
            opts_win.min.y >= r_modals.top_bar.max.y
                && opts_win.min.y <= r_modals.top_bar.max.y + 12.0
                && !opts_win.intersects(r_modals.top_bar)
                && opts_win.max.x >= vp.x - 24.0
                && opts_win.max.x <= vp.x,
            "vp={vp:?} OPTIONS window {:?} must anchor top-right below top_bar {:?}",
            opts_win,
            r_modals.top_bar
        );
        assert!(
            opts_win.contains_rect(demo_btn) && demo_btn.max.y <= vp.y,
            "vp={vp:?} DEMO 5V5 {:?} must remain accessible inside OPTIONS {:?} and viewport",
            demo_btn,
            opts_win
        );
        let mut modal_rect_shapes = Vec::new();
        for clipped in &modals_out.shapes {
            collect_painted_rect_shapes(&clipped.shape, &mut modal_rect_shapes);
        }
        assert!(
            modal_rect_shapes.iter().any(|s| {
                (s.rect.min - opts_win.min).length() <= 1.5
                    && (s.rect.max - opts_win.max).length() <= 1.5
                    && s.corner_radius == egui::CornerRadius::ZERO
                    && s.fill == P_INDIGO_1
                    && s.stroke == Stroke::new(1.0_f32, P_LINE)
            }),
            "vp={vp:?} OPTIONS window {:?} must render a square (CornerRadius::ZERO) frame with P_INDIGO_1 fill and P_LINE stroke",
            opts_win
        );
        assert!(
            modal_rect_shapes.iter().any(|s| {
                (s.rect.min - acct_win.min).length() <= 1.5
                    && (s.rect.max - acct_win.max).length() <= 1.5
                    && s.corner_radius == egui::CornerRadius::ZERO
                    && s.fill == P_INDIGO_1
                    && s.stroke == Stroke::new(1.0_f32, P_LINE)
            }),
            "vp={vp:?} STEAM ACCOUNT LINK window {:?} must render a square (CornerRadius::ZERO) frame with P_INDIGO_1 fill and P_LINE stroke",
            acct_win
        );
        assert!(
            (acct_win.min.y - opts_win.min.y).abs() <= 2.0
                && !acct_win.intersects(r_modals.top_bar)
                && acct_win.max.x <= opts_win.min.x - 4.0
                && acct_win.min.x >= 0.0,
            "vp={vp:?} STEAM ACCOUNT LINK {:?} must anchor beside OPTIONS {:?} below top_bar {:?}",
            acct_win,
            opts_win,
            r_modals.top_bar
        );
        assert!(
            !opts_win.intersects(acct_win),
            "vp={vp:?} OPTIONS {:?} and STEAM ACCOUNT LINK {:?} must not overlap",
            opts_win,
            acct_win
        );
    }
    // Narrow / min supported viewport bounds (640x480 and min_inner_size 640x440):
    // OPTIONS + STEAM ACCOUNT LINK never collide, and APPLY/CLOSE/DEMO 5V5 remain visible and reachable.
    for min_vp in [vec2(640.0, 480.0), vec2(640.0, 440.0)] {
        let r_narrow = render_at_viewport(&mut app, &ctx, min_vp);
        let opts_narrow = r_narrow.options_window.expect("OPTIONS open at min vp");
        let demo_narrow = r_narrow
            .options_demo_button
            .expect("DEMO 5V5 open at min vp");
        let acct_narrow = r_narrow
            .account_window
            .expect("STEAM ACCOUNT LINK open at min vp");
        let apply_btn = r_narrow
            .account_apply_button
            .expect("APPLY button must be rendered inside STEAM ACCOUNT LINK");
        let close_btn = r_narrow
            .account_close_button
            .expect("CLOSE button must be rendered inside STEAM ACCOUNT LINK");
        assert!(
            !opts_narrow.intersects(acct_narrow)
                && !opts_narrow.intersects(r_narrow.top_bar)
                && !acct_narrow.intersects(r_narrow.top_bar)
                && acct_narrow.min.x >= 0.0
                && acct_narrow.max.x <= min_vp.x
                && opts_narrow.max.y <= min_vp.y
                && acct_narrow.max.y <= min_vp.y,
            "vp={min_vp:?} OPTIONS {:?} and STEAM ACCOUNT LINK {:?} must not collide or overflow below top_bar {:?}",
            opts_narrow,
            acct_narrow,
            r_narrow.top_bar
        );
        assert!(
            acct_narrow.contains_rect(apply_btn)
                && acct_narrow.contains_rect(close_btn)
                && apply_btn.max.y <= min_vp.y
                && close_btn.max.y <= min_vp.y
                && close_btn.max.x <= min_vp.x
                && opts_narrow.contains_rect(demo_narrow)
                && demo_narrow.max.y <= min_vp.y,
            "vp={min_vp:?} buttons APPLY={apply_btn:?}, CLOSE={close_btn:?}, DEMO={demo_narrow:?} must stay reachable inside viewport"
        );
    }
    assert!(
        two_row_bar_h > one_row_bar_h + 10.0,
        "860x680 must exercise wrapped two-row top_bar ({two_row_bar_h} vs {one_row_bar_h})"
    );
}

#[test]
fn suggested_vs_enemies_cells_align_and_pick_column_follows_row_stripes() {
    let ctx = egui::Context::default();
    for mut app in [
        TrueSightGuiApp::new(AppSession::new().unwrap(), None, None),
        TrueSightGuiApp::new(make_5v5_gui_session(), None, None),
    ] {
        for viewport in [
            vec2(960.0, 680.0),
            vec2(1468.0, 960.0),
            vec2(1920.0, 1080.0),
        ] {
            let (r, full_output) = render_at_viewport_with_output(&mut app, &ctx, viewport);
            assert!(
                !r.suggested_row_rects.is_empty()
                    && r.suggested_row_rects.len() == r.suggested_vs_row_rects.len()
                    && r.suggested_row_rects.len() == r.suggested_pick_rects.len()
                    && r.suggested_row_rects.len() == r.suggested_pick_stripe_rects.len(),
                "viewport={viewport:?} suggested telemetry vector lengths mismatched"
            );

            let mut painted_rects = Vec::new();
            for clipped in &full_output.shapes {
                collect_painted_rects(&clipped.shape, &mut painted_rects);
            }
            let same_rect = |a: Rect, b: Rect, tol: f32| {
                (a.min - b.min).length() <= tol && (a.max - b.max).length() <= tol
            };

            let mut checked_visible_buttons = 0usize;
            for (idx, (((row_r, vs_r), pick_r), stripe_r)) in r
                .suggested_row_rects
                .iter()
                .zip(r.suggested_vs_row_rects.iter())
                .zip(r.suggested_pick_rects.iter())
                .zip(r.suggested_pick_stripe_rects.iter())
                .enumerate()
            {
                let pick_visible =
                    pick_r.min.y >= r.suggested.min.y && pick_r.max.y <= r.suggested.max.y;
                if pick_visible {
                    checked_visible_buttons += 1;
                }
                assert!(
                    (vs_r.center().y - row_r.center().y).abs() <= 1.0
                        && vs_r.min.y >= row_r.min.y - 0.5
                        && vs_r.max.y <= row_r.max.y + 0.5,
                    "viewport={viewport:?} row {idx} VS ENEMIES squares misaligned vertically: vs={:?} row={:?}",
                    vs_r,
                    row_r
                );
                if idx % 2 == 0 {
                    let s = stripe_r.unwrap_or_else(|| {
                        panic!(
                            "viewport={viewport:?} row {idx} PICK cell missing striped background"
                        )
                    });
                    assert!(
                        s.contains_rect(*pick_r)
                            && s.min.y <= row_r.min.y
                            && s.max.y >= row_r.max.y,
                        "viewport={viewport:?} row {idx} PICK stripe {:?} must cover PICK {:?} and row {:?}",
                        s,
                        pick_r,
                        row_r
                    );
                    assert!(
                        painted_rects
                            .iter()
                            .any(|&(rect, fill)| same_rect(rect, s, 0.5) && fill == P_INDIGO_2),
                        "viewport={viewport:?} row {idx} PICK stripe rect {:?} was not painted with P_INDIGO_2",
                        s
                    );
                    if pick_visible {
                        assert!(
                            painted_rects.iter().any(|&(rect, fill)| {
                                same_rect(rect, *pick_r, 1.5) && fill == P_INDIGO_2
                            }),
                            "viewport={viewport:?} row {idx} PICK button {:?} was not painted with P_INDIGO_2",
                            pick_r
                        );
                    }
                } else {
                    assert!(
                        stripe_r.is_none(),
                        "viewport={viewport:?} row {idx} PICK cell should stay unstriped: {:?}",
                        stripe_r
                    );
                    assert!(
                        !painted_rects.iter().any(|&(rect, fill)| {
                            fill == P_INDIGO_2
                                && rect.contains_rect(*pick_r)
                                && (rect.center().y - pick_r.center().y).abs() <= 2.0
                                && rect.height() <= row_r.height() + 12.0
                        }),
                        "viewport={viewport:?} row {idx} unstriped PICK row {:?} must not paint a P_INDIGO_2 stripe",
                        pick_r
                    );
                    if pick_visible {
                        assert!(
                            painted_rects.iter().any(|&(rect, fill)| {
                                same_rect(rect, *pick_r, 1.5) && fill == P_INDIGO_1
                            }),
                            "viewport={viewport:?} row {idx} PICK button {:?} was not painted with P_INDIGO_1",
                            pick_r
                        );
                    }
                }
            }
            assert!(
                checked_visible_buttons >= 4,
                "viewport={viewport:?} expected at least 4 visible PICK buttons inside {:?}",
                r.suggested
            );
        }
    }
}

#[test]
fn top_bar_renders_compact_vector_gear_and_info_button_without_overlap() {
    let mut app = TrueSightGuiApp::new(AppSession::new().unwrap(), None, None);
    app.onboarding_open = false;
    app.onboarding_dismissed = true;
    let ctx = egui::Context::default();

    for viewport in [vec2(960.0, 680.0), vec2(1180.0, 700.0)] {
        let r = render_at_viewport(&mut app, &ctx, viewport);
        assert!(
            (38.0..=44.0).contains(&r.options_button.width()),
            "viewport={viewport:?} gear button width must be compact (38..44px), got {}",
            r.options_button.width()
        );
        assert!(
            (20.0..=24.0).contains(&r.info_button.width()),
            "viewport={viewport:?} info button width must be ~22px, got {}",
            r.info_button.width()
        );
        assert!(r.top_bar.contains_rect(r.options_button));
        assert!(r.top_bar.contains_rect(r.info_button));
        assert!(!r.options_button.intersects(r.info_button));
        assert!(!r.info_button.intersects(r.reset_button));
        assert!(!r.info_button.intersects(r.bracket_bar));
    }

    let viewport = vec2(1180.0, 700.0);
    let r = render_at_viewport(&mut app, &ctx, viewport);
    let click_pos = r.info_button.center();
    app.onboarding_step = 2;
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
        app.onboarding_open,
        "clicking top-bar `i` button must reopen onboarding guide even after dismissal"
    );
    assert_eq!(app.onboarding_step, 0);
}

#[test]
fn onboarding_guide_navigates_steps_and_handles_clipboard_stratz_token() {
    let mut app = TrueSightGuiApp::new(AppSession::new().unwrap(), None, None);
    app.onboarding_open = true;
    app.onboarding_step = 0;
    let ctx = egui::Context::default();
    let r = render_at_viewport(&mut app, &ctx, vec2(1180.0, 700.0));
    assert!(
        r.onboarding_window.is_some(),
        "onboarding guide window must render when onboarding_open is true"
    );

    // Simulate active 90s clipboard watcher detecting a valid STRATZ JWT
    let now = 1_700_000_000_u64;
    app.stratz_clipboard_watch_until = Some(now + 90);
    let exp = now + 365 * 86_400;
    let payload = format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521469","iat":{now},"exp":{exp}}}"#
    );
    let hdr_b64 = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9";
    let pay_b64 = encode_b64url_for_test(payload.as_bytes());
    let jwt = format!("{hdr_b64}.{pay_b64}.c2ln");

    assert!(app.process_stratz_clipboard_text(&jwt, now));
    assert!(app.stratz_clipboard_watch_until.is_none());
    assert_eq!(app.session.cache_mgr.player_account_id, Some(127_255_741));
    assert!(
        app.status_banner
            .contains("STRATZ token connected (365d remaining)"),
        "unexpected status_banner: {}",
        app.status_banner
    );
}

#[test]
fn pending_stratz_steam_link_preserves_explicit_account_in_gui_until_confirmed() {
    let mut session = AppSession::new().unwrap();
    session.set_account_id(Some(111_222_333), true);
    let mut app = TrueSightGuiApp::new(session, None, None);
    app.onboarding_open = true;
    app.onboarding_step = 2;

    let now = truesight_adapters::sync::LocalCacheManager::now_unix();
    let exp = now + 365 * 86_400;
    let payload = format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521469","iat":{now},"exp":{exp}}}"#
    );
    let hdr_b64 = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9";
    let pay_b64 = encode_b64url_for_test(payload.as_bytes());
    let jwt = format!("{hdr_b64}.{pay_b64}.c2ln");

    assert!(app.apply_stratz_token_ui(&jwt, now));
    assert_eq!(app.session.cache_mgr.player_account_id, Some(111_222_333));
    assert_eq!(app.session.pending_stratz_steam32_link, Some(127_255_741));

    let ctx = egui::Context::default();
    let r = render_at_viewport(&mut app, &ctx, vec2(1180.0, 700.0));
    assert!(r.onboarding_window.is_some());
}

#[test]
fn connect_stratz_ignores_initial_clipboard_baseline_until_new_token_copied() {
    let mut app = TrueSightGuiApp::new(AppSession::new().unwrap(), None, None);
    let now = truesight_adapters::sync::LocalCacheManager::now_unix();
    let exp = now + 365 * 86_400;
    let hdr_b64 = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9";

    let payload_a = format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521469","iat":{now},"exp":{exp}}}"#
    );
    let token_a = format!(
        "{hdr_b64}.{}.c2ln",
        encode_b64url_for_test(payload_a.as_bytes())
    );

    let payload_b = format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521470","iat":{now},"exp":{exp}}}"#
    );
    let token_b = format!(
        "{hdr_b64}.{}.c2ln",
        encode_b64url_for_test(payload_b.as_bytes())
    );

    // Pre-existing valid token_a on the clipboard before clicking CONNECT STRATZ.
    app.test_clipboard_override = Some(token_a.clone());
    app.start_stratz_clipboard_watch(now);
    app.poll_stratz_clipboard_watcher();

    assert!(
        app.is_watching_stratz_clipboard(now),
        "watcher must stay active when clipboard still holds the initial baseline token"
    );
    assert_eq!(
        app.session.stratz_api_token, None,
        "initial baseline token_a must NOT be applied"
    );

    // User later copies a new valid token_b onto the clipboard.
    app.test_clipboard_override = Some(token_b.clone());
    app.poll_stratz_clipboard_watcher();

    assert!(
        !app.is_watching_stratz_clipboard(now),
        "watcher must stop after applying new token_b"
    );
    assert_eq!(
        app.session.stratz_api_token.as_deref(),
        Some(token_b.as_str()),
        "new token_b must be applied once clipboard changes from baseline"
    );
}

#[test]
fn paste_stratz_token_reads_current_clipboard_not_stale_snapshot() {
    let mut app = TrueSightGuiApp::new(AppSession::new().unwrap(), None, None);
    let now = truesight_adapters::sync::LocalCacheManager::now_unix();
    let exp = now + 365 * 86_400;
    let hdr_b64 = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9";

    let payload_c = format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521471","iat":{now},"exp":{exp}}}"#
    );
    let token_c = format!(
        "{hdr_b64}.{}.c2ln",
        encode_b64url_for_test(payload_c.as_bytes())
    );

    // Even with no watcher active, clicking PASTE TOKEN reads the current clipboard value.
    app.test_clipboard_override = Some(token_c.clone());
    app.try_paste_stratz_token();

    assert_eq!(
        app.session.stratz_api_token.as_deref(),
        Some(token_c.as_str()),
        "PASTE TOKEN must apply the current clipboard token_c"
    );
    assert!(!app.stratz_paste_open);

    // Simulate a subsequent clipboard change to token_d and click PASTE TOKEN again.
    let payload_d = format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521472","iat":{now},"exp":{exp}}}"#
    );
    let token_d = format!(
        "{hdr_b64}.{}.c2ln",
        encode_b64url_for_test(payload_d.as_bytes())
    );
    app.test_clipboard_override = Some(token_d.clone());
    app.try_paste_stratz_token();

    assert_eq!(
        app.session.stratz_api_token.as_deref(),
        Some(token_d.as_str()),
        "PASTE TOKEN must read fresh clipboard value token_d instead of any stale snapshot"
    );
}

#[test]
fn clipboard_watcher_retries_delivery_when_single_slot_queue_is_full() {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::sync_channel::<String>(1);
    let mut last_seen: Option<String> = Some("baseline_text".into());

    // First sample fills the 1-slot channel.
    assert!(
        super::onboarding::try_deliver_clipboard_sample(
            &tx,
            &mut last_seen,
            "unrelated_copied_url".into()
        ),
        "first sample should enter the empty 1-slot queue"
    );
    assert_eq!(last_seen.as_deref(), Some("unrelated_copied_url"));

    // Second sample (the new STRATZ token) arrives while the 1-slot queue is still full.
    let now = truesight_adapters::sync::LocalCacheManager::now_unix();
    let exp = now + 365 * 86_400;
    let hdr_b64 = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9";
    let payload = format!(
        r#"{{"iss":"https://api.stratz.com","SteamId":"76561198087521479","iat":{now},"exp":{exp}}}"#
    );
    let token = format!(
        "{hdr_b64}.{}.c2ln",
        encode_b64url_for_test(payload.as_bytes())
    );

    assert!(
        !super::onboarding::try_deliver_clipboard_sample(&tx, &mut last_seen, token.clone()),
        "send must report full queue without advancing last_seen"
    );
    assert_eq!(
        last_seen.as_deref(),
        Some("unrelated_copied_url"),
        "last_seen must NOT advance when try_send fails on a full queue"
    );

    // Consumer drains the first sample; next watcher poll retries the same clipboard token and succeeds.
    assert_eq!(rx.try_recv().ok().as_deref(), Some("unrelated_copied_url"));
    assert!(
        super::onboarding::try_deliver_clipboard_sample(&tx, &mut last_seen, token.clone()),
        "watcher must retry and deliver the token once the queue slot frees up"
    );
    assert_eq!(last_seen.as_deref(), Some(token.as_str()));
    assert_eq!(rx.try_recv().ok().as_deref(), Some(token.as_str()));
}

fn encode_b64url_for_test(input: &[u8]) -> String {
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

#[test]
fn gui_build_bar_and_overlay_preserve_win_parity_and_render_variants() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 anti-mage").unwrap());
    assert!(execute_command(&mut session, "enemy 1 phantom assassin").unwrap());
    session.draft.target_role = Role::Mid;

    let (analysis, _) = session.evaluate();
    let expected_recs = truesight_core::math::recommend_heroes(
        &session.draft,
        &session.matrix,
        &session.weights,
        8,
    );
    let summary = super::viewmodel::compute_overlay_summary(&session);
    assert!(!summary.is_post_draft);
    assert!(
        (summary.projected_ally_win_pct - analysis.ally_win_probability * 100.0).abs() < 1e-4,
        "overlay win% must match main window analysis exactly"
    );
    assert_eq!(summary.top_suggestions.len(), expected_recs.len().min(8));
    for ((id, _, win_pct), expected) in summary.top_suggestions.iter().zip(expected_recs.iter()) {
        assert_eq!(*id, expected.hero_id);
        assert!((*win_pct - expected.projected_win_probability * 100.0).abs() < 1e-4);
    }

    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();

    // Render overlay in draft phase
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = app.render_overlay_panel(ui).rect;
                assert!(r.width() > 0.0 && r.height() > 0.0);
            });
        },
    );

    // Transition to post-draft and render both build bar card and overlay post-draft panel
    app.session
        .apply_event(truesight_core::ports::DraftEvent::GameRulesStateChanged {
            state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
            game_time_secs: 600,
        });
    let summary_post = super::viewmodel::compute_overlay_summary(&app.session);
    assert!(summary_post.is_post_draft);
    assert!(summary_post.build.is_some());

    let mut suggested_rect = egui::Rect::NOTHING;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                egui::vec2(1180.0, 700.0),
            )),
            ..Default::default()
        },
        |ctx| {
            let rects = app.render_frame(ctx);
            suggested_rect = rects.suggested;
        },
    );
    assert!(suggested_rect.width() > 0.0 && suggested_rect.height() > 0.0);

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = app.render_overlay_panel(ui).rect;
                assert!(r.width() > 0.0 && r.height() > 0.0);
            });
        },
    );
}

fn collect_painted_texts(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
    match shape {
        egui::epaint::Shape::Text(text_shape) => {
            out.push(text_shape.galley.text().to_string());
        }
        egui::epaint::Shape::Vec(shapes) => {
            for inner in shapes {
                collect_painted_texts(inner, out);
            }
        }
        _ => {}
    }
}

#[test]
fn gui_overlay_syncs_and_clamps_live_viewport_geometry() {
    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.set_known_monitors(vec![crate::session::MonitorRect::new(
        0.0, 0.0, 2560.0, 1440.0,
    )]);
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 120.0,
        y: 130.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();

    let mut raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            pos2(0.0, 0.0),
            egui::vec2(380.0, 230.0),
        )),
        ..Default::default()
    };
    raw_input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            monitor_size: Some(egui::vec2(2560.0, 1440.0)),
            outer_rect: Some(egui::Rect::from_min_size(
                pos2(240.0, 40.0),
                egui::vec2(380.0, 230.0),
            )),
            inner_rect: Some(egui::Rect::from_min_size(
                pos2(240.0, 40.0),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        },
    );

    let full_output = ctx.run(raw_input, |vctx| {
        app.sync_live_overlay_viewport_geometry(vctx);
    });

    assert_eq!(app.session.overlay_geometry.x, 240.0);
    assert_eq!(
        app.session.overlay_geometry.y, 116.0,
        "live viewport sync must clamp y >= 116.0 on 1440p monitor"
    );
    let root_cmds = &full_output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .expect("root viewport output must exist")
        .commands;
    assert!(
        root_cmds.iter().any(|cmd| matches!(
            cmd,
            egui::ViewportCommand::OuterPosition(p) if (p.x - 240.0).abs() < 0.5 && (p.y - 116.0).abs() < 0.5
        )),
        "must emit ViewportCommand::OuterPosition to enforce clamp at runtime, got: {root_cmds:?}"
    );
}

#[test]
fn gui_overlay_keeps_next_badge_visible_for_fifth_step() {
    use truesight_core::ports::ItemMetaRepository;

    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 anti-mage").unwrap());
    assert!(execute_command(&mut session, "enemy 1 phantom assassin").unwrap());
    session.apply_event(truesight_core::ports::DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 1800,
    });

    let initial_build = session
        .compute_recommended_build()
        .expect("picked ally must produce a build");
    assert!(
        initial_build.steps.len() >= 5,
        "build must have at least 5 steps"
    );

    // Mark the first 4 items as owned so [NEXT] moves to the 5th step (index 4).
    session.owned_items = initial_build.steps[..4].iter().map(|s| s.item_id).collect();
    let build_with_fifth_next = session.compute_recommended_build().unwrap();
    assert_eq!(build_with_fifth_next.next_unowned_index, Some(4));
    let fifth_item_name = session
        .item_repo
        .item_catalog(build_with_fifth_next.steps[4].item_id)
        .map(|e| e.display_name)
        .unwrap();

    let fifth_timing = format!(
        "{}m",
        build_with_fifth_next.steps[4]
            .avg_time_mins
            .round()
            .max(1.0) as u32
    );

    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();
    let mut full_output = None;
    for _ in 0..2 {
        full_output = Some(ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    pos2(0.0, 0.0),
                    egui::vec2(380.0, 230.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = app.render_overlay_panel(ui);
                });
            },
        ));
    }
    let full_output = full_output.expect("second pass overlay output");

    let mut texts = Vec::new();
    for clipped in &full_output.shapes {
        collect_painted_texts(&clipped.shape, &mut texts);
    }
    assert!(
        texts.iter().any(|t| t == "NEXT"),
        "overlay must render NEXT badge even when next_unowned_index is 4; painted texts: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == &fifth_timing),
        "overlay must paint overlaid timing '{fifth_timing}' on the 5th step icon; painted texts: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t == &fifth_item_name),
        "icon-only build overlay must omit verbose item label '{fifth_item_name}'; painted texts: {texts:?}"
    );
}

#[test]
fn live_overlay_viewport_geometry_preserves_secondary_monitor_and_avoids_offset_capture_strip() {
    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.set_known_monitors(vec![
        crate::session::MonitorRect::new(0.0, 0.0, 1920.0, 1080.0),
        crate::session::MonitorRect::new(1920.0, 0.0, 1920.0, 1080.0),
    ]);
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 2200.0,
        y: 120.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();

    // Pass 1: Secondary monitor at (1920, 0) with size 1920x1080 and overlay placed at (2200, 30).
    let mut raw_input_secondary = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            pos2(0.0, 0.0),
            egui::vec2(380.0, 230.0),
        )),
        ..Default::default()
    };
    raw_input_secondary.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            monitor_size: Some(egui::vec2(1920.0, 1080.0)),
            outer_rect: Some(egui::Rect::from_min_size(
                pos2(2200.0, 30.0),
                egui::vec2(380.0, 230.0),
            )),
            inner_rect: Some(egui::Rect::from_min_size(
                pos2(2200.0, 30.0),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        },
    );

    let secondary_output = ctx.run(raw_input_secondary, |vctx| {
        app.sync_live_overlay_viewport_geometry(vctx);
    });
    assert_eq!(
        app.session.overlay_geometry.x, 2200.0,
        "secondary monitor X coordinate must not be forced back to 0..1920"
    );
    assert_eq!(app.session.overlay_geometry.y, 96.0);
    let secondary_cmds = &secondary_output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .expect("root viewport output must exist")
        .commands;
    assert!(secondary_cmds.iter().any(|cmd| matches!(
        cmd,
        egui::ViewportCommand::OuterPosition(p) if (p.x - 2200.0).abs() < 0.5 && (p.y - 96.0).abs() < 0.5
    )));

    // Pass 2: Offset windowed Dota capture strip at (200, 180, 1600, 72) on primary monitor.
    app.session
        .set_active_capture_strip_rect(Some((200.0, 180.0, 1600.0, 72.0)));
    let mut raw_input_strip = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            pos2(0.0, 0.0),
            egui::vec2(380.0, 230.0),
        )),
        ..Default::default()
    };
    raw_input_strip.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            monitor_size: Some(egui::vec2(1920.0, 1080.0)),
            outer_rect: Some(egui::Rect::from_min_size(
                pos2(240.0, 160.0),
                egui::vec2(380.0, 230.0),
            )),
            inner_rect: Some(egui::Rect::from_min_size(
                pos2(240.0, 160.0),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        },
    );

    let strip_output = ctx.run(raw_input_strip, |vctx| {
        app.sync_live_overlay_viewport_geometry(vctx);
    });
    assert_eq!(app.session.overlay_geometry.x, 240.0);
    assert_eq!(
        app.session.overlay_geometry.y, 252.0,
        "overlay overlapping offset capture strip must be pushed below y=252.0"
    );
    let strip_cmds = &strip_output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .expect("root viewport output must exist")
        .commands;
    assert!(strip_cmds.iter().any(|cmd| matches!(
        cmd,
        egui::ViewportCommand::OuterPosition(p) if (p.x - 240.0).abs() < 0.5 && (p.y - 252.0).abs() < 0.5
    )));
}

#[test]
fn live_overlay_viewport_geometry_handles_heterogeneous_monitors_and_unavailable_bounds_fallback() {
    // 1. Heterogeneous dual-monitor setup: 1920x1080 primary at (0, 0) + 1280x720 secondary at (1920, 0).
    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.set_known_monitors(vec![
        crate::session::MonitorRect::new(0.0, 0.0, 1920.0, 1080.0),
        crate::session::MonitorRect::new(1920.0, 0.0, 1280.0, 720.0),
    ]);
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 2450.0,
        y: 140.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();

    let make_input = |x: f32, y: f32| {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        };
        raw.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                monitor_size: Some(egui::vec2(1280.0, 720.0)),
                outer_rect: Some(egui::Rect::from_min_size(
                    pos2(x, y),
                    egui::vec2(380.0, 230.0),
                )),
                inner_rect: Some(egui::Rect::from_min_size(
                    pos2(x, y),
                    egui::vec2(380.0, 230.0),
                )),
                ..Default::default()
            },
        );
        raw
    };

    let _ = ctx.run(make_input(2450.0, 140.0), |vctx| {
        app.sync_live_overlay_viewport_geometry(vctx);
    });
    assert_eq!(
        app.session.overlay_geometry.x, 2450.0,
        "valid 380pt overlay at x=2450 on 1280pt secondary monitor (1920..3200) must not be moved to 2180"
    );
    assert_eq!(app.session.overlay_geometry.y, 140.0);

    // Dragging past the right edge of the 1280pt secondary monitor (x=3000) clamps to 1920 + 1280 - 380 = 2820.
    let _ = ctx.run(make_input(3000.0, 140.0), |vctx| {
        app.sync_live_overlay_viewport_geometry(vctx);
    });
    assert_eq!(app.session.overlay_geometry.x, 2820.0);

    // 2. Reliable native monitor bounds unavailable: must NOT reposition x=2450 based on a guessed origin.
    let mut fallback_session = AppSession::new().unwrap();
    fallback_session.overlay_mode = true;
    fallback_session.overlay_geometry = crate::session::OverlayGeometry {
        x: 2450.0,
        y: 140.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let mut fallback_app = TrueSightGuiApp::new(fallback_session, None, None);
    fallback_app.native_monitors_probed = true;
    let _ = ctx.run(make_input(2450.0, 140.0), |vctx| {
        fallback_app.sync_live_overlay_viewport_geometry(vctx);
    });
    assert_eq!(
        fallback_app.session.overlay_geometry.x, 2450.0,
        "when native monitor bounds are unavailable, x=2450 must not be moved to guessed 2180"
    );
    assert_eq!(fallback_app.session.overlay_geometry.y, 140.0);
}

#[test]
fn live_overlay_avoids_offset_capture_strip_from_bounded_vision_worker_production_path() {
    use truesight_adapters::vision::{BoundedVisionWorker, RgbFrame, ScreenFrameSource};
    use truesight_core::ports::PortError;

    struct OffsetWindowSource;
    impl ScreenFrameSource for OffsetWindowSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(Some(
                RgbFrame::with_source_height(1600, 72, 900, vec![0; 1600 * 72 * 3])
                    .with_capture_strip_rect((200.0, 180.0, 1600.0, 72.0)),
            ))
        }
    }

    let worker = BoundedVisionWorker::spawn_with_source(OffsetWindowSource)
        .expect("vision worker must spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while worker.capture_strip_rect().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        worker.capture_strip_rect(),
        Some((200.0, 180.0, 1600.0, 72.0))
    );

    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.set_known_monitors(vec![crate::session::MonitorRect::new(
        0.0, 0.0, 1920.0, 1080.0,
    )]);
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 240.0,
        y: 160.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };

    let mut app = TrueSightGuiApp::new(session, None, Some(worker));
    let ctx = egui::Context::default();
    let mut raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            pos2(0.0, 0.0),
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };
    let overlay_id = egui::ViewportId::from_hash_of("truesight_compact_overlay");
    raw_input.viewports.insert(
        overlay_id,
        egui::ViewportInfo {
            monitor_size: Some(egui::vec2(1920.0, 1080.0)),
            outer_rect: Some(egui::Rect::from_min_size(
                pos2(240.0, 160.0),
                egui::vec2(380.0, 230.0),
            )),
            inner_rect: Some(egui::Rect::from_min_size(
                pos2(240.0, 160.0),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        },
    );

    let _ = ctx.run(raw_input, |ctx| {
        let _ = app.render_frame(ctx);
    });

    assert_eq!(
        app.session.active_capture_strip_rect,
        Some((200.0, 180.0, 1600.0, 72.0)),
        "production render_frame -> poll_live_sources must populate active_capture_strip_rect from BoundedVisionWorker"
    );
    assert_eq!(
        app.session.overlay_geometry.y, 252.0,
        "overlay overlapping live worker's offset capture strip (y=180..252) must be clamped to y=252.0"
    );
}

#[test]
fn live_overlay_viewport_geometry_normalizes_mixed_dpi_secondary_monitor_and_capture_strip() {
    use truesight_adapters::vision::{BoundedVisionWorker, RgbFrame, ScreenFrameSource};
    use truesight_core::ports::PortError;

    // Physical Dota capture strip on secondary 150%-DPI monitor (origin 1920, 0):
    // physical rect = (2100, 180, 1600, 72) -> y = 180..252 physical px.
    struct SecondaryMonitorDotaSource;
    impl ScreenFrameSource for SecondaryMonitorDotaSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(Some(
                RgbFrame::with_source_height(1600, 72, 1440, vec![0; 1600 * 72 * 3])
                    .with_capture_strip_rect((2100.0, 180.0, 1600.0, 72.0)),
            ))
        }
    }

    let worker = BoundedVisionWorker::spawn_with_source(SecondaryMonitorDotaSource)
        .expect("vision worker must spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while worker.capture_strip_rect().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        worker.capture_strip_rect(),
        Some((2100.0, 180.0, 1600.0, 72.0))
    );

    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    // Mixed-DPI desktop: 100% primary (1920x1080 at 0,0) + 150% secondary (2560x1440 at 1920,0) in physical pixels.
    session.set_known_monitors(vec![
        crate::session::MonitorRect::new(0.0, 0.0, 1920.0, 1080.0),
        crate::session::MonitorRect::new(1920.0, 0.0, 2560.0, 1440.0),
    ]);
    // Overlay physically at (2100, 180, 570, 345) on 150% monitor -> logical egui points (1400, 120, 380, 230).
    // Without DPI normalization, x=1400 looks inside primary (0..1920) and misses the physical strip at (2100, 180).
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 1400.0,
        y: 120.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };

    let mut app = TrueSightGuiApp::new(session, None, Some(worker));
    let ctx = egui::Context::default();
    let overlay_id = egui::ViewportId::from_hash_of("truesight_compact_overlay");

    let make_input = |x_pts: f32, y_pts: f32| {
        let vp_info = egui::ViewportInfo {
            native_pixels_per_point: Some(1.5),
            monitor_size: Some(egui::vec2(2560.0 / 1.5, 1440.0 / 1.5)),
            outer_rect: Some(egui::Rect::from_min_size(
                pos2(x_pts, y_pts),
                egui::vec2(380.0, 230.0),
            )),
            inner_rect: Some(egui::Rect::from_min_size(
                pos2(x_pts, y_pts),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        };
        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        raw_input
            .viewports
            .insert(egui::ViewportId::ROOT, vp_info.clone());
        raw_input.viewports.insert(overlay_id, vp_info);
        raw_input
    };

    let _ = ctx.run(make_input(1400.0, 120.0), |ctx| {
        let _ = app.render_frame(ctx);
    });

    assert_eq!(
        app.session.overlay_geometry.x, 1400.0,
        "150%-DPI secondary monitor overlay at physical x=2100 (1400pt) must stay on secondary monitor"
    );
    assert_eq!(
        app.session.overlay_geometry.y, 168.0,
        "150%-DPI overlay overlapping physical strip (y=180..252) must be pushed to physical y=252 (168pt)"
    );
    assert!(
        (app.session.overlay_geometry.width - 380.0).abs() < 1e-4,
        "width should stay ~380.0, got {}",
        app.session.overlay_geometry.width
    );
    assert!(
        (app.session.overlay_geometry.height - 230.0).abs() < 1e-4,
        "height should stay ~230.0, got {}",
        app.session.overlay_geometry.height
    );

    // Dragging past right edge of 150% secondary monitor (1920 + 2560 - 570 = 3910px = 2606.6667pt)
    let _ = ctx.run(make_input(2800.0, 168.0), |ctx| {
        let _ = app.render_frame(ctx);
    });
    assert!(
        (app.session.overlay_geometry.x - (3910.0 / 1.5)).abs() < 0.1,
        "dragging past right edge on 150%-DPI secondary monitor must clamp to physical max_x / 1.5, got {}",
        app.session.overlay_geometry.x
    );
}

#[test]
fn live_overlay_viewport_geometry_honors_effective_scale_with_native_dpi_and_app_zoom() {
    use truesight_adapters::vision::{BoundedVisionWorker, RgbFrame, ScreenFrameSource};
    use truesight_core::ports::PortError;

    // Physical Dota capture strip on secondary 150%-DPI monitor (origin 1920, 0):
    // physical rect = (2160, 180, 1600, 72) -> y = 180..252 physical px.
    struct SecondaryMonitorZoomedDotaSource;
    impl ScreenFrameSource for SecondaryMonitorZoomedDotaSource {
        fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
            Ok(Some(
                RgbFrame::with_source_height(1600, 72, 1440, vec![0; 1600 * 72 * 3])
                    .with_capture_strip_rect((2160.0, 180.0, 1600.0, 72.0)),
            ))
        }
    }

    let worker = BoundedVisionWorker::spawn_with_source(SecondaryMonitorZoomedDotaSource)
        .expect("vision worker must spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while worker.capture_strip_rect().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        worker.capture_strip_rect(),
        Some((2160.0, 180.0, 1600.0, 72.0))
    );

    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    // Mixed-DPI desktop: 100% primary (1920x1080 at 0,0) + 150% secondary (2560x1440 at 1920,0) in physical pixels.
    session.set_known_monitors(vec![
        crate::session::MonitorRect::new(0.0, 0.0, 1920.0, 1080.0),
        crate::session::MonitorRect::new(1920.0, 0.0, 2560.0, 1440.0),
    ]);
    // With native_pixels_per_point = 1.5 and app zoom_factor = 1.2, effective ppp = 1.8.
    // Logical (1200, 100, 380, 230) -> physical (2160, 180, 684, 414) on secondary monitor.
    // Using raw native 1.5 instead of effective 1.8 would map x=1200 to 1800px (wrong primary monitor!).
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 1200.0,
        y: 100.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };

    let mut app = TrueSightGuiApp::new(session, None, Some(worker));
    let ctx = egui::Context::default();
    ctx.set_zoom_factor(1.2);
    // `Context::set_zoom_factor` takes effect at the end of a pass for the following pass.
    let _ = ctx.run(egui::RawInput::default(), |_| {});
    assert!((ctx.zoom_factor() - 1.2).abs() < 1e-5);
    let overlay_id = egui::ViewportId::from_hash_of("truesight_compact_overlay");

    let make_input = |x_pts: f32, y_pts: f32| {
        let effective_ppp = 1.5 * 1.2;
        let vp_info = egui::ViewportInfo {
            native_pixels_per_point: Some(1.5),
            monitor_size: Some(egui::vec2(2560.0 / effective_ppp, 1440.0 / effective_ppp)),
            outer_rect: Some(egui::Rect::from_min_size(
                pos2(x_pts, y_pts),
                egui::vec2(380.0, 230.0),
            )),
            inner_rect: Some(egui::Rect::from_min_size(
                pos2(x_pts, y_pts),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        };
        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        raw_input
            .viewports
            .insert(egui::ViewportId::ROOT, vp_info.clone());
        raw_input.viewports.insert(overlay_id, vp_info);
        raw_input
    };

    let _ = ctx.run(make_input(1200.0, 100.0), |ctx| {
        let _ = app.render_frame(ctx);
    });

    assert!(
        (app.session.overlay_geometry.x - 1200.0).abs() < 0.1,
        "150%-DPI + 1.2x zoom overlay at physical x=2160 (1200pt) must stay on secondary monitor, got {}",
        app.session.overlay_geometry.x
    );
    assert!(
        (app.session.overlay_geometry.y - (252.0 / 1.8)).abs() < 0.1,
        "150%-DPI + 1.2x zoom overlay overlapping physical strip (y=180..252) must be pushed to physical y=252 (140pt), got {}",
        app.session.overlay_geometry.y
    );
    assert!(
        (app.session.overlay_geometry.width - 380.0).abs() < 1e-4,
        "width should stay ~380.0, got {}",
        app.session.overlay_geometry.width
    );
    assert!(
        (app.session.overlay_geometry.height - 230.0).abs() < 1e-4,
        "height should stay ~230.0, got {}",
        app.session.overlay_geometry.height
    );

    // Dragging past right edge of 150% secondary monitor with 1.2x zoom:
    // physical max_x = 1920 + 2560 - (380 * 1.8) = 4480 - 684 = 3796px -> 3796 / 1.8 = 2108.8889pt.
    let _ = ctx.run(make_input(2500.0, 140.0), |ctx| {
        let _ = app.render_frame(ctx);
    });
    assert!(
        (app.session.overlay_geometry.x - (3796.0 / 1.8)).abs() < 0.1,
        "dragging past right edge on 150%-DPI + 1.2x zoom secondary monitor must clamp to physical max_x / 1.8, got {}",
        app.session.overlay_geometry.x
    );
}

#[test]
fn overlay_drag_and_resize_handles_emit_start_drag_and_begin_resize_commands() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.session.overlay_mode = true;
    app.session.set_overlay_click_through(false);

    let ctx = egui::Context::default();

    // Hold-and-drag workflow + Wayland pointer release reset + vertical expandability + reset + icons
    let run_overlay_at =
        |vp_size: egui::Vec2, events: Vec<egui::Event>, app: &mut TrueSightGuiApp| {
            let mut panel_out = None;
            let full_output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vp_size)),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        panel_out = Some(app.render_overlay_panel(ui));
                    });
                },
            );
            (panel_out.expect("overlay panel output"), full_output)
        };

    fn count_user_textured_meshes(shape: &egui::epaint::Shape) -> usize {
        match shape {
            egui::epaint::Shape::Mesh(m) => {
                usize::from(m.texture_id != egui::TextureId::Managed(0))
            }
            egui::epaint::Shape::Vec(v) => v.iter().map(count_user_textured_meshes).sum(),
            _ => 0,
        }
    }

    let vp_compact = egui::vec2(380.0, 230.0);
    let (initial, initial_full) = run_overlay_at(vp_compact, vec![], &mut app);
    assert!(initial.drag_handle.width() > 0.0 && initial.drag_handle.height() > 0.0);
    assert!(initial.resize_handle.width() > 0.0 && initial.resize_handle.height() > 0.0);
    assert!(initial.reset_button.width() > 0.0 && initial.reset_button.height() > 0.0);
    assert!(!initial.drag_started);
    assert!(!initial.resize_started);
    assert_eq!(app.textures.heroes.len(), 127);
    assert_eq!(
        app.textures.items.len(),
        truesight_adapters::item_repo::embedded_item_ids().len()
    );
    let draft_tex_meshes: usize = initial_full
        .shapes
        .iter()
        .map(|c| count_user_textured_meshes(&c.shape))
        .sum();
    assert!(
        draft_tex_meshes > 0,
        "overlay draft view must paint hero portrait meshes"
    );

    // 1. Plain click without holding + moving >= 3pt must NOT trigger StartDrag.
    let drag_start = initial.drag_handle.center();
    let (arm_out, arm_full) = run_overlay_at(
        vp_compact,
        vec![
            egui::Event::PointerMoved(drag_start),
            egui::Event::PointerButton {
                pos: drag_start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ],
        &mut app,
    );
    assert!(!arm_out.drag_started);
    assert!(
        !arm_full
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .unwrap()
            .commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::StartDrag))
    );

    let (release_out, release_full) = run_overlay_at(
        vp_compact,
        vec![egui::Event::PointerButton {
            pos: drag_start,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
        &mut app,
    );
    assert!(!release_out.drag_started);
    assert!(
        !release_full
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .unwrap()
            .commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::StartDrag))
    );
    assert!(app.overlay_drag_armed.is_none());

    // 2. Press + hold + move >= 3pt emits StartDrag AND resets pointer state (fixing Wayland stuck drag).
    let _ = run_overlay_at(
        vp_compact,
        vec![
            egui::Event::PointerMoved(drag_start),
            egui::Event::PointerButton {
                pos: drag_start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ],
        &mut app,
    );
    let drag_end = drag_start + egui::vec2(12.0, 6.0);
    let (drag_out, drag_full) = run_overlay_at(
        vp_compact,
        vec![egui::Event::PointerMoved(drag_end)],
        &mut app,
    );
    assert!(drag_out.drag_started);
    let root_drag_cmds = &drag_full
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .expect("viewport output")
        .commands;
    assert!(
        root_drag_cmds
            .iter()
            .any(|cmd| matches!(cmd, egui::ViewportCommand::StartDrag)),
        "expected ViewportCommand::StartDrag, got {root_drag_cmds:?}"
    );
    assert!(
        !ctx.input(|i| i.pointer.primary_down()),
        "StartDrag must clear stuck primary_down state on Wayland/KWin"
    );
    let (after_move_out, _) = run_overlay_at(
        vp_compact,
        vec![egui::Event::PointerMoved(drag_end + egui::vec2(10.0, 5.0))],
        &mut app,
    );
    assert!(
        !after_move_out.drag_started,
        "moving cursor after drag completes without holding must never re-trigger StartDrag"
    );

    // 3. Hold + move >= 3pt on resize handle emits BeginResize(SouthEast) and resets pointer state.
    let resize_start = initial.resize_handle.center();
    let _ = run_overlay_at(
        vp_compact,
        vec![
            egui::Event::PointerMoved(resize_start),
            egui::Event::PointerButton {
                pos: resize_start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ],
        &mut app,
    );
    let resize_end = resize_start + egui::vec2(10.0, 8.0);
    let (resize_out, resize_full) = run_overlay_at(
        vp_compact,
        vec![egui::Event::PointerMoved(resize_end)],
        &mut app,
    );
    assert!(resize_out.resize_started);
    let root_resize_cmds = &resize_full
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .expect("viewport output")
        .commands;
    assert!(
        root_resize_cmds.iter().any(|cmd| matches!(
            cmd,
            egui::ViewportCommand::BeginResize(egui::ResizeDirection::SouthEast)
        )),
        "expected ViewportCommand::BeginResize(SouthEast), got {root_resize_cmds:?}"
    );
    assert!(!ctx.input(|i| i.pointer.primary_down()));

    // 4. Vertical & horizontal expansion scales overlay frame, resize corner, and item build textures.
    let (tall_out, _) = run_overlay_at(egui::vec2(460.0, 520.0), vec![], &mut app);
    assert!(
        tall_out.rect.height() >= 500.0 && tall_out.resize_handle.bottom() >= 500.0,
        "vertical overlay resize must expand painted panel & corner handle: rect={:?}, resize={:?}",
        tall_out.rect,
        tall_out.resize_handle
    );

    if !app.session.is_post_draft_build_view() {
        app.session.toggle_post_draft_build_view();
    }
    assert!(app.session.is_post_draft_build_view());
    let (_, build_full) = run_overlay_at(egui::vec2(460.0, 320.0), vec![], &mut app);
    let build_tex_meshes: usize = build_full
        .shapes
        .iter()
        .map(|c| count_user_textured_meshes(&c.shape))
        .sum();
    assert!(
        build_tex_meshes > 0,
        "overlay post-draft build view must paint Dota 2 item icons"
    );

    // 5. Clicking overlay reset icon button clears the draft.
    let (compact_now, _) = run_overlay_at(vp_compact, vec![], &mut app);
    let reset_pt = compact_now.reset_button.center();
    for pressed in [true, false] {
        let _ = run_overlay_at(
            vp_compact,
            vec![
                egui::Event::PointerMoved(reset_pt),
                egui::Event::PointerButton {
                    pos: reset_pt,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            &mut app,
        );
    }
    assert!(
        app.session.draft.allies.iter().all(Option::is_none)
            && app.session.draft.enemies.iter().all(Option::is_none),
        "clicking overlay reset icon must clear the active draft"
    );

    // 6. When click-through is active, overlay shows clean PASS badge and ignores handle drags.
    app.session.set_overlay_click_through(true);
    let (locked_out, locked_full) = run_overlay_at(
        vp_compact,
        vec![
            egui::Event::PointerMoved(drag_start),
            egui::Event::PointerButton {
                pos: drag_start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerMoved(drag_end),
        ],
        &mut app,
    );
    assert!(!locked_out.drag_started);
    assert!(!locked_out.resize_started);
    let mut texts = Vec::new();
    for clipped in &locked_full.shapes {
        collect_painted_texts(&clipped.shape, &mut texts);
    }
    assert!(
        texts.iter().any(|t| t == "PASS"),
        "locked overlay must show PASS indicator; texts={texts:?}"
    );
}

#[test]
fn overlay_click_through_unlocks_via_top_bar_button_options_and_ctrl_shift_p_hotkey() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.session.overlay_mode = true;
    app.session.set_overlay_click_through(true);

    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    app.hotkey_listener = Some(super::hotkey::GlobalHotkeyListener::from_channel_for_test(
        rx,
        "Ctrl+Shift+P (Test Global)",
    ));

    let ctx = egui::Context::default();
    let viewport = vec2(1280.0, 800.0);

    let click_at = |pos: egui::Pos2, app: &mut TrueSightGuiApp| {
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    app.render_frame(ctx);
                },
            );
        }
    };

    let press_key = |key: egui::Key, modifiers: egui::Modifiers, app: &mut TrueSightGuiApp| {
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                    modifiers,
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    app.render_frame(ctx);
                },
            );
        }
    };

    // 1. Top bar renders unlock pill while click-through is ON; clicking it disables click-through.
    let r0 = render_at_viewport(&mut app, &ctx, viewport);
    let unlock_rect = r0
        .overlay_pass_unlock_button
        .expect("top bar unlock button must appear while overlay click-through is ON");
    click_at(unlock_rect.center(), &mut app);
    assert!(
        !app.session.overlay_geometry.click_through,
        "clicking top bar unlock button must disable click-through"
    );
    let r1 = render_at_viewport(&mut app, &ctx, viewport);
    assert!(
        r1.overlay_pass_unlock_button.is_none(),
        "top bar unlock button must disappear once click-through is OFF"
    );

    // 2. Pressing F9 must do NOTHING (reserved cleanly for Dota 2 Pause!).
    press_key(egui::Key::F9, egui::Modifiers::default(), &mut app);
    assert!(
        !app.session.overlay_geometry.click_through,
        "F9 must never toggle TrueSight overlay click-through (reserved for Dota 2 Pause)"
    );

    // 3. Pressing Ctrl+Shift+P toggles click-through ON then back OFF without double-toggling.
    let ctrl_shift = egui::Modifiers {
        ctrl: true,
        shift: true,
        ..Default::default()
    };
    press_key(egui::Key::P, ctrl_shift, &mut app);
    assert!(
        app.session.overlay_geometry.click_through,
        "Ctrl+Shift+P must enable click-through without child viewport double-toggling"
    );
    press_key(egui::Key::P, ctrl_shift, &mut app);
    assert!(
        !app.session.overlay_geometry.click_through,
        "second Ctrl+Shift+P press must disable click-through"
    );

    // 4. Out-of-focus global hotkey event via channel toggles click-through automatically.
    tx.send(super::hotkey::GlobalHotkeyAction::ToggleOverlayPass)
        .unwrap();
    let _ = render_at_viewport(&mut app, &ctx, viewport);
    assert!(
        app.session.overlay_geometry.click_through,
        "global hotkey channel must enable click-through without focused window key event"
    );
    tx.send(super::hotkey::GlobalHotkeyAction::ToggleOverlayPass)
        .unwrap();
    let _ = render_at_viewport(&mut app, &ctx, viewport);
    assert!(
        !app.session.overlay_geometry.click_through,
        "second global hotkey event must unlock click-through"
    );
}

#[test]
fn language_selector_switches_es_en_ru_from_quick_guide_and_options() {
    use crate::session::UiLanguage;

    let mut app = TrueSightGuiApp::new(AppSession::new().unwrap(), None, None);
    app.onboarding_open = true;
    let ctx = egui::Context::default();
    let viewport = vec2(1280.0, 800.0);

    let click_at = |pos: egui::Pos2, app: &mut TrueSightGuiApp| {
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    app.render_frame(ctx);
                },
            );
        }
    };

    // 1. Toggle from Quick Guide (`ES`, `EN`, `RU`) + verify clean non-ASCII nav buttons
    let (r0, en_out) = render_at_viewport_with_output(&mut app, &ctx, viewport);
    let mut en_texts = Vec::new();
    for c in &en_out.shapes {
        collect_painted_texts(&c.shape, &mut en_texts);
    }
    assert!(en_texts.iter().any(|t| t == "NEXT"));
    assert!(en_texts.iter().any(|t| t == "BUILD"));
    assert!(
        !en_texts
            .iter()
            .any(|t| t.contains("[ BUILD") || t.contains("NEXT >") || t == "RESET"),
        "main buttons must not render ASCII brackets/arrows or raw RESET text: {en_texts:?}"
    );
    let guide_btns = r0
        .onboarding_lang_buttons
        .expect("Quick Guide must expose [ES] [EN] [RU] buttons");
    for b in guide_btns {
        assert!(b.width() > 0.0 && b.height() > 0.0);
    }

    click_at(guide_btns[0].center(), &mut app);
    assert_eq!(app.session.ui_language, UiLanguage::Es);
    let (_, es_out) = render_at_viewport_with_output(&mut app, &ctx, viewport);
    let mut es_texts = Vec::new();
    for c in &es_out.shapes {
        collect_painted_texts(&c.shape, &mut es_texts);
    }
    assert!(
        es_texts
            .iter()
            .any(|t| t.contains("GUÍA RÁPIDA") || t == "SIGUIENTE"),
        "Quick Guide in ES must paint Spanish labels; got {es_texts:?}"
    );

    let r_es = render_at_viewport(&mut app, &ctx, viewport);
    let guide_btns = r_es.onboarding_lang_buttons.unwrap();
    click_at(guide_btns[2].center(), &mut app);
    assert_eq!(app.session.ui_language, UiLanguage::Ru);
    let (_, ru_out) = render_at_viewport_with_output(&mut app, &ctx, viewport);
    let mut ru_texts = Vec::new();
    for c in &ru_out.shapes {
        collect_painted_texts(&c.shape, &mut ru_texts);
    }
    assert!(
        ru_texts
            .iter()
            .any(|t| t.contains("БЫСТРЫЙ СТАРТ") || t == "ДАЛЕЕ"),
        "Quick Guide in RU must paint Russian labels; got {ru_texts:?}"
    );

    let r_ru = render_at_viewport(&mut app, &ctx, viewport);
    let guide_btns = r_ru.onboarding_lang_buttons.unwrap();
    click_at(guide_btns[1].center(), &mut app);
    assert_eq!(app.session.ui_language, UiLanguage::En);

    // 2. Toggle from Options (`ES`, `EN`, `RU`)
    app.onboarding_open = false;
    app.options_open = true;
    let r_opt = render_at_viewport(&mut app, &ctx, viewport);
    let opt_btns = r_opt
        .options_lang_buttons
        .expect("OPTIONS must expose [ES] [EN] [RU] buttons");
    click_at(opt_btns[0].center(), &mut app);
    assert_eq!(app.session.ui_language, UiLanguage::Es);

    let r_opt = render_at_viewport(&mut app, &ctx, viewport);
    let opt_btns = r_opt.options_lang_buttons.unwrap();
    click_at(opt_btns[2].center(), &mut app);
    assert_eq!(app.session.ui_language, UiLanguage::Ru);

    let r_opt = render_at_viewport(&mut app, &ctx, viewport);
    let opt_btns = r_opt.options_lang_buttons.unwrap();
    click_at(opt_btns[1].center(), &mut app);
    assert_eq!(app.session.ui_language, UiLanguage::En);

    // 3. OPTIONS modal displays installed version and fits inside 640x440 in all 3 languages.
    app.account_editor_open = true;
    let min_vp = vec2(640.0, 440.0);
    let expected_ver = format!("v{}", env!("CARGO_PKG_VERSION"));
    for lang in [UiLanguage::En, UiLanguage::Es, UiLanguage::Ru] {
        app.session.set_ui_language(lang);
        let (r, min_out) = render_at_viewport_with_output(&mut app, &ctx, min_vp);
        let opts_win = r.options_window.expect("OPTIONS window open");
        let demo_btn = r.options_demo_button.expect("DEMO 5V5 button visible");
        let mut min_texts = Vec::new();
        for c in &min_out.shapes {
            collect_painted_texts(&c.shape, &mut min_texts);
        }
        assert!(
            min_texts.iter().any(|t| t == &expected_ver),
            "OPTIONS must display installed version {expected_ver} in {lang:?}: {min_texts:?}"
        );
        assert!(
            opts_win.contains_rect(demo_btn)
                && opts_win.max.x <= min_vp.x
                && opts_win.max.y <= min_vp.y
                && demo_btn.max.y <= min_vp.y,
            "lang={lang:?} OPTIONS {opts_win:?} or DEMO {demo_btn:?} overflowed {min_vp:?}"
        );
    }
}

#[test]
fn overlay_resize_grip_remains_inside_minimum_280x160_viewport_and_starts_resize() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.session.overlay_mode = true;
    app.session.set_overlay_click_through(false);

    let ctx = egui::Context::default();
    let min_vp = vec2(280.0, 160.0);
    let screen_rect = Rect::from_min_size(pos2(0.0, 0.0), min_vp);
    let run_min_overlay = |events: Vec<egui::Event>, app: &mut TrueSightGuiApp| {
        let mut out = None;
        let full = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        out = Some(app.render_overlay_panel(ui));
                    });
            },
        );
        (out.expect("overlay panel output"), full)
    };

    let (initial, _) = run_min_overlay(vec![], &mut app);
    assert!(
        screen_rect.contains_rect(initial.resize_handle)
            && initial.resize_handle.width() >= 10.0
            && initial.resize_handle.height() >= 10.0,
        "resize handle {:?} must stay completely inside minimum 280x160 viewport {:?}",
        initial.resize_handle,
        screen_rect
    );

    let start = initial.resize_handle.center();
    let _ = run_min_overlay(
        vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ],
        &mut app,
    );
    let (dragged, full) = run_min_overlay(
        vec![egui::Event::PointerMoved(start + vec2(8.0, 6.0))],
        &mut app,
    );
    assert!(
        dragged.resize_started,
        "holding and dragging minimum-size resize handle must initiate BeginResize"
    );
    let cmds = &full
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .expect("root viewport output")
        .commands;
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            egui::ViewportCommand::BeginResize(egui::ResizeDirection::SouthEast)
        )),
        "expected BeginResize(SouthEast), got {cmds:?}"
    );
}

#[test]
fn gui_overlay_preserves_resized_dimensions_on_native_wayland_without_position_rects() {
    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 120.0,
        y: 130.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let mut app = TrueSightGuiApp::new(session, None, None);
    app.native_monitors_probed = true;
    let ctx = egui::Context::default();

    let mut raw_input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 420.0))),
        ..Default::default()
    };
    raw_input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            monitor_size: None,
            outer_rect: None,
            inner_rect: None,
            ..Default::default()
        },
    );

    let full_output = ctx.run(raw_input, |vctx| {
        app.sync_live_overlay_viewport_geometry(vctx);
    });

    assert_eq!(app.session.overlay_geometry.width, 480.0);
    assert_eq!(app.session.overlay_geometry.height, 420.0);
    assert_eq!(app.session.overlay_geometry.x, 120.0);
    assert_eq!(app.session.overlay_geometry.y, 130.0);
    let cmds = &full_output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .expect("root viewport output")
        .commands;
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::OuterPosition(_))),
        "must not emit OuterPosition when native Wayland omits outer/inner position rects: {cmds:?}"
    );
}

#[test]
fn hotkey_ignores_key_repeats_and_processes_multiple_queued_global_presses() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.session.overlay_mode = true;
    app.session.set_overlay_click_through(false);

    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    app.hotkey_listener = Some(super::hotkey::GlobalHotkeyListener::from_channel_for_test(
        rx,
        "Ctrl+Shift+P (Test Global)",
    ));

    let ctx = egui::Context::default();
    let viewport = vec2(1280.0, 800.0);
    let send_key_event = |key: egui::Key,
                          pressed: bool,
                          repeat: bool,
                          modifiers: egui::Modifiers,
                          app: &mut TrueSightGuiApp| {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                modifiers,
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| {
                app.render_frame(ctx);
            },
        );
    };

    let ctrl_shift = egui::Modifiers {
        ctrl: true,
        shift: true,
        ..Default::default()
    };
    // Initial press toggles ON (`false -> true`), held auto-repeat (`keys_down` active -> egui `repeat: true`)
    // MUST NOT flicker back OFF; release afterwards cleans `keys_down`.
    send_key_event(egui::Key::P, true, false, ctrl_shift, &mut app);
    assert!(
        app.session.overlay_geometry.click_through,
        "initial Ctrl+Shift+P press must toggle overlay click-through ON"
    );
    send_key_event(egui::Key::P, true, true, ctrl_shift, &mut app);
    assert!(
        app.session.overlay_geometry.click_through,
        "repeating Ctrl+Shift+P key event while held must not toggle overlay click-through"
    );
    send_key_event(egui::Key::P, false, false, ctrl_shift, &mut app);

    // Same for F8: initial press toggles overlay OFF (`true -> false`), held auto-repeat MUST NOT flicker back ON.
    send_key_event(
        egui::Key::F8,
        true,
        false,
        egui::Modifiers::default(),
        &mut app,
    );
    assert!(
        !app.session.overlay_mode,
        "initial F8 press must toggle overlay mode OFF"
    );
    send_key_event(
        egui::Key::F8,
        true,
        true,
        egui::Modifiers::default(),
        &mut app,
    );
    assert!(
        !app.session.overlay_mode,
        "repeating F8 key event while held must not toggle overlay mode"
    );
    send_key_event(
        egui::Key::F8,
        false,
        false,
        egui::Modifiers::default(),
        &mut app,
    );
    app.session.set_overlay_mode(true);
    app.session.set_overlay_click_through(false);

    // Enqueue one global press -> toggles ON and updates banner.
    tx.send(super::hotkey::GlobalHotkeyAction::ToggleOverlayPass)
        .unwrap();
    let _ = render_at_viewport(&mut app, &ctx, viewport);
    assert!(
        app.session.overlay_geometry.click_through,
        "single queued global press must toggle overlay click-through ON"
    );
    let tr = super::i18n::strings(app.session.ui_language);
    assert_eq!(app.status_banner, tr.status_pass_enabled);

    // Enqueue two distinct global presses in one frame (`true -> false -> true`):
    // resetting `status_banner` proves neither 0 nor 1 execution can pass this assertion.
    app.status_banner = "sentinel".into();
    tx.send(super::hotkey::GlobalHotkeyAction::ToggleOverlayPass)
        .unwrap();
    tx.send(super::hotkey::GlobalHotkeyAction::ToggleOverlayPass)
        .unwrap();
    let _ = render_at_viewport(&mut app, &ctx, viewport);
    assert!(
        app.session.overlay_geometry.click_through,
        "two queued global presses in one frame must toggle twice (`true -> false -> true`) without dropping the second"
    );
    assert_eq!(app.status_banner, tr.status_pass_enabled);
}

#[test]
fn wayland_portal_shortcut_syntax_binding_status_and_cancel_handling() {
    use super::hotkey::{
        PORTAL_PREFERRED_TRIGGER, PortalLoopOutcome, portal_status_label,
        should_fallback_to_xwayland,
    };

    assert_eq!(PORTAL_PREFERRED_TRIGGER, "CTRL+SHIFT+p");
    assert_eq!(portal_status_label(None), "Ctrl+Shift+P (In-App)");
    assert_eq!(portal_status_label(Some("")), "Ctrl+Shift+P (In-App)");
    assert_eq!(portal_status_label(Some("   ")), "Ctrl+Shift+P (In-App)");
    assert_eq!(
        portal_status_label(Some("Meta+Shift+P")),
        "Meta+Shift+P (Global · Wayland Portal)"
    );

    // Explicit user cancellation (`response_code == 1`) must NEVER fall back to XWayland XGrabKey.
    assert!(!should_fallback_to_xwayland(
        &PortalLoopOutcome::Cancelled,
        true
    ));
    assert!(!should_fallback_to_xwayland(
        &PortalLoopOutcome::Finished,
        true
    ));
    assert!(should_fallback_to_xwayland(
        &PortalLoopOutcome::Unavailable("no portal".into()),
        true
    ));
    assert!(!should_fallback_to_xwayland(
        &PortalLoopOutcome::Unavailable("no portal".into()),
        false
    ));

    #[cfg(target_os = "linux")]
    {
        use super::hotkey::{extract_assigned_trigger_from_pairs, inspect_portal_shortcuts_value};
        use std::collections::HashMap;
        use zbus::zvariant::{OwnedValue, Value};

        let empty: Vec<(String, HashMap<String, Value<'_>>)> = Vec::new();
        let empty_val = Value::from(empty);
        assert_eq!(
            inspect_portal_shortcuts_value(&empty_val, "toggle_overlay_pass"),
            (false, None)
        );

        let mut blank_props: HashMap<String, Value<'_>> = HashMap::new();
        blank_props.insert("description".into(), Value::from("Toggle Overlay"));
        blank_props.insert(
            "trigger_description".into(),
            Value::Value(Box::new(Value::from("   "))),
        );
        let blank_val = Value::from(vec![("toggle_overlay_pass".to_string(), blank_props)]);
        let (listed, trigger) = inspect_portal_shortcuts_value(&blank_val, "toggle_overlay_pass");
        assert!(listed, "listed shortcut id should be detected");
        assert_eq!(
            trigger, None,
            "blank trigger_description must NOT report Global"
        );
        assert_eq!(
            portal_status_label(trigger.as_deref()),
            "Ctrl+Shift+P (In-App)"
        );

        let mut bound_props: HashMap<String, Value<'_>> = HashMap::new();
        bound_props.insert("description".into(), Value::from("Toggle Overlay"));
        bound_props.insert("trigger_description".into(), Value::from("Ctrl+Shift+P"));
        let bound_val = Value::Value(Box::new(Value::from(vec![(
            "toggle_overlay_pass".to_string(),
            bound_props,
        )])));
        assert_eq!(
            inspect_portal_shortcuts_value(&bound_val, "toggle_overlay_pass"),
            (true, Some("Ctrl+Shift+P".to_string()))
        );

        let mut changed_bound: HashMap<String, OwnedValue> = HashMap::new();
        changed_bound.insert(
            "trigger_description".into(),
            OwnedValue::try_from(Value::from("Meta+P")).unwrap(),
        );
        let changed_pairs = vec![("toggle_overlay_pass".to_string(), changed_bound)];
        assert_eq!(
            extract_assigned_trigger_from_pairs(&changed_pairs, "toggle_overlay_pass"),
            Some("Meta+P".to_string())
        );

        let mut changed_cleared: HashMap<String, OwnedValue> = HashMap::new();
        changed_cleared.insert(
            "trigger_description".into(),
            OwnedValue::try_from(Value::from("")).unwrap(),
        );
        let cleared_pairs = vec![("toggle_overlay_pass".to_string(), changed_cleared)];
        assert_eq!(
            extract_assigned_trigger_from_pairs(&cleared_pairs, "toggle_overlay_pass"),
            None
        );
    }
}

#[test]
fn gui_build_bar_renders_three_timed_phases_and_role_filtered_optional_section() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 4 nyx assassin").unwrap());
    assert!(execute_command(&mut session, "enemy 1 phantom assassin").unwrap());
    assert!(execute_command(&mut session, "enemy 3 riki").unwrap());
    assert!(execute_command(&mut session, "enemy 5 shadow shaman").unwrap());
    session.draft.target_role = Role::SoftSupport;
    session.local_player_hero = Some(HeroId(88));
    session.toggle_post_draft_build_view();
    assert!(session.is_post_draft_build_view());

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.onboarding_open = false;
    let ctx = egui::Context::default();

    // Check full desktop 3-column integration (1468x960) plus 2+1 (460x760) & stacked (320x860)
    // responsive breakpoints directly on the build card.
    let (r, desktop_out) = render_at_viewport_with_output(&mut app, &ctx, vec2(1468.0, 960.0));
    assert!(
        r.suggested.width() > 0.0 && r.suggested.height() > 0.0,
        "build card must occupy non-zero rect on desktop"
    );
    assert!(
        r.matrix.width() >= 180.0 && r.suggested.max.x <= r.matrix.min.x + 2.0,
        "desktop build card {:?} must stay strictly left of Counter Matrix {:?} without pushing it off-screen",
        r.suggested,
        r.matrix
    );
    fn find_painted_text_bounds(shape: &egui::epaint::Shape, target: &str) -> Option<Rect> {
        match shape {
            egui::epaint::Shape::Text(ts) if ts.galley.text() == target => {
                Some(ts.visual_bounding_rect())
            }
            egui::epaint::Shape::Vec(v) => {
                v.iter().find_map(|s| find_painted_text_bounds(s, target))
            }
            _ => None,
        }
    }
    let mut checked_labels = 0_usize;
    for label in ["Arcane Boots", "Phylactery"] {
        if let Some(bounds) = desktop_out
            .shapes
            .iter()
            .filter(|c| r.suggested.intersects(c.clip_rect))
            .find_map(|c| find_painted_text_bounds(&c.shape, label))
        {
            checked_labels += 1;
            assert!(
                bounds.height() <= 28.0 && bounds.width() >= 30.0,
                "phase item label '{label}' wrapped vertically into narrow column: {bounds:?}"
            );
        }
    }
    assert!(
        checked_labels >= 2,
        "desktop build card must render readable primary item labels without clipping ({checked_labels})"
    );
    let mut outputs = vec![(vec2(1468.0, 960.0), desktop_out)];
    for card_vp in [vec2(460.0, 760.0), vec2(320.0, 860.0)] {
        let card_out = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), card_vp)),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let _ = app.render_item_build_card(ui);
                    });
            },
        );
        outputs.push((card_vp, card_out));
    }
    for (vp, out) in outputs {
        let mut texts = Vec::new();
        for c in &out.shapes {
            collect_painted_texts(&c.shape, &mut texts);
        }
        assert!(texts.iter().any(|t| t == "EARLY"), "{vp:?}: {texts:?}");
        assert!(texts.iter().any(|t| t == "CORE"), "{vp:?}: {texts:?}");
        assert!(texts.iter().any(|t| t == "LATE"), "{vp:?}: {texts:?}");
        assert!(
            texts.iter().any(|t| t == "OPTIONAL / COUNTERS"),
            "{vp:?}: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t == "Lotus Orb" || t == "Dust of Appearance"),
            "Pos 4 Nyx vs Shaman + Riki should display role-eligible Optional counters at {vp:?}: {texts:?}"
        );
        for forbidden in [
            "Pipe of Insight",
            "Battle Fury",
            "Monkey King Bar",
            "Eye of Skadi",
        ] {
            assert!(
                !texts.iter().any(|t| t == forbidden),
                "Pos 4 Nyx must not display carry/offlane counter '{forbidden}' at {vp:?}"
            );
        }
    }
}

#[test]
fn gui_overlay_switches_independent_picks_and_build_geometries_and_renders_icon_only_build() {
    let mut session = make_5v5_gui_session();
    assert!(execute_command(&mut session, "enemy 4 riki").unwrap());
    session.overlay_mode = true;
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 60.0,
        y: 140.0,
        width: 420.0,
        height: 260.0,
        click_through: false,
    };
    session.build_overlay_geometry = crate::session::OverlayGeometry {
        x: 80.0,
        y: 160.0,
        width: 500.0,
        height: 142.0,
        click_through: false,
    };

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.onboarding_open = false;
    app.native_monitors_probed = true;
    let ctx = egui::Context::default();

    let overlay_id = egui::ViewportId::from_hash_of("truesight_compact_overlay");
    let picks_vp = egui::ViewportInfo {
        monitor_size: Some(vec2(1920.0, 1080.0)),
        outer_rect: Some(Rect::from_min_size(pos2(60.0, 140.0), vec2(420.0, 260.0))),
        inner_rect: Some(Rect::from_min_size(pos2(60.0, 140.0), vec2(420.0, 260.0))),
        ..Default::default()
    };
    let mut raw_picks = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1180.0, 700.0))),
        ..Default::default()
    };
    raw_picks
        .viewports
        .insert(egui::ViewportId::ROOT, picks_vp.clone());
    raw_picks.viewports.insert(overlay_id, picks_vp);

    // 1. Frame in Picks mode initializes last_overlay_is_post_draft = Some(false).
    let _ = ctx.run(raw_picks.clone(), |ctx| {
        let _ = app.render_frame(ctx);
    });
    assert_eq!(app.last_overlay_is_post_draft, Some(false));
    assert_eq!(app.session.overlay_geometry.width, 420.0);

    // 2. Toggle to Build mode: must preserve Picks 420x260 and command Build 500x142 + min 220x104
    // on the immediate transition frame.
    app.session.toggle_post_draft_build_view();
    let switch_out = ctx.run(raw_picks, |ctx| {
        let _ = app.render_frame(ctx);
    });
    let cmds = &switch_out
        .viewport_output
        .get(&overlay_id)
        .or_else(|| switch_out.viewport_output.get(&egui::ViewportId::ROOT))
        .expect("overlay viewport output")
        .commands;
    assert!(
        cmds.iter()
            .any(|c| matches!(c, egui::ViewportCommand::MinInnerSize(s) if (s.x - 220.0).abs() < 0.5 && (s.y - 104.0).abs() < 0.5)),
        "Build overlay transition must relax min inner size to 220x104: {cmds:?}"
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, egui::ViewportCommand::InnerSize(s) if (s.x - 500.0).abs() < 0.5 && (s.y - 142.0).abs() < 0.5)),
        "Build overlay transition must apply saved Build dimensions 500x142: {cmds:?}"
    );
    assert_eq!(app.session.overlay_geometry.width, 420.0);
    assert_eq!(app.session.overlay_geometry.height, 260.0);

    // 3. Direct responsive checks of icon-only Build overlay at 240x110, 390x136, and 540x180.
    for vp in [vec2(240.0, 110.0), vec2(390.0, 136.0), vec2(540.0, 180.0)] {
        let mut out_opt = None;
        let screen_rect = Rect::from_min_size(pos2(0.0, 0.0), vp);
        let full = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            },
            |vctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(vctx, |ui| {
                        out_opt = Some(app.render_overlay_panel(ui));
                    });
            },
        );
        let panel_out = out_opt.expect("overlay panel output");
        assert!(
            screen_rect.contains_rect(panel_out.resize_handle),
            "resize handle {:?} must stay inside Build overlay viewport {vp:?}",
            panel_out.resize_handle
        );
        let mut texts = Vec::new();
        for c in &full.shapes {
            collect_painted_texts(&c.shape, &mut texts);
        }
        assert!(
            texts.iter().any(|t| t.ends_with('m') && t.len() <= 4),
            "Build overlay at {vp:?} must paint overlaid timing pills (e.g. '7m'); got {texts:?}"
        );
        if vp.y >= 130.0 {
            assert!(
                texts.iter().any(|t| t == "OPT"),
                "Build overlay at {vp:?} with Riki enemy must render compact OPT strip; got {texts:?}"
            );
        }
    }
}

#[test]
fn gui_clicking_second_ally_sets_build_view_hero_without_setting_local_player_hero() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 anti-mage").unwrap());
    assert!(execute_command(&mut session, "ally 4 nyx assassin").unwrap());
    session.local_player_hero = None;
    session.local_player_hero_from_gsi = false;

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.onboarding_open = false;
    let ctx = egui::Context::default();
    let viewport = vec2(1468.0, 960.0);
    let r = render_at_viewport(&mut app, &ctx, viewport);
    let slot4 = &r.ally_slots[3];
    let click_pos = pos2(slot4.row.min.x + 24.0, slot4.row.center().y);

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
            let _ = app.render_frame(ctx);
        });
    }

    assert_eq!(
        app.session.build_view_hero,
        Some(HeroId(88)),
        "clicking ally P4 (Nyx) must select build_view_hero"
    );
    assert_eq!(
        app.session.local_player_hero, None,
        "clicking an ally without GSI identity must never assign local_player_hero"
    );
}
