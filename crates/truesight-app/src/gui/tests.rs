use super::*;
use eframe::egui::pos2;
use truesight_adapters::vision::parse_hero_atlas;
use truesight_core::domain::{HeroId, Role};
use crate::execute_command;

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

    let mut app = TrueSightGuiApp::new(session, None, None);
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
                r.top_bar.height() > 24.0 && r.top_bar.height() < 65.0,
                "viewport={viewport:?} tab={tab:?} top_bar height out of range: {:?}",
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
    assert_eq!(app.session.draft.allies[1].map(|p| p.hero_id), Some(puck_id));
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
}
