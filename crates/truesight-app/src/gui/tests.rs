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

fn run_test_ui(
    ctx: &egui::Context,
    raw_input: egui::RawInput,
    run_ui: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    let mut output = ctx.run_ui(raw_input, run_ui);
    output.textures_delta.clear();
    output
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
        let output = run_test_ui(ctx, raw_input.clone(), |ui| {
            layout = Some(app.render_frame(ui));
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
        let _ = run_test_ui(&ctx, input, |ctx| {
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
    let center_green_max = icon
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| px[1])
        .max()
        .unwrap_or(0);
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
        let _ = run_test_ui(&ctx, input, |ctx| {
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
        let _ = run_test_ui(&ctx, input, |ctx| {
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
        let _ = run_test_ui(&ctx, input, |ctx| {
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
    let _ = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
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
    let _ = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                egui::vec2(1180.0, 700.0),
            )),
            ..Default::default()
        },
        |ui| {
            let rects = app.render_frame(ui);
            suggested_rect = rects.suggested;
        },
    );
    assert!(suggested_rect.width() > 0.0 && suggested_rect.height() > 0.0);

    let _ = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                egui::vec2(380.0, 230.0),
            )),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
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

fn assert_overlay_labels_single_line_in_viewport(
    shape: &egui::epaint::Shape,
    vp: egui::Vec2,
    targets: &[&str],
) {
    match shape {
        egui::epaint::Shape::Text(ts) => {
            let txt = ts.galley.text();
            if targets.contains(&txt) {
                assert_eq!(
                    ts.galley.rows.len(),
                    1,
                    "Label '{txt}' at {vp:?} must stay single-line (not vertically wrapped), got {} rows",
                    ts.galley.rows.len()
                );
                assert!(
                    ts.pos.y < vp.y,
                    "Label '{txt}' pos.y ({}) must stay inside viewport height {}",
                    ts.pos.y,
                    vp.y
                );
            }
        }
        egui::epaint::Shape::Vec(shapes) => {
            for inner in shapes {
                assert_overlay_labels_single_line_in_viewport(inner, vp, targets);
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

    let full_output = run_test_ui(&ctx, raw_input, |ui| {
        app.sync_live_overlay_viewport_geometry(ui.ctx());
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
        full_output = Some(run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    pos2(0.0, 0.0),
                    egui::vec2(380.0, 230.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
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

    let secondary_output = run_test_ui(&ctx, raw_input_secondary, |ui| {
        app.sync_live_overlay_viewport_geometry(ui.ctx());
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

    let strip_output = run_test_ui(&ctx, raw_input_strip, |ui| {
        app.sync_live_overlay_viewport_geometry(ui.ctx());
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

    let _ = run_test_ui(&ctx, make_input(2450.0, 140.0), |ui| {
        app.sync_live_overlay_viewport_geometry(ui.ctx());
    });
    assert_eq!(
        app.session.overlay_geometry.x, 2450.0,
        "valid 380pt overlay at x=2450 on 1280pt secondary monitor (1920..3200) must not be moved to 2180"
    );
    assert_eq!(app.session.overlay_geometry.y, 140.0);

    // Dragging past the right edge of the 1280pt secondary monitor (x=3000) clamps to 1920 + 1280 - 380 = 2820.
    let _ = run_test_ui(&ctx, make_input(3000.0, 140.0), |ui| {
        app.sync_live_overlay_viewport_geometry(ui.ctx());
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
    let _ = run_test_ui(&ctx, make_input(2450.0, 140.0), |ui| {
        fallback_app.sync_live_overlay_viewport_geometry(ui.ctx());
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

    let _ = run_test_ui(&ctx, raw_input, |ui| {
        let _ = app.render_frame(ui);
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

    let _ = run_test_ui(&ctx, make_input(1400.0, 120.0), |ui| {
        let _ = app.render_frame(ui);
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
    let _ = run_test_ui(&ctx, make_input(2800.0, 168.0), |ui| {
        let _ = app.render_frame(ui);
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
    let _ = run_test_ui(&ctx, egui::RawInput::default(), |_| {});
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

    let _ = run_test_ui(&ctx, make_input(1200.0, 100.0), |ui| {
        let _ = app.render_frame(ui);
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
    let _ = run_test_ui(&ctx, make_input(2500.0, 140.0), |ui| {
        let _ = app.render_frame(ui);
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
            let full_output = run_test_ui(
                &ctx,
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vp_size)),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
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
        "Ctrl+Shift+T (Test Global)",
    ));

    let ctx = egui::Context::default();
    let viewport = vec2(1280.0, 800.0);

    let click_at = |pos: egui::Pos2, app: &mut TrueSightGuiApp| {
        for pressed in [true, false] {
            let _ = run_test_ui(
                &ctx,
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
                |ui| {
                    app.render_frame(ui);
                },
            );
        }
    };

    let press_key = |key: egui::Key, modifiers: egui::Modifiers, app: &mut TrueSightGuiApp| {
        for pressed in [true, false] {
            let _ = run_test_ui(
                &ctx,
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    app.render_frame(ui);
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

    // 3. Pressing Ctrl+Shift+T toggles click-through ON then back OFF without double-toggling.
    let ctrl_shift = egui::Modifiers {
        ctrl: true,
        shift: true,
        ..Default::default()
    };
    press_key(egui::Key::T, ctrl_shift, &mut app);
    assert!(
        app.session.overlay_geometry.click_through,
        "Ctrl+Shift+T must enable click-through without child viewport double-toggling"
    );
    press_key(egui::Key::T, ctrl_shift, &mut app);
    assert!(
        !app.session.overlay_geometry.click_through,
        "second Ctrl+Shift+T press must disable click-through"
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
            let _ = run_test_ui(
                &ctx,
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
                |ui| {
                    app.render_frame(ui);
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
        let full = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen_rect),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
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

    let full_output = run_test_ui(&ctx, raw_input, |ui| {
        app.sync_live_overlay_viewport_geometry(ui.ctx());
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
        "Ctrl+Shift+T (Test Global)",
    ));

    let ctx = egui::Context::default();
    let viewport = vec2(1280.0, 800.0);
    let send_key_event = |key: egui::Key,
                          pressed: bool,
                          repeat: bool,
                          modifiers: egui::Modifiers,
                          app: &mut TrueSightGuiApp| {
        let _ = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat,
                    modifiers,
                }],
                ..Default::default()
            },
            |ui| {
                app.render_frame(ui);
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
    send_key_event(egui::Key::T, true, false, ctrl_shift, &mut app);
    assert!(
        app.session.overlay_geometry.click_through,
        "initial Ctrl+Shift+T press must toggle overlay click-through ON"
    );
    send_key_event(egui::Key::T, true, true, ctrl_shift, &mut app);
    assert!(
        app.session.overlay_geometry.click_through,
        "repeating Ctrl+Shift+T key event while held must not toggle overlay click-through"
    );
    send_key_event(egui::Key::T, false, false, ctrl_shift, &mut app);

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

    assert_eq!(PORTAL_PREFERRED_TRIGGER, "CTRL+SHIFT+t");
    assert_eq!(
        portal_status_label(None, "Ctrl+Shift+T"),
        "Ctrl+Shift+T (In-App)"
    );
    assert_eq!(
        portal_status_label(Some(""), "Ctrl+Shift+T"),
        "Ctrl+Shift+T (In-App)"
    );
    assert_eq!(
        portal_status_label(Some("   "), "Ctrl+Shift+T"),
        "Ctrl+Shift+T (In-App)"
    );
    assert_eq!(
        portal_status_label(Some("Meta+Shift+T"), "Ctrl+Shift+T"),
        "Meta+Shift+T (Global · Wayland Portal)"
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
            portal_status_label(trigger.as_deref(), "Ctrl+Shift+T"),
            "Ctrl+Shift+T (In-App)"
        );

        let mut bound_props: HashMap<String, Value<'_>> = HashMap::new();
        bound_props.insert("description".into(), Value::from("Toggle Overlay"));
        bound_props.insert("trigger_description".into(), Value::from("Ctrl+Shift+T"));
        let bound_val = Value::Value(Box::new(Value::from(vec![(
            "toggle_overlay_pass".to_string(),
            bound_props,
        )])));
        assert_eq!(
            inspect_portal_shortcuts_value(&bound_val, "toggle_overlay_pass"),
            (true, Some("Ctrl+Shift+T".to_string()))
        );

        let mut changed_bound: HashMap<String, OwnedValue> = HashMap::new();
        changed_bound.insert(
            "trigger_description".into(),
            OwnedValue::try_from(Value::from("Meta+T")).unwrap(),
        );
        let changed_pairs = vec![("toggle_overlay_pass".to_string(), changed_bound)];
        assert_eq!(
            extract_assigned_trigger_from_pairs(&changed_pairs, "toggle_overlay_pass"),
            Some("Meta+T".to_string())
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
        let card_out = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), card_vp)),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
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
    let _ = run_test_ui(&ctx, raw_picks.clone(), |ui| {
        let _ = app.render_frame(ui);
    });
    assert_eq!(app.last_overlay_is_post_draft, Some(false));
    assert_eq!(app.session.overlay_geometry.width, 420.0);

    // 2. Toggle to Build mode: must preserve Picks 420x260 and command Build 500x142 + min 220x104
    // on the immediate transition frame.
    app.session.toggle_post_draft_build_view();
    let switch_out = run_test_ui(&ctx, raw_picks, |ui| {
        let _ = app.render_frame(ui);
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

    // 3. Direct responsive checks of phased Build overlay across compact -> expanded viewports.
    for vp in [
        vec2(240.0, 110.0),
        vec2(390.0, 136.0),
        vec2(420.0, 168.0),
        vec2(540.0, 190.0),
    ] {
        let mut out_opt = None;
        let screen_rect = Rect::from_min_size(pos2(0.0, 0.0), vp);
        let full = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
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
            assert_overlay_labels_single_line_in_viewport(
                &c.shape,
                vp,
                &["EARLY", "CORE", "LATE", "START", "OPT", "OPT · CNT"],
            );
        }
        assert!(
            texts.iter().any(|t| t == "EARLY") && texts.iter().any(|t| t == "CORE"),
            "Build overlay at {vp:?} must render framed phase headers (EARLY/CORE): {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.ends_with('m') && t.len() <= 4),
            "Build overlay at {vp:?} must paint overlaid timing pills (e.g. '7m'); got {texts:?}"
        );
        if vp.y >= 136.0 {
            assert!(
                texts.iter().any(|t| t == "OPT" || t == "OPT · CNT"),
                "Build overlay at {vp:?} with Riki enemy must render framed OPT counter bar; got {texts:?}"
            );
            assert!(
                texts.iter().any(|t| t == "START"),
                "Build overlay at {vp:?} must display starting items (START); got {texts:?}"
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
        let _ = run_test_ui(&ctx, input, |ctx| {
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

fn find_all_painted_text_rects(shape: &egui::epaint::Shape, target: &str, out: &mut Vec<Rect>) {
    match shape {
        egui::epaint::Shape::Text(ts) if ts.galley.text() == target => {
            out.push(ts.visual_bounding_rect());
        }
        egui::epaint::Shape::Vec(v) => {
            for s in v {
                find_all_painted_text_rects(s, target, out);
            }
        }
        _ => {}
    }
}

fn make_four_late_steps_mock_repo() -> truesight_adapters::item_repo::SeededItemMetaRepository {
    use truesight_core::domain::{HeroItemStat, ItemPhase};
    let mut repo = truesight_adapters::item_repo::SeededItemMetaRepository::new();
    repo.custom_hero_stats.push((
        HeroId(41),
        Some(Role::Carry),
        None,
        vec![
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(11),
                phase: ItemPhase::Starting,
                avg_time_mins: 0.0,
                match_count: 1000,
                win_count: 520,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(75),
                phase: ItemPhase::Early,
                avg_time_mins: 6.0,
                match_count: 620,
                win_count: 320,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(63),
                phase: ItemPhase::Early,
                avg_time_mins: 9.0,
                match_count: 980,
                win_count: 520,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(50),
                phase: ItemPhase::Early,
                avg_time_mins: 8.5,
                match_count: 600,
                win_count: 320,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(180),
                phase: ItemPhase::Early,
                avg_time_mins: 8.5,
                match_count: 500,
                win_count: 265,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(214),
                phase: ItemPhase::Early,
                avg_time_mins: 8.5,
                match_count: 450,
                win_count: 238,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(166),
                phase: ItemPhase::Core,
                avg_time_mins: 15.0,
                match_count: 880,
                win_count: 480,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(147),
                phase: ItemPhase::Core,
                avg_time_mins: 21.0,
                match_count: 840,
                win_count: 470,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(158),
                phase: ItemPhase::Late,
                avg_time_mins: 26.0,
                match_count: 760,
                win_count: 440,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(139),
                phase: ItemPhase::Late,
                avg_time_mins: 28.5,
                match_count: 690,
                win_count: 405,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(160),
                phase: ItemPhase::Late,
                avg_time_mins: 31.0,
                match_count: 620,
                win_count: 370,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(141),
                phase: ItemPhase::Late,
                avg_time_mins: 33.5,
                match_count: 580,
                win_count: 350,
            },
            // Extra Late candidates attaching as 3+ variants under Late primaries.
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(156),
                phase: ItemPhase::Late,
                avg_time_mins: 35.0,
                match_count: 540,
                win_count: 325,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(135),
                phase: ItemPhase::Late,
                avg_time_mins: 36.0,
                match_count: 500,
                win_count: 300,
            },
            HeroItemStat {
                item_id: truesight_core::domain::ItemId(116),
                phase: ItemPhase::Late,
                avg_time_mins: 27.0,
                match_count: 490,
                win_count: 295,
            },
        ],
    ));
    repo
}

#[test]
fn paint_item_icon_fallback_draws_monogram_when_texture_is_absent() {
    use super::theme::{P_LINE, item_monogram, paint_item_icon_with_fallback};

    assert_eq!(item_monogram("Sabatons of Mordiggian"), "SM");
    assert_eq!(item_monogram("Blink"), "BL");
    assert_eq!(item_monogram("#1999"), "19");
    assert_eq!(item_monogram("Ristul's Devotion"), "RD");

    let ctx = egui::Context::default();
    let out = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 80.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(36.0, 26.0), egui::Sense::hover());
                paint_item_icon_with_fallback(
                    ui.painter(),
                    rect,
                    None,
                    Some("Sabatons of Mordiggian"),
                    P_LINE,
                    false,
                );
            });
        },
    );

    let mut texts = Vec::new();
    for c in &out.shapes {
        collect_painted_texts(&c.shape, &mut texts);
    }
    assert!(
        texts.iter().any(|t| t == "SM"),
        "missing texture must render uppercase monogram 'SM': {texts:?}"
    );
}

#[test]
fn build_bar_renders_four_late_items_in_2x2_grid_on_desktop() {
    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
    session.item_repo = make_four_late_steps_mock_repo();
    session.toggle_post_draft_build_view();

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.onboarding_open = false;
    let ctx = egui::Context::default();
    let out = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(920.0, 420.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let _ = app.render_item_build_card(ui);
                });
        },
    );

    let mut texts = Vec::new();
    for c in &out.shapes {
        collect_painted_texts(&c.shape, &mut texts);
    }
    for name in ["Mjollnir", "Butterfly", "Eye of Skadi", "Daedalus"] {
        assert!(
            texts.iter().any(|t| t == name),
            "2x2 Late grid must render '{name}': {texts:?}"
        );
    }

    let rect_of = |label: &str| {
        let mut rects = Vec::new();
        for c in &out.shapes {
            find_all_painted_text_rects(&c.shape, label, &mut rects);
        }
        rects.into_iter().next().expect(label)
    };
    let r_mjollnir = rect_of("Mjollnir");
    let r_butter = rect_of("Butterfly");
    let r_skadi = rect_of("Eye of Skadi");
    let r_daed = rect_of("Daedalus");

    // Items 0 & 1 occupy row 1 cleanly side by side; items 2 & 3 sit below on row 2.
    assert!(
        (r_mjollnir.min.y - r_butter.min.y).abs() < 4.0 && r_butter.min.x > r_mjollnir.max.x,
        "Mjollnir ({r_mjollnir:?}) and Butterfly ({r_butter:?}) must share Late row 1"
    );
    assert!(
        r_skadi.min.y > r_mjollnir.max.y + 8.0 && (r_skadi.min.y - r_daed.min.y).abs() < 4.0,
        "Eye of Skadi ({r_skadi:?}) and Daedalus ({r_daed:?}) must share Late row 2 below row 1"
    );
}

#[test]
fn overlay_build_switches_late_between_4x1_wide_and_2x2_tall_and_wraps_three_variants() {
    use super::overlay_build::{max_overlay_variant_rows, select_overlay_late_grid_2x2};

    // Pure geometry switch checks
    assert!(!select_overlay_late_grid_2x2(548.0, 90.0, 4));
    assert!(select_overlay_late_grid_2x2(408.0, 204.0, 4));
    assert!(max_overlay_variant_rows(90.0, 32.0, 15.0) >= 2);

    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
    session.item_repo = make_four_late_steps_mock_repo();
    session.overlay_mode = true;
    session.toggle_post_draft_build_view();

    let build = session.compute_recommended_build().unwrap();
    let max_vars = build
        .steps
        .iter()
        .map(|s| s.variants.len())
        .max()
        .unwrap_or(0);
    assert!(
        max_vars >= 3,
        "fixture build should have a step with 3 variants for wrap check: {:?}",
        build.steps
    );

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.onboarding_open = false;
    let ctx = egui::Context::default();

    let collect_late_timing_rects = |full: &egui::FullOutput| {
        let mut out = Vec::new();
        for t in ["26m", "29m", "31m", "34m"] {
            let mut rects = Vec::new();
            for c in &full.shapes {
                find_all_painted_text_rects(&c.shape, t, &mut rects);
            }
            out.push((t, rects));
        }
        out
    };

    // 1. Wide/low 560x145 -> 4x1 horizontal Late row (all 4 share similar Y).
    let wide_vp = vec2(560.0, 145.0);
    let wide_out = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), wide_vp)),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let _ = app.render_overlay_panel(ui);
                });
        },
    );
    let wide_pills = collect_late_timing_rects(&wide_out);
    assert!(
        wide_pills.iter().all(|(_, r)| !r.is_empty()),
        "4x1 wide overlay must paint all 4 Late timing pills: {wide_pills:?}"
    );
    let y0 = wide_pills[0].1[0].min.y;
    for (lbl, rects) in &wide_pills {
        assert!(
            (rects[0].min.y - y0).abs() < 4.0,
            "4x1 wide overlay must keep Late item '{lbl}' on one row (y0={y0}, got={:?})",
            rects[0]
        );
    }

    // 2. Taller/compact 420x270 -> 2x2 Late grid + >= 2 variant rows (>= 3 variant icons rendered).
    let tall_vp = vec2(420.0, 270.0);
    let tall_out = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), tall_vp)),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let _ = app.render_overlay_panel(ui);
                });
        },
    );
    let tall_pills = collect_late_timing_rects(&tall_out);
    let row1_y = tall_pills[0].1[0].min.y.min(tall_pills[1].1[0].min.y);
    let row2_y = tall_pills[2].1[0].min.y.min(tall_pills[3].1[0].min.y);
    assert!(
        row2_y > row1_y + 18.0,
        "2x2 tall overlay must place items 3-4 below items 1-2: {tall_pills:?}"
    );
    fn count_build_textures(shape: &egui::epaint::Shape) -> usize {
        match shape {
            egui::epaint::Shape::Mesh(m)
                if !matches!(m.texture_id, egui::TextureId::Managed(0)) =>
            {
                1
            }
            egui::epaint::Shape::Vec(v) => v.iter().map(count_build_textures).sum(),
            _ => 0,
        }
    }

    // Count item texture meshes across overlay: 1 starting + 8 primaries + 3 variants = 12+ textures
    // (previously capped at 1 + 6 + 2 = 9).
    let tex_count: usize = tall_out
        .shapes
        .iter()
        .map(|c| count_build_textures(&c.shape))
        .sum();
    assert!(
        tex_count >= 12,
        "expanded tall overlay must render all 8 primaries + >=3 multi-row variants (+starting), got {tex_count} textures"
    );
}

#[test]
fn build_bar_and_overlay_label_blessing_upgrade_and_highlight_child_when_scepter_owned() {
    use super::build_bar::variant_chip_label_and_highlight;
    use truesight_adapters::item_repo::SeededItemMetaRepository;
    use truesight_core::domain::{HeroItemStat, ItemCatalogEntry, ItemId, ItemPhase};

    assert_eq!(
        variant_chip_label_and_highlight("Eye of Skadi", false, true, false),
        ("Eye of Skadi".to_string(), false)
    );
    assert_eq!(
        variant_chip_label_and_highlight("Aghanim's Blessing", true, false, false),
        ("UPG · Aghanim's Blessing".to_string(), false)
    );
    assert_eq!(
        variant_chip_label_and_highlight("Aghanim's Blessing", true, true, false),
        ("UPG READY · Aghanim's Blessing".to_string(), true)
    );

    let make_repo = || SeededItemMetaRepository {
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

    let render_texts = |owned_scepter: bool| {
        let mut session = AppSession::new().unwrap();
        assert!(execute_command(&mut session, "ally 5 crystal maiden").unwrap());
        session.local_player_hero = Some(HeroId(5));
        session.owned_items_owner = Some(HeroId(5));
        session.item_repo = make_repo();
        session.owned_items = if owned_scepter {
            vec![ItemId(214), ItemId(108)]
        } else {
            vec![ItemId(214)]
        };
        session.toggle_post_draft_build_view();

        let mut app = TrueSightGuiApp::new(session, None, None);
        app.onboarding_open = false;
        let ctx = egui::Context::default();
        let desk_out = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(920.0, 420.0))),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        let _ = app.render_item_build_card(ui);
                    });
            },
        );
        let mut desk_texts = Vec::new();
        for c in &desk_out.shapes {
            collect_painted_texts(&c.shape, &mut desk_texts);
        }

        app.session.overlay_mode = true;
        let ovl_out = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(520.0, 240.0))),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        let _ = app.render_overlay_panel(ui);
                    });
            },
        );
        let mut ovl_texts = Vec::new();
        for c in &ovl_out.shapes {
            collect_painted_texts(&c.shape, &mut ovl_texts);
        }
        (desk_texts, ovl_texts)
    };

    let (desk_unowned, ovl_unowned) = render_texts(false);
    assert!(
        desk_unowned.iter().any(|t| t == "UPG · Aghanim's Blessing"),
        "desktop build bar must label Blessing as an upgrade before Scepter is owned: {desk_unowned:?}"
    );
    assert!(
        ovl_unowned.iter().any(|t| t == "UPG"),
        "overlay child upgrade must display micro UPG badge: {ovl_unowned:?}"
    );

    let (desk_owned, ovl_owned) = render_texts(true);
    assert!(
        desk_owned
            .iter()
            .any(|t| t == "UPG READY · Aghanim's Blessing"),
        "desktop build bar must highlight Blessing upgrade availability below owned Scepter: {desk_owned:?}"
    );
    assert!(
        desk_owned.iter().filter(|t| t.as_str() == "OK").count() >= 2,
        "owned Tranquil Boots + owned Scepter cards must remain marked OK: {desk_owned:?}"
    );
    assert!(
        ovl_owned.iter().any(|t| t == "UPG"),
        "overlay must continue labeling Blessing child tile when Scepter is owned: {ovl_owned:?}"
    );
}

#[test]
fn direct_upgrade_chip_in_build_bar_and_overlay_is_informational_and_preserves_owned_scepter() {
    use truesight_adapters::item_repo::SeededItemMetaRepository;
    use truesight_core::domain::{HeroItemStat, ItemCatalogEntry, ItemId, ItemPhase};
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
    session.toggle_post_draft_build_view();

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.onboarding_open = false;

    let ctx = egui::Context::default();
    let desk_vp = vec2(920.0, 420.0);
    let run_desktop = |app: &mut TrueSightGuiApp, events: Vec<egui::Event>| {
        run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), desk_vp)),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        let _ = app.render_item_build_card(ui);
                    });
            },
        )
    };

    // Pass 1: locate the desktop `UPG READY · Aghanim's Blessing` chip and `Arcane Boots` chip.
    let desk_init = run_desktop(&mut app, Vec::new());
    let mut upg_rects = Vec::new();
    let mut arcane_rects = Vec::new();
    for c in &desk_init.shapes {
        find_all_painted_text_rects(&c.shape, "UPG READY · Aghanim's Blessing", &mut upg_rects);
        find_all_painted_text_rects(&c.shape, "Arcane Boots", &mut arcane_rects);
    }
    let upg_center = upg_rects
        .first()
        .expect("desktop build bar must render UPG READY · Aghanim's Blessing chip")
        .center();
    let arcane_center = arcane_rects
        .first()
        .expect("desktop build bar must render Arcane Boots alternative chip")
        .center();

    // Pass 2: click the desktop `UPG READY · Aghanim's Blessing` chip -> Scepter stays owned primary!
    for pressed in [true, false] {
        let _ = run_desktop(
            &mut app,
            vec![
                egui::Event::PointerMoved(upg_center),
                egui::Event::PointerButton {
                    pos: upg_center,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
    }
    let after_desk_upg = app.session.compute_recommended_build().unwrap();
    let desk_scepter = after_desk_upg
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(108))
        .expect("clicking UPG READY chip in desktop build bar must preserve owned Scepter primary");
    assert!(desk_scepter.owned);
    assert_eq!(
        desk_scepter.variants.first().map(|v| v.item_id),
        Some(ItemId(271))
    );
    assert_eq!(
        after_desk_upg
            .next_unowned_index
            .map(|idx| after_desk_upg.steps[idx].item_id),
        Some(ItemId(256)),
        "clicking UPG READY chip must keep NEXT on Aeon Disk"
    );

    // Pass 3: click the `UPG` micro tile in the overlay -> owned Scepter & NEXT stay unchanged!
    app.session.overlay_mode = true;
    let ovl_vp = vec2(520.0, 240.0);
    let run_overlay = |app: &mut TrueSightGuiApp, events: Vec<egui::Event>| {
        run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), ovl_vp)),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        let _ = app.render_overlay_panel(ui);
                    });
            },
        )
    };
    let ovl_init = run_overlay(&mut app, Vec::new());
    let mut ovl_upg_rects = Vec::new();
    for c in &ovl_init.shapes {
        find_all_painted_text_rects(&c.shape, "UPG", &mut ovl_upg_rects);
    }
    let ovl_upg_center = ovl_upg_rects
        .first()
        .expect("overlay must paint UPG pill over Blessing child tile")
        .center();
    for pressed in [true, false] {
        let _ = run_overlay(
            &mut app,
            vec![
                egui::Event::PointerMoved(ovl_upg_center),
                egui::Event::PointerButton {
                    pos: ovl_upg_center,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
    }
    let after_ovl_upg = app.session.compute_recommended_build().unwrap();
    let ovl_scepter = after_ovl_upg
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(108))
        .expect("clicking UPG tile in overlay must preserve owned Scepter primary");
    assert!(ovl_scepter.owned);
    assert_eq!(
        after_ovl_upg
            .next_unowned_index
            .map(|idx| after_ovl_upg.steps[idx].item_id),
        Some(ItemId(256))
    );

    // Pass 4: clicking ordinary alternative `Arcane Boots` in desktop build bar still swaps cleanly!
    app.session.overlay_mode = false;
    let _ = run_desktop(&mut app, Vec::new());
    for pressed in [true, false] {
        let _ = run_desktop(
            &mut app,
            vec![
                egui::Event::PointerMoved(arcane_center),
                egui::Event::PointerButton {
                    pos: arcane_center,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
    }
    let after_alt_click = app.session.compute_recommended_build().unwrap();
    assert!(
        after_alt_click
            .steps
            .iter()
            .any(|s| s.item_id == ItemId(180)),
        "clicking ordinary alternative chip (Arcane Boots) must still swap into primary slot: {:?}",
        after_alt_click.steps
    );
}

#[test]
fn live_overlay_viewport_suppresses_corrective_commands_during_active_wm_resize_and_settles() {
    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.set_known_monitors(vec![crate::session::MonitorRect::new(
        0.0, 0.0, 1920.0, 1080.0,
    )]);
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 1500.0,
        y: 780.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();

    // Simulate BeginResize(SouthEast) having armed WM modal resize (pointer cleared by egui).
    app.overlay_viewport_sync.begin_wm_gesture();

    let run_sync_tick_with_events =
        |app: &mut TrueSightGuiApp, x: f32, y: f32, w: f32, h: f32, events: Vec<egui::Event>| {
            let mut raw_input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h))),
                events,
                ..Default::default()
            };
            raw_input.viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    monitor_size: Some(vec2(1920.0, 1080.0)),
                    native_pixels_per_point: Some(1.0),
                    outer_rect: Some(Rect::from_min_size(pos2(x, y), vec2(w, h))),
                    inner_rect: Some(Rect::from_min_size(pos2(x, y), vec2(w, h))),
                    ..Default::default()
                },
            );
            let out = run_test_ui(&ctx, raw_input, |ui| {
                app.sync_live_overlay_viewport_geometry(ui.ctx());
            });
            out.viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|v| v.commands.clone())
                .unwrap_or_default()
        };

    // 8 consecutive frames of OS WM_SIZING expansion past the bottom-right monitor edge:
    // sync must NOT emit fighting OuterPosition or InnerSize commands while size changes.
    for step in 0..8 {
        let w = 380.0 + (step as f32) * 14.0;
        let h = 230.0 + (step as f32) * 18.0;
        let cmds = run_sync_tick_with_events(&mut app, 1500.0, 780.0, w, h, Vec::new());
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                egui::ViewportCommand::OuterPosition(_) | egui::ViewportCommand::InnerSize(_)
            )),
            "tick {step}: active OS resize must suppress fighting viewport commands, got: {cmds:?}"
        );
    }

    // Holding the expanded rectangle steady WITHOUT releasing the native grab must keep
    // clamping suppressed across arbitrary idle ticks (user pauses mid-resize).
    let final_w = 380.0 + 7.0 * 14.0;
    let final_h = 230.0 + 7.0 * 18.0;
    for hold_tick in 0..10 {
        let cmds = run_sync_tick_with_events(&mut app, 1500.0, 780.0, final_w, final_h, Vec::new());
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                egui::ViewportCommand::OuterPosition(_) | egui::ViewportCommand::InnerSize(_)
            )),
            "hold_tick {hold_tick}: held pause before release must keep clamp suppressed, got: {cmds:?}"
        );
    }

    // Once the user releases (or OS modal resize returns control to pointer events), 5 steady
    // frames let the settle window flush and emit 1 final clamp command with 0 oscillation.
    let mut settled_cmds = Vec::new();
    for tick in 0..6 {
        let events = if tick == 0 {
            vec![egui::Event::PointerButton {
                pos: pos2(10.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }]
        } else {
            Vec::new()
        };
        let cmds = run_sync_tick_with_events(&mut app, 1500.0, 780.0, final_w, final_h, events);
        settled_cmds.extend(cmds);
    }
    let pos_cmds = settled_cmds
        .iter()
        .filter(|c| matches!(c, egui::ViewportCommand::OuterPosition(_)))
        .count();
    assert_eq!(
        pos_cmds, 1,
        "after WM resize settles post-release, sync must emit exactly 1 final OuterPosition clamp: {settled_cmds:?}"
    );
    assert!(
        app.session.overlay_geometry.y + app.session.overlay_geometry.height <= 1080.0 + 0.5,
        "clamped overlay must fit inside monitor height after settle: {:?}",
        app.session.overlay_geometry
    );
}

#[test]
fn live_overlay_viewport_resumes_clamp_when_pointer_resumes_after_held_pause() {
    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.set_known_monitors(vec![crate::session::MonitorRect::new(
        0.0, 0.0, 1920.0, 1080.0,
    )]);
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 1500.0,
        y: 780.0,
        width: 478.0,
        height: 356.0,
        click_through: false,
    };
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();
    app.overlay_viewport_sync.begin_wm_gesture();

    let run_tick = |app: &mut TrueSightGuiApp, events: Vec<egui::Event>| {
        let mut raw_input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(478.0, 356.0))),
            events,
            ..Default::default()
        };
        raw_input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                monitor_size: Some(vec2(1920.0, 1080.0)),
                native_pixels_per_point: Some(1.0),
                outer_rect: Some(Rect::from_min_size(pos2(1500.0, 780.0), vec2(478.0, 356.0))),
                inner_rect: Some(Rect::from_min_size(pos2(1500.0, 780.0), vec2(478.0, 356.0))),
                ..Default::default()
            },
        );
        run_test_ui(&ctx, raw_input, |ui| {
            app.sync_live_overlay_viewport_geometry(ui.ctx());
        })
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|v| v.commands.clone())
        .unwrap_or_default()
    };

    for _ in 0..8 {
        let cmds = run_tick(&mut app, Vec::new());
        assert!(!cmds.iter().any(|c| matches!(
            c,
            egui::ViewportCommand::OuterPosition(_) | egui::ViewportCommand::InnerSize(_)
        )));
    }

    // Cursor leave (`PointerGone` / WM_MOUSELEAVE) during a held pause must NOT end the native grab.
    for tick in 0..6 {
        let events = if tick == 0 {
            vec![egui::Event::PointerGone]
        } else {
            Vec::new()
        };
        let cmds = run_tick(&mut app, events);
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                egui::ViewportCommand::OuterPosition(_) | egui::ViewportCommand::InnerSize(_)
            )),
            "tick {tick}: PointerGone without button release must keep clamp suppressed: {cmds:?}"
        );
    }

    // Windows WM_SIZING consumes WM_LBUTTONUP when exiting modal resize; cursor motion resumes instead.
    let mut post_move_cmds = Vec::new();
    for tick in 0..6 {
        let events = if tick == 0 {
            vec![egui::Event::PointerMoved(pos2(20.0, 20.0))]
        } else {
            Vec::new()
        };
        post_move_cmds.extend(run_tick(&mut app, events));
    }
    assert!(
        post_move_cmds
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::OuterPosition(_))),
        "resumed pointer motion after WM exit must settle and clamp once: {post_move_cmds:?}"
    );
}

#[test]
fn live_overlay_viewport_suppresses_unannounced_continuous_os_resize_motion() {
    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.set_known_monitors(vec![crate::session::MonitorRect::new(
        0.0, 0.0, 1920.0, 1080.0,
    )]);
    session.overlay_geometry = crate::session::OverlayGeometry {
        x: 1540.0,
        y: 800.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();

    let run_sync_tick = |app: &mut TrueSightGuiApp, w: f32, h: f32| {
        let mut raw_input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h))),
            ..Default::default()
        };
        raw_input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                monitor_size: Some(vec2(1920.0, 1080.0)),
                native_pixels_per_point: Some(1.0),
                outer_rect: Some(Rect::from_min_size(pos2(1540.0, 800.0), vec2(w, h))),
                inner_rect: Some(Rect::from_min_size(pos2(1540.0, 800.0), vec2(w, h))),
                ..Default::default()
            },
        );
        run_test_ui(&ctx, raw_input, |ui| {
            app.sync_live_overlay_viewport_geometry(ui.ctx());
        })
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|v| v.commands.clone())
        .unwrap_or_default()
    };

    // Frame 0 baseline inside monitor, frames 1..5 simulate continuous native OS border resize
    // past the right/bottom edge without going through our resize handle.
    let _ = run_sync_tick(&mut app, 380.0, 230.0);
    let mut late_motion_cmds = Vec::new();
    for step in 1..=5 {
        let w = 380.0 + (step as f32) * 16.0;
        let h = 230.0 + (step as f32) * 16.0;
        let cmds = run_sync_tick(&mut app, w, h);
        if step >= 2 {
            late_motion_cmds.extend(cmds);
        }
    }
    assert!(
        !late_motion_cmds.iter().any(|c| matches!(
            c,
            egui::ViewportCommand::OuterPosition(_) | egui::ViewportCommand::InnerSize(_)
        )),
        "continuous OS resize motion must pause corrective commands rather than vibrating: {late_motion_cmds:?}"
    );
}

#[test]
fn live_overlay_viewport_fractional_dpi_and_boundary_clamp_do_not_oscillate() {
    for ppp in [1.25_f32, 1.5_f32, 1.75_f32] {
        let mon_w_px = (1920.0_f32 * ppp).round();
        let mon_h_px = (1080.0_f32 * ppp).round();
        let mut session = AppSession::new().unwrap();
        session.overlay_mode = true;
        session.set_known_monitors(vec![crate::session::MonitorRect::new(
            0.0, 0.0, mon_w_px, mon_h_px,
        )]);
        // Active Dota capture strip on top of the monitor, forcing lower clamp boundary.
        session.set_active_capture_strip_rect(Some((
            (180.0 * ppp).round(),
            (20.0 * ppp).round(),
            (1560.0 * ppp).round(),
            (72.0 * ppp).round(),
        )));
        session.overlay_geometry = crate::session::OverlayGeometry {
            x: 1545.0,
            y: 45.0,
            width: 391.0,
            height: 241.0,
            click_through: false,
        };
        let mut app = TrueSightGuiApp::new(session, None, None);
        let ctx = egui::Context::default();

        // Simulate Win32 integer-pixel SetWindowPos quantization + 1px DWM outer vs inner delta.
        let quantize = |v: f32| (v * ppp).floor() / ppp;
        let mut live_x = quantize(1545.3);
        let mut live_y = quantize(45.2);
        let mut live_w = quantize(391.4);
        let mut live_h = quantize(241.3);
        let mut total_pos_cmds = 0usize;
        let mut total_size_cmds = 0usize;

        for tick in 0..15 {
            let dwm_pad = if tick % 2 == 0 { 1.0 / ppp } else { 0.0 };
            let mut raw_input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(live_w, live_h))),
                ..Default::default()
            };
            raw_input.viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    monitor_size: Some(vec2(mon_w_px / ppp, mon_h_px / ppp)),
                    native_pixels_per_point: Some(ppp),
                    outer_rect: Some(Rect::from_min_size(
                        pos2(live_x, live_y),
                        vec2(live_w + dwm_pad, live_h + dwm_pad),
                    )),
                    inner_rect: Some(Rect::from_min_size(
                        pos2(live_x, live_y),
                        vec2(live_w, live_h),
                    )),
                    ..Default::default()
                },
            );
            let out = run_test_ui(&ctx, raw_input, |ui| {
                app.sync_live_overlay_viewport_geometry(ui.ctx());
            });
            let cmds = out
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|v| v.commands.clone())
                .unwrap_or_default();
            for cmd in cmds {
                match cmd {
                    egui::ViewportCommand::OuterPosition(p) => {
                        total_pos_cmds += 1;
                        live_x = quantize(p.x);
                        live_y = quantize(p.y);
                    }
                    egui::ViewportCommand::InnerSize(s) => {
                        total_size_cmds += 1;
                        live_w = quantize(s.x);
                        live_h = quantize(s.y);
                    }
                    _ => {}
                }
            }
        }

        assert!(
            total_pos_cmds <= 1,
            "ppp={ppp}: fractional DPI quantization must converge with <=1 OuterPosition command, emitted {total_pos_cmds}"
        );
        assert!(
            total_size_cmds <= 1,
            "ppp={ppp}: fractional DPI quantization must converge with <=1 InnerSize command, emitted {total_size_cmds}"
        );
    }
}

#[test]
fn gemstone_micro_grain_texture_is_deterministic_tileable_and_subtle() {
    let img_a = super::surfaces::generate_micro_grain_image();
    let img_b = super::surfaces::generate_micro_grain_image();
    assert_eq!(
        img_a.size,
        [
            super::surfaces::MICRO_GRAIN_SIZE,
            super::surfaces::MICRO_GRAIN_SIZE
        ]
    );
    assert_eq!(img_a.pixels, img_b.pixels);

    let mut min_lum = u8::MAX;
    let mut max_lum = 0_u8;
    for px in &img_a.pixels {
        let [r, g, b, a] = px.to_srgba_unmultiplied();
        min_lum = min_lum.min(r).min(g).min(b);
        max_lum = max_lum.max(r).max(g).max(b);
        assert!(
            a >= 200,
            "tileable grain texture alpha should stay high and subtle via tint: {a}"
        );
    }
    assert!(
        max_lum > min_lum && (max_lum - min_lum) >= 18,
        "grain texture must have crystalline variance: min={min_lum}, max={max_lum}"
    );
}

#[test]
fn gemstone_radial_glow_texture_decays_smoothly_without_hard_edges() {
    let img = super::surfaces::generate_radial_glow_image();
    let sz = super::surfaces::RADIAL_GLOW_SIZE;
    assert_eq!(img.size, [sz, sz]);
    let origin_alpha = img.pixels[0].to_srgba_unmultiplied()[3];
    let mid_alpha = img.pixels[(sz / 3) * sz + (sz / 3)].to_srgba_unmultiplied()[3];
    let right_edge_alpha = img.pixels[sz - 1].to_srgba_unmultiplied()[3];
    let bottom_edge_alpha = img.pixels[(sz - 1) * sz].to_srgba_unmultiplied()[3];
    let corner_alpha = img.pixels[sz * sz - 1].to_srgba_unmultiplied()[3];

    assert!(
        origin_alpha >= 240,
        "glow center near UV origin must stay luminous, got {origin_alpha}"
    );
    assert!(
        mid_alpha > 0 && mid_alpha < origin_alpha,
        "mid-card glow alpha must fall off monotonically ({origin_alpha} -> {mid_alpha})"
    );
    assert_eq!(
        right_edge_alpha, 0,
        "right edge must reach zero alpha before rect boundary"
    );
    assert_eq!(
        bottom_edge_alpha, 0,
        "bottom edge must reach zero alpha before rect boundary"
    );
    assert_eq!(corner_alpha, 0);
}

#[test]
fn hud_card_renders_gemstone_facets_behind_content_with_corner_ticks() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    let ctx = egui::Context::default();
    let full_output = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 260.0))),
            ..Default::default()
        },
        |ui| {
            app.textures.ensure_loaded(ui.ctx());
            egui::CentralPanel::default().show(ui, |ui| {
                super::theme::hud_card(ui, |ui| {
                    ui.label("Gemstone Smooth Glow Test");
                });
            });
        },
    );

    fn count_card_shapes(
        shape: &egui::Shape,
        polygons: &mut usize,
        lines: &mut usize,
        meshes: &mut usize,
    ) {
        match shape {
            egui::Shape::Vec(items) => {
                for s in items {
                    count_card_shapes(s, polygons, lines, meshes);
                }
            }
            egui::Shape::Path(path) if path.closed && path.fill != egui::Color32::TRANSPARENT => {
                *polygons += 1
            }
            egui::Shape::LineSegment { .. } | egui::Shape::Path(_) => *lines += 1,
            egui::Shape::Mesh(_) => *meshes += 1,
            _ => {}
        }
    }
    let mut polygons = 0;
    let mut lines = 0;
    let mut meshes = 0;
    for clipped in &full_output.shapes {
        count_card_shapes(&clipped.shape, &mut polygons, &mut lines, &mut meshes);
    }
    assert!(
        polygons >= 3,
        "hud_card must paint >= 3 sculpted crystal facet planes behind content, got {polygons}"
    );
    assert!(
        lines >= 8,
        "hud_card must paint specular bevels, facet ridge creases, and corner ticks, got {lines}"
    );
    assert!(
        meshes >= 3,
        "hud_card must composite radial emerald glow, obsidian vignette, and grain meshes, got {meshes}"
    );
}

#[test]
fn options_window_exposes_overlay_opacity_control_and_updates_session() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.options_open = true;
    app.session.set_overlay_opacity(0.58);
    let ctx = egui::Context::default();

    let mut out = None;
    for _ in 0..2 {
        out = Some(run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
                ..Default::default()
            },
            |ui| {
                let _ = app.render_options_window(ui.ctx(), 48.0);
            },
        ));
    }
    let out = out.expect("second pass options window output");

    let mut texts = Vec::new();
    for clipped in &out.shapes {
        collect_painted_texts(&clipped.shape, &mut texts);
    }
    let tr = super::i18n::strings(app.session.ui_language);
    assert!(
        texts.iter().any(|t| t.contains(tr.overlay_opacity_label)),
        "options drawer must show '{}' control; painted texts: {texts:?}",
        tr.overlay_opacity_label
    );
    assert!(
        texts.iter().any(|t| t.contains("58%")),
        "options drawer must show live overlay glass percentage '58%'; painted texts: {texts:?}"
    );
}

#[test]
fn overlay_small_text_scrim_clears_wcag_aa_contrast_over_white_backdrop_at_default_and_min_opacity()
{
    use super::theme::{P_CYAN, P_GLINT, P_NEUTRAL};
    use crate::session::{DEFAULT_OVERLAY_OPACITY, MIN_OVERLAY_OPACITY};
    use eframe::egui::Color32;

    let blend_over = |fg: Color32, bg: Color32| -> Color32 {
        let a = f32::from(fg.a()) / 255.0;
        let ch = |f: u8, b: u8| -> u8 {
            ((f32::from(f) * a + f32::from(b) * (1.0 - a)).round()).clamp(0.0, 255.0) as u8
        };
        Color32::from_rgb(ch(fg.r(), bg.r()), ch(fg.g(), bg.g()), ch(fg.b(), bg.b()))
    };
    let rel_luminance = |c: Color32| -> f32 {
        let lin = |u: u8| -> f32 {
            let v = f32::from(u) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    };
    let contrast_ratio = |a: Color32, b: Color32| -> f32 {
        let la = rel_luminance(a);
        let lb = rel_luminance(b);
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    };

    let scrim = Color32::from_rgba_unmultiplied(
        P_VOID.r(),
        P_VOID.g(),
        P_VOID.b(),
        super::surfaces::OVERLAY_TEXT_SCRIM_ALPHA,
    );
    for opacity in [DEFAULT_OVERLAY_OPACITY, MIN_OVERLAY_OPACITY] {
        let glass_shape = super::surfaces::build_overlay_glass_background(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 200.0)),
            opacity,
            None,
            None,
        );
        let base_fill = match glass_shape {
            egui::Shape::Vec(shapes) => match shapes.first() {
                Some(egui::Shape::Rect(r)) => r.fill,
                other => panic!("expected filled glass base rect, got {other:?}"),
            },
            other => panic!("expected Shape::Vec from glass background, got {other:?}"),
        };
        let glass_over_white = blend_over(base_fill, Color32::WHITE);
        let scrim_over_white = blend_over(scrim, glass_over_white);

        for (label, fg) in [
            ("P_GLINT", P_GLINT),
            ("P_SILVER", P_SILVER),
            ("P_GEM", P_GEM),
            ("P_CYAN", P_CYAN),
            ("P_NEUTRAL", P_NEUTRAL),
            ("OVERLAY_DIRE_TEXT", super::surfaces::OVERLAY_DIRE_TEXT),
        ] {
            let ratio = contrast_ratio(fg, scrim_over_white);
            assert!(
                ratio >= 4.5,
                "opacity={opacity:.2}: {label} contrast {ratio:.2}:1 must meet WCAG AA >= 4.5:1 over white backdrop"
            );
        }
    }

    let ctx = egui::Context::default();
    let out = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(160.0, 80.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                super::surfaces::paint_empty_matrix_slot(
                    ui.painter(),
                    Rect::from_min_size(pos2(8.0, 8.0), vec2(26.0, 26.0)),
                    36,
                    120,
                    Some("A1"),
                    8.5,
                );
                super::surfaces::paint_empty_matrix_slot(
                    ui.painter(),
                    Rect::from_min_size(pos2(40.0, 8.0), vec2(26.0, 26.0)),
                    36,
                    120,
                    Some("E1"),
                    8.5,
                );
            });
        },
    );
    fn collect_fills(shape: &egui::Shape, out: &mut Vec<Color32>) {
        match shape {
            egui::Shape::Rect(r) if r.fill != Color32::TRANSPARENT => out.push(r.fill),
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect_fills(s, out)),
            _ => {}
        }
    }
    let mut fills = Vec::new();
    for clipped in &out.shapes {
        collect_fills(&clipped.shape, &mut fills);
    }
    let scrim_count = fills
        .iter()
        .filter(|c| {
            **c == Color32::from_rgba_unmultiplied(
                P_VOID.r(),
                P_VOID.g(),
                P_VOID.b(),
                super::surfaces::OVERLAY_TEXT_SCRIM_ALPHA,
            )
        })
        .count();
    assert_eq!(
        scrim_count, 2,
        "both labeled empty matrix headers (A1, E1) must paint an opaque-enough dark scrim backing; fills={fills:?}"
    );
}

#[test]
fn pass_hotkey_egui_recording_captures_modified_key_and_rejects_unmodified() {
    use crate::session::{PassHotkey, PassHotkeyKey};

    let ctrl_shift = egui::Modifiers {
        ctrl: true,
        shift: true,
        ..Default::default()
    };
    assert_eq!(
        super::hotkey::from_egui_key(egui::Key::T, ctrl_shift),
        Some(PassHotkey {
            ctrl: true,
            alt: false,
            shift: true,
            key: PassHotkeyKey::T,
        })
    );

    let alt_only = egui::Modifiers {
        alt: true,
        ..Default::default()
    };
    assert_eq!(
        super::hotkey::from_egui_key(egui::Key::F10, alt_only),
        Some(PassHotkey {
            ctrl: false,
            alt: true,
            shift: false,
            key: PassHotkeyKey::F10,
        })
    );

    assert_eq!(
        super::hotkey::from_egui_key(egui::Key::T, egui::Modifiers::default()),
        None,
        "unmodified keys must be rejected when recording pass-through shortcut"
    );
    assert_eq!(
        super::hotkey::from_egui_key(egui::Key::F9, egui::Modifiers::default()),
        None
    );
}

#[test]
fn custom_pass_hotkey_triggers_in_app_toggle_and_suppresses_old_default() {
    use crate::session::PassHotkey;

    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.session.overlay_mode = true;
    app.session.set_overlay_click_through(false);

    let custom = PassHotkey::parse("Ctrl+Alt+K").unwrap();
    app.session.set_pass_hotkey(custom);

    let ctx = egui::Context::default();
    let viewport = vec2(1280.0, 800.0);
    let press_key = |key: egui::Key, modifiers: egui::Modifiers, app: &mut TrueSightGuiApp| {
        for pressed in [true, false] {
            let _ = run_test_ui(
                &ctx,
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    app.render_frame(ui);
                },
            );
        }
    };

    let ctrl_shift = egui::Modifiers {
        ctrl: true,
        shift: true,
        ..Default::default()
    };
    press_key(egui::Key::T, ctrl_shift, &mut app);
    assert!(
        !app.session.overlay_geometry.click_through,
        "old default Ctrl+Shift+T must NOT toggle after switching pass_hotkey to Ctrl+Alt+K"
    );

    let ctrl_alt = egui::Modifiers {
        ctrl: true,
        alt: true,
        ..Default::default()
    };
    press_key(egui::Key::K, ctrl_alt, &mut app);
    assert!(
        app.session.overlay_geometry.click_through,
        "custom Ctrl+Alt+K must toggle click-through ON"
    );
    press_key(egui::Key::K, ctrl_alt, &mut app);
    assert!(
        !app.session.overlay_geometry.click_through,
        "second Ctrl+Alt+K press must toggle click-through OFF"
    );
}

#[test]
fn options_window_shows_wayland_configure_shortcut_button_and_custom_recorder() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.options_open = true;
    let (_tx, rx) = std::sync::mpsc::sync_channel(8);
    app.hotkey_listener = Some(
        super::hotkey::GlobalHotkeyListener::from_channel_for_test_with_wayland(
            rx,
            "Ctrl+Shift+T (In-App)",
            true,
        ),
    );
    let ctx = egui::Context::default();
    let mut out = None;
    for _ in 0..2 {
        out = Some(run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
                ..Default::default()
            },
            |ui| {
                let _ = app.render_options_window(ui.ctx(), 48.0);
            },
        ));
    }
    let out = out.expect("second pass options window output");
    let mut texts = Vec::new();
    for clipped in &out.shapes {
        collect_painted_texts(&clipped.shape, &mut texts);
    }
    let tr = super::i18n::strings(app.session.ui_language);
    assert!(
        texts.iter().any(|t| t.contains(tr.pass_hotkey_label)),
        "options window must render PASS HOTKEY section; texts={texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains(tr.hotkey_change_btn)),
        "options window must render custom hotkey CHANGE button; texts={texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains(tr.configure_shortcut_btn)),
        "options window must render Wayland CONFIGURE SHORTCUT button; texts={texts:?}"
    );
}

#[test]
fn portal_v2_shortcut_routing_and_startup_bind_decisions() {
    use super::hotkey::{should_bind_on_startup, should_use_configure_shortcuts};

    assert!(!should_use_configure_shortcuts(0));
    assert!(!should_use_configure_shortcuts(1));
    assert!(should_use_configure_shortcuts(2));
    assert!(should_use_configure_shortcuts(3));

    // Startup must always bind when forced or when no shortcut entry is listed yet.
    assert!(should_bind_on_startup(true, true, false, 2));
    assert!(should_bind_on_startup(false, false, false, 1));
    assert!(should_bind_on_startup(false, false, false, 2));

    // When already listed with an active trigger on Portal v2, bind cleanly on startup so
    // KDE KGlobalAccel keeps Activated signals active without prompting.
    assert!(should_bind_on_startup(false, true, true, 2));

    // When listed with empty/unbound trigger (`none,none`) or on Portal v1, skip startup prompt
    // unless force_bind is requested.
    assert!(!should_bind_on_startup(false, true, false, 2));
    assert!(!should_bind_on_startup(false, true, true, 1));
    assert!(!should_bind_on_startup(false, true, false, 1));
}

#[test]
fn modified_f8_pass_hotkey_toggles_click_through_without_closing_overlay() {
    use crate::session::PassHotkey;

    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.session.overlay_mode = true;
    app.session.set_overlay_click_through(false);
    app.session
        .set_pass_hotkey(PassHotkey::parse("Ctrl+F8").unwrap());

    let ctx = egui::Context::default();
    let viewport = vec2(1280.0, 800.0);
    let ctrl = egui::Modifiers {
        ctrl: true,
        ..Default::default()
    };

    // Sending Ctrl+F8 must NOT toggle overlay visibility; it must trigger PASS only.
    for pressed in [true, false] {
        let _ = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                events: vec![egui::Event::Key {
                    key: egui::Key::F8,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: ctrl,
                }],
                ..Default::default()
            },
            |ui| {
                app.render_frame(ui);
            },
        );
    }
    assert!(
        app.session.overlay_mode,
        "Ctrl+F8 must keep overlay open instead of triggering unmodified F8 visibility toggle"
    );
    assert!(
        app.session.overlay_geometry.click_through,
        "Ctrl+F8 must toggle PASS click-through ON"
    );

    // Sending unmodified F8 still closes the overlay cleanly.
    for pressed in [true, false] {
        let _ = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                events: vec![egui::Event::Key {
                    key: egui::Key::F8,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                app.render_frame(ui);
            },
        );
    }
    assert!(
        !app.session.overlay_mode,
        "unmodified F8 must close the overlay"
    );
}

#[test]
fn recording_pass_hotkey_consumes_capture_keypress_without_toggling_overlay() {
    use crate::session::PassHotkey;

    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.options_open = true;
    app.hotkey_recording = true;
    app.session.overlay_mode = true;
    app.session.set_overlay_click_through(false);

    let ctx = egui::Context::default();
    let viewport = vec2(1280.0, 800.0);
    let send_key = |key: egui::Key, modifiers: egui::Modifiers, app: &mut TrueSightGuiApp| {
        for pressed in [true, false] {
            let _ = run_test_ui(
                &ctx,
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    app.render_frame(ui);
                },
            );
        }
    };

    let ctrl_alt = egui::Modifiers {
        ctrl: true,
        alt: true,
        ..Default::default()
    };
    send_key(egui::Key::K, ctrl_alt, &mut app);
    assert_eq!(
        app.session.pass_hotkey,
        PassHotkey::parse("Ctrl+Alt+K").unwrap()
    );
    assert!(!app.hotkey_recording);
    assert!(
        !app.session.overlay_geometry.click_through,
        "capture keypress Ctrl+Alt+K must NOT immediately toggle click-through on the capture frame"
    );

    // Next Ctrl+Alt+K press activates the newly recorded binding.
    send_key(egui::Key::K, ctrl_alt, &mut app);
    assert!(
        app.session.overlay_geometry.click_through,
        "subsequent Ctrl+Alt+K press must toggle click-through ON"
    );

    // Also test recording Ctrl+F8 while overlay is open: must neither close overlay nor toggle PASS.
    app.hotkey_recording = true;
    app.session.set_overlay_click_through(false);
    let ctrl = egui::Modifiers {
        ctrl: true,
        ..Default::default()
    };
    send_key(egui::Key::F8, ctrl, &mut app);
    assert_eq!(
        app.session.pass_hotkey,
        PassHotkey::parse("Ctrl+F8").unwrap()
    );
    assert!(!app.hotkey_recording);
    assert!(app.session.overlay_mode);
    assert!(!app.session.overlay_geometry.click_through);
}

#[test]
fn recording_pass_hotkey_suppresses_queued_global_action_on_capture_frame() {
    use super::hotkey::{GlobalHotkeyAction, GlobalHotkeyListener};
    use crate::session::PassHotkey;

    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    app.hotkey_listener = Some(GlobalHotkeyListener::from_channel_for_test(
        rx,
        "Ctrl+Shift+T (Global)",
    ));
    app.onboarding_open = false;
    app.options_open = true;
    app.hotkey_recording = true;
    app.session.overlay_mode = true;
    app.session.set_overlay_click_through(false);

    let ctx = egui::Context::default();
    let viewport = vec2(1280.0, 800.0);
    let ctrl_shift = egui::Modifiers {
        ctrl: true,
        shift: true,
        ..Default::default()
    };

    // Re-record current Ctrl+Shift+T while the global shortcut thread also enqueues
    // ToggleOverlayPass in the same frame: must save without toggling PASS.
    tx.send(GlobalHotkeyAction::ToggleOverlayPass).unwrap();
    let _ = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
            events: vec![egui::Event::Key {
                key: egui::Key::T,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: ctrl_shift,
            }],
            ..Default::default()
        },
        |ui| {
            app.render_frame(ui);
        },
    );
    let _ = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
            events: vec![egui::Event::Key {
                key: egui::Key::T,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: ctrl_shift,
            }],
            ..Default::default()
        },
        |ui| {
            app.render_frame(ui);
        },
    );

    assert_eq!(app.session.pass_hotkey, PassHotkey::default());
    assert!(!app.hotkey_recording);
    assert!(
        !app.session.overlay_geometry.click_through,
        "queued global ToggleOverlayPass on capture frame must NOT toggle PASS"
    );

    // Subsequent global action when not recording works normally.
    tx.send(GlobalHotkeyAction::ToggleOverlayPass).unwrap();
    let _ = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), viewport)),
            ..Default::default()
        },
        |ui| {
            app.render_frame(ui);
        },
    );
    assert!(
        app.session.overlay_geometry.click_through,
        "subsequent global ToggleOverlayPass outside capture frame must toggle PASS ON"
    );
}

#[test]
fn build_bar_p1_p5_selector_switches_build_view_without_mutating_local_hero() {
    let mut session = make_5v5_gui_session();
    session.clear_slot_manual(truesight_core::domain::Side::Allies, 1);
    let local_hero = session.draft.allies[0]
        .expect("slot 0 must hold initial ally")
        .hero_id;
    session.local_player_hero = Some(local_hero);
    session.owned_items_owner = Some(local_hero);
    session.toggle_post_draft_build_view();
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();

    let run_build_bar = |app: &mut TrueSightGuiApp, events: Vec<egui::Event>| {
        let mut captured = None;
        let _ = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 500.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    captured = Some(app.render_item_build_card(ui));
                });
            },
        );
        captured.expect("render_item_build_card output")
    };
    let click_build_bar = |app: &mut TrueSightGuiApp, pos: egui::Pos2| {
        for pressed in [true, false] {
            let _ = run_build_bar(
                app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
        }
    };

    let out = run_build_bar(&mut app, Vec::new());
    let p3_rect = out.ally_role_buttons[2];
    let p2_rect = out.ally_role_buttons[1];
    assert_ne!(
        p3_rect,
        Rect::NOTHING,
        "picked P3 button must have valid rect"
    );
    assert_ne!(
        p2_rect,
        Rect::NOTHING,
        "empty P2 button must have valid rect"
    );

    // Click empty P2 slot: disabled, must NOT change build_view_hero.
    click_build_bar(&mut app, p2_rect.center());
    assert_eq!(app.session.build_view_hero, None);

    // Click picked P3: switches build_view_hero while preserving local_player_hero & owner.
    click_build_bar(&mut app, p3_rect.center());
    let expected_p3 = app.session.draft.allies[2].map(|p| p.hero_id);
    assert_eq!(app.session.build_view_hero, expected_p3);
    assert_eq!(app.session.local_player_hero, Some(local_hero));
    assert_eq!(app.session.owned_items_owner, Some(local_hero));
    assert_eq!(
        app.session.draft.allies[0].map(|p| p.hero_id),
        Some(local_hero)
    );
}

#[test]
fn overlay_build_p1_p5_selector_switches_build_view_and_preserves_local_identity() {
    use truesight_core::domain::HeroId;

    let mut session = make_5v5_gui_session();
    session.local_player_hero = Some(HeroId(1));
    session.owned_items_owner = Some(HeroId(1));
    session.overlay_mode = true;
    session.toggle_post_draft_build_view();
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();

    let run_overlay = |app: &mut TrueSightGuiApp, events: Vec<egui::Event>| {
        let mut captured = None;
        let _ = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(440.0, 210.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    captured = Some(app.render_overlay_panel(ui));
                });
            },
        );
        captured.expect("render_overlay_panel output")
    };

    let out = run_overlay(&mut app, Vec::new());
    let p3_rect = out.build_ally_buttons[2];
    assert_ne!(
        p3_rect,
        Rect::NOTHING,
        "overlay build view must expose P1..P5 ally buttons"
    );

    for pressed in [true, false] {
        let _ = run_overlay(
            &mut app,
            vec![
                egui::Event::PointerMoved(p3_rect.center()),
                egui::Event::PointerButton {
                    pos: p3_rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
    }
    let expected_p3 = app.session.draft.allies[2].map(|p| p.hero_id);
    assert_eq!(app.session.build_view_hero, expected_p3);
    assert_eq!(app.session.local_player_hero, Some(HeroId(1)));
    assert_eq!(app.session.owned_items_owner, Some(HeroId(1)));
}

#[test]
fn overlay_header_faction_swap_button_toggles_ally_faction() {
    use truesight_core::domain::Faction;

    let mut session = make_5v5_gui_session();
    session.overlay_mode = true;
    assert_eq!(session.draft.ally_faction, Faction::Radiant);
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();

    let run_overlay = |app: &mut TrueSightGuiApp, events: Vec<egui::Event>| {
        let mut captured = None;
        let _ = run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 240.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    captured = Some(app.render_overlay_panel(ui));
                });
            },
        );
        captured.expect("render_overlay_panel output")
    };

    let out = run_overlay(&mut app, Vec::new());
    assert_ne!(
        out.faction_swap_button,
        Rect::NOTHING,
        "overlay header must expose Radiant/Dire faction swap button"
    );
    for pressed in [true, false] {
        let _ = run_overlay(
            &mut app,
            vec![
                egui::Event::PointerMoved(out.faction_swap_button.center()),
                egui::Event::PointerButton {
                    pos: out.faction_swap_button.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
    }
    assert_eq!(
        app.session.draft.ally_faction,
        Faction::Dire,
        "clicking overlay faction swap button must toggle Radiant -> Dire"
    );
}

#[test]
fn overlay_picks_matrix_and_build_phases_center_when_space_allows() {
    assert_eq!(super::overlay::centered_leading_space(200.0, 140.0), 30.0);
    assert_eq!(super::overlay::centered_leading_space(120.0, 140.0), 0.0);

    // 1. Picks overlay: 5x5 CNT matrix is horizontally centered between TOP PICKS and right edge.
    let mut session = make_5v5_gui_session();
    session.overlay_mode = true;
    let mut app = TrueSightGuiApp::new(session, None, None);
    let ctx = egui::Context::default();
    let mut picks_out = None;
    let _ = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(580.0, 260.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    picks_out = Some(app.render_overlay_panel(ui));
                });
        },
    );
    let picks_out = picks_out.expect("picks overlay output");
    let zone_left = picks_out.picks_col_rect.max.x + 8.0;
    let zone_right = picks_out.rect.max.x - 8.0;
    let left_margin = picks_out.matrix_rect.min.x - zone_left;
    let right_margin = zone_right - picks_out.matrix_rect.max.x;
    assert!(
        (left_margin - right_margin).abs() <= 3.0,
        "picks matrix should be horizontally centered in right zone: left_margin={left_margin:.1}, right_margin={right_margin:.1}, zone=({zone_left:.1}..{zone_right:.1}), matrix={:?}",
        picks_out.matrix_rect
    );

    // 2. Build overlay: EARLY > CORE > LATE phase strip centers when wide, stays left-aligned when narrow.
    app.session.toggle_post_draft_build_view();
    let mut wide_build = None;
    let _ = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(620.0, 200.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    wide_build = Some(app.render_overlay_panel(ui));
                });
        },
    );
    let wide_build = wide_build.expect("wide build overlay output");
    let phases_rect = wide_build
        .build_phases_rect
        .expect("build_phases_rect must be present");
    let inner_left = wide_build.rect.min.x + 8.0;
    let inner_right = wide_build.rect.max.x - 8.0;
    let b_left = phases_rect.min.x - inner_left;
    let b_right = inner_right - phases_rect.max.x;
    assert!(
        b_left > 10.0 && (b_left - b_right).abs() <= 4.0,
        "wide build phases ({:?}) should be centered inside panel ({:?}): b_left={b_left:.1}, b_right={b_right:.1}",
        phases_rect,
        wide_build.rect
    );

    // 3. Narrow Build overlay preserves zero-clamped leading space (`centered_leading_space == 0.0`
    //    when content exceeds width) so horizontal scrolling stays left-aligned without clipping.
    let mut narrow_build = None;
    let _ = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(220.0, 120.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    narrow_build = Some(app.render_overlay_panel(ui));
                });
        },
    );
    let narrow_build = narrow_build.expect("narrow build overlay output");
    let narrow_phases = narrow_build
        .build_phases_rect
        .expect("narrow build_phases_rect must be present");
    let narrow_avail = (narrow_build.rect.width() - 16.0).max(0.0);
    let expected_lead = super::overlay::centered_leading_space(narrow_avail, narrow_phases.width());
    let actual_lead = narrow_phases.min.x - (narrow_build.rect.min.x + 8.0);
    assert!(
        (actual_lead - expected_lead).abs() <= 2.5,
        "narrow build phases ({:?}) leading offset {actual_lead:.1} must match centered_leading_space={expected_lead:.1}",
        narrow_phases
    );
}

#[test]
fn options_window_renders_auto_overlay_toggle_without_low_res_overlap() {
    let mut app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    app.onboarding_open = false;
    app.options_open = true;
    let ctx = egui::Context::default();
    let tr = super::i18n::strings(app.session.ui_language);

    let mut out = None;
    for _ in 0..2 {
        out = Some(run_test_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1180.0, 700.0))),
                ..Default::default()
            },
            |ui| {
                let _ = app.render_options_window(ui.ctx(), 48.0);
            },
        ));
    }
    let out = out.expect("second pass options window output");
    let mut texts = Vec::new();
    for clipped in &out.shapes {
        collect_painted_texts(&clipped.shape, &mut texts);
    }
    assert!(
        texts.iter().any(|t| t.contains(tr.auto_overlay_label)),
        "options window must render Auto-overlay toggle; texts={texts:?}"
    );

    // Must not overlap top_bar or overflow at minimum supported 640x440 resolution.
    let r_min = render_at_viewport(&mut app, &ctx, vec2(640.0, 440.0));
    let opts_rect = r_min.options_window.expect("OPTIONS open at 640x440");
    assert!(
        !opts_rect.intersects(r_min.top_bar) && opts_rect.max.y <= 440.0,
        "OPTIONS {:?} must not collide with top_bar {:?} or overflow 640x440",
        opts_rect,
        r_min.top_bar
    );
}

#[test]
fn eframe_app_clear_color_is_transparent_and_overlay_builder_disables_native_shadow() {
    let app = TrueSightGuiApp::new(make_5v5_gui_session(), None, None);
    let clear = eframe::App::clear_color(&app, &egui::Visuals::dark());
    assert_eq!(
        clear,
        [0.0, 0.0, 0.0, 0.0],
        "eframe::App::clear_color must be fully transparent so Windows overlay viewports composite cleanly"
    );

    let builder = super::overlay::build_overlay_viewport_builder(
        crate::session::OverlayGeometry::default(),
        false,
    );
    assert_eq!(builder.transparent, Some(true));
    assert_eq!(builder.decorations, Some(false));
    assert_eq!(
        builder.has_shadow,
        Some(false),
        "compact transparent overlay viewport must disable native DWM drop shadow"
    );

    let native_opts = super::build_native_options();
    assert_eq!(
        native_opts.renderer,
        eframe::Renderer::Glow,
        "native GUI must explicitly select eframe::Renderer::Glow so Windows executes the upstream transparent viewport fix"
    );
}

#[test]
fn build_bar_and_overlay_clear_upg_ready_after_prerequisite_is_upgraded() {
    use truesight_adapters::item_repo::SeededItemMetaRepository;
    use truesight_core::domain::{HeroItemStat, ItemCatalogEntry, ItemId, ItemPhase};

    let mut session = AppSession::new().unwrap();
    assert!(execute_command(&mut session, "ally 3 axe").unwrap());
    session.local_player_hero = Some(HeroId(2));
    session.owned_items_owner = Some(HeroId(2));
    session.item_repo = SeededItemMetaRepository {
        custom_catalog: vec![
            ItemCatalogEntry {
                id: ItemId(1),
                slug: "blink".into(),
                display_name: "Blink Dagger".into(),
                cost: 2250,
                components: vec![],
                is_recipe: false,
                is_consumable_drop: false,
            },
            ItemCatalogEntry {
                id: ItemId(600),
                slug: "overwhelming_blink".into(),
                display_name: "Overwhelming Blink".into(),
                cost: 6800,
                components: vec![ItemId(1), ItemId(53)],
                is_recipe: false,
                is_consumable_drop: false,
            },
            ItemCatalogEntry {
                id: ItemId(603),
                slug: "swift_blink".into(),
                display_name: "Swift Blink".into(),
                cost: 6800,
                components: vec![ItemId(1), ItemId(52)],
                is_recipe: false,
                is_consumable_drop: false,
            },
            ItemCatalogEntry {
                id: ItemId(114),
                slug: "heart".into(),
                display_name: "Heart of Tarrasque".into(),
                cost: 5100,
                components: vec![],
                is_recipe: false,
                is_consumable_drop: false,
            },
        ],
        custom_hero_stats: vec![(
            HeroId(2),
            None,
            None,
            vec![
                HeroItemStat {
                    item_id: ItemId(1),
                    phase: ItemPhase::Early,
                    avg_time_mins: 11.0,
                    match_count: 2500,
                    win_count: 1375,
                },
                HeroItemStat {
                    item_id: ItemId(603),
                    phase: ItemPhase::Early,
                    avg_time_mins: 13.0,
                    match_count: 900,
                    win_count: 500,
                },
                HeroItemStat {
                    item_id: ItemId(600),
                    phase: ItemPhase::Late,
                    avg_time_mins: 32.0,
                    match_count: 1600,
                    win_count: 920,
                },
                HeroItemStat {
                    item_id: ItemId(114),
                    phase: ItemPhase::Late,
                    avg_time_mins: 36.0,
                    match_count: 1200,
                    win_count: 680,
                },
            ],
        )],
        ..Default::default()
    };
    // Hero upgraded Blink Dagger (1) into Overwhelming Blink (600); Swift Blink (603) stays attached
    // as a sibling direct-upgrade variant beneath Blink Dagger.
    session.owned_items = vec![ItemId(600)];
    session.toggle_post_draft_build_view();

    let build = session.compute_recommended_build().unwrap();
    let blink_step = build
        .steps
        .iter()
        .find(|s| s.item_id == ItemId(1))
        .expect("Blink Dagger step present");
    assert!(
        blink_step.owned,
        "Blink Dagger (1) must be marked satisfied when Overwhelming Blink (600) is owned"
    );
    assert_eq!(
        build.next_unowned_index.map(|i| build.steps[i].item_id),
        Some(ItemId(114)),
        "NEXT must advance past upgraded Blink Dagger directly to Heart (114): {:?}",
        build.steps
    );

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.onboarding_open = false;
    let ctx = egui::Context::default();
    let desk_out = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(920.0, 420.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let _ = app.render_item_build_card(ui);
                });
        },
    );
    let mut desk_texts = Vec::new();
    for c in &desk_out.shapes {
        collect_painted_texts(&c.shape, &mut desk_texts);
    }
    assert!(
        desk_texts.iter().any(|t| t == "OK"),
        "desktop build bar must mark upgraded Blink Dagger OK: {desk_texts:?}"
    );
    assert!(
        desk_texts.iter().all(|t| !t.starts_with("UPG READY")),
        "sibling upgrade must NOT highlight UPG READY when base item was already upgraded: {desk_texts:?}"
    );

    app.session.overlay_mode = true;
    let ovl_out = run_test_ui(
        &ctx,
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(520.0, 240.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let _ = app.render_overlay_panel(ui);
                });
        },
    );
    let mut ovl_texts = Vec::new();
    for c in &ovl_out.shapes {
        collect_painted_texts(&c.shape, &mut ovl_texts);
    }
    assert!(
        ovl_texts.iter().any(|t| t == "OK"),
        "overlay must mark upgraded Blink Dagger OK: {ovl_texts:?}"
    );
}
