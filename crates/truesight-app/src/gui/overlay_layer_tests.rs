use super::TrueSightGuiApp;
use super::overlay_backend::{
    LogicalOutputBounds, OverlayBackendKind, OverlayHostMode, OverlayRect,
    build_wayland_overlay_raw_input, compute_anchored_drag_desktop_pos, compute_drag_margin_step,
    compute_next_logical_output_geometry, compute_next_monitor_geometry, compute_resize_step,
    resolve_cross_monitor_drag_target, resolve_overlay_backend,
};
use super::overlay_raster::OverlayRasterizer;
use crate::AppSession;
use crate::session::{MonitorRect, OverlayGeometry};
use eframe::egui::{
    self, Color32, ColorImage, Mesh, Pos2, Rect, TextureFilter, TextureId, TextureOptions,
    TexturesDelta, epaint::Vertex, pos2, vec2,
};
use truesight_core::domain::Side;

#[test]
fn backend_router_respects_overrides_and_fallbacks() {
    assert_eq!(
        resolve_overlay_backend(Some("viewport"), true, true),
        OverlayBackendKind::EframeViewport
    );
    assert_eq!(
        resolve_overlay_backend(Some("layer-shell"), true, true),
        OverlayBackendKind::WaylandLayerShell
    );
    assert_eq!(
        resolve_overlay_backend(Some("layer-shell"), true, false),
        OverlayBackendKind::EframeViewport
    );
    assert_eq!(
        resolve_overlay_backend(None, true, true),
        OverlayBackendKind::WaylandLayerShell
    );
    assert_eq!(
        resolve_overlay_backend(None, true, false),
        OverlayBackendKind::EframeViewport
    );
    assert_eq!(
        resolve_overlay_backend(None, false, false),
        OverlayBackendKind::EframeViewport
    );
}

#[test]
fn drag_margin_and_resize_helpers_clamp_within_bounds() {
    let moved = compute_drag_margin_step((100, 120), (50.4, -30.2), (380, 230), Some((1920, 1080)));
    assert_eq!(moved, (150, 90));

    let clamped_hi =
        compute_drag_margin_step((1700, 950), (100.0, 100.0), (380, 230), Some((1920, 1080)));
    assert_eq!(clamped_hi, (1540, 850));

    let clamped_lo =
        compute_drag_margin_step((10, 10), (-50.0, -50.0), (380, 230), Some((1920, 1080)));
    assert_eq!(clamped_lo, (0, 0));

    let resized_picks_min =
        compute_resize_step((380, 230), (-200.0, -200.0), (280, 160), (840, 720));
    assert_eq!(resized_picks_min, (280, 160));

    let resized_build_min =
        compute_resize_step((420, 168), (-300.0, -100.0), (220, 104), (840, 720));
    assert_eq!(resized_build_min, (220, 104));

    let resized_max = compute_resize_step((700, 600), (300.0, 300.0), (280, 160), (840, 720));
    assert_eq!(resized_max, (840, 720));

    let rect = OverlayRect::from(Rect::from_min_max(pos2(10.0, 20.0), pos2(30.0, 40.0)));
    assert!(rect.contains(10.0, 20.0));
    assert!(rect.contains(30.0, 40.0));
    assert!(!rect.contains(9.9, 25.0));
}

fn push_quad(mesh: &mut Mesh, rect: Rect, uv_rect: Rect, color: Color32) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices.extend_from_slice(&[
        Vertex {
            pos: rect.left_top(),
            uv: uv_rect.left_top(),
            color,
        },
        Vertex {
            pos: rect.right_top(),
            uv: uv_rect.right_top(),
            color,
        },
        Vertex {
            pos: rect.right_bottom(),
            uv: uv_rect.right_bottom(),
            color,
        },
        Vertex {
            pos: rect.left_bottom(),
            uv: uv_rect.left_bottom(),
            color,
        },
    ]);
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[test]
fn rasterizer_renders_solid_blended_and_clipped_quads_in_argb8888() {
    let mut raster = OverlayRasterizer::new();
    let white_tex = TextureId::Managed(0);
    let mut delta = TexturesDelta::default();
    delta.push(
        white_tex,
        egui::epaint::ImageDelta::full(
            ColorImage::filled([1, 1], Color32::WHITE),
            TextureOptions::NEAREST,
        ),
    );
    raster.apply_textures_delta(&mut delta);

    let mut mesh = Mesh::with_texture(white_tex);
    // Solid premultiplied opaque green rect in [2..6, 2..6]
    let green = Color32::from_rgba_premultiplied(10, 200, 40, 255);
    push_quad(
        &mut mesh,
        Rect::from_min_max(pos2(2.0, 2.0), pos2(6.0, 6.0)),
        Rect::ZERO,
        green,
    );
    // Semi-transparent red rect [3..8, 3..8], clipped strictly to x < 5
    let red_half = Color32::from_rgba_premultiplied(100, 0, 0, 128);
    let mut clipped_mesh = Mesh::with_texture(white_tex);
    push_quad(
        &mut clipped_mesh,
        Rect::from_min_max(pos2(3.0, 3.0), pos2(8.0, 8.0)),
        Rect::ZERO,
        red_half,
    );

    let primitives = vec![
        egui::ClippedPrimitive {
            clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)),
            primitive: egui::epaint::Primitive::Mesh(mesh),
        },
        egui::ClippedPrimitive {
            clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(5.0, 10.0)),
            primitive: egui::epaint::Primitive::Mesh(clipped_mesh),
        },
    ];

    let bytes = raster
        .rasterize_if_dirty(10, 10, 1.0, &primitives)
        .expect("first frame must rasterize");
    assert_eq!(bytes.len(), 10 * 10 * 4);

    // Pixel (0, 0) outside any rect -> transparent [0,0,0,0]
    assert_eq!(&bytes[0..4], &[0, 0, 0, 0]);

    // Pixel (2, 2) -> solid green [B=40, G=200, R=10, A=255]
    let p22 = (2 * 10 + 2) * 4;
    assert_eq!(&bytes[p22..p22 + 4], &[40, 200, 10, 255]);

    // Pixel (3, 3) -> semi-transparent red over solid green:
    // out_c = src_c + (dst_c * (255 - 128) + 127) / 255
    let p33 = (3 * 10 + 3) * 4;
    let inv_a = 255_u32 - 128;
    let exp_b = (40 * inv_a + 127) / 255;
    let exp_g = (200 * inv_a + 127) / 255;
    let exp_r = 100 + (10 * inv_a + 127) / 255;
    assert!((bytes[p33] as i32 - exp_b as i32).abs() <= 1);
    assert!((bytes[p33 + 1] as i32 - exp_g as i32).abs() <= 1);
    assert!((bytes[p33 + 2] as i32 - exp_r as i32).abs() <= 1);
    assert_eq!(bytes[p33 + 3], 255);

    // Pixel (5, 3) -> strictly clipped out of red rect, remains solid green
    let p53 = (3 * 10 + 5) * 4;
    assert_eq!(&bytes[p53..p53 + 4], &[40, 200, 10, 255]);

    // Pixel (6, 4) -> outside both green and clipped red -> transparent
    let p64 = (4 * 10 + 6) * 4;
    assert_eq!(&bytes[p64..p64 + 4], &[0, 0, 0, 0]);

    // Identical second call must skip rasterization (dirty hash match)
    assert!(
        raster
            .rasterize_if_dirty(10, 10, 1.0, &primitives)
            .is_none()
    );

    // Invalidating must force re-rasterization on an identical frame (e.g. after Hide -> Show)
    raster.invalidate();
    assert!(
        raster
            .rasterize_if_dirty(10, 10, 1.0, &primitives)
            .is_some()
    );
}

#[test]
fn rasterizer_supports_partial_texture_delta_and_free() {
    let mut raster = OverlayRasterizer::new();
    let tex_id = TextureId::Managed(7);
    let mut delta1 = TexturesDelta::default();
    delta1.push(
        tex_id,
        egui::epaint::ImageDelta::full(
            ColorImage::filled([4, 4], Color32::from_rgba_premultiplied(10, 20, 30, 255)),
            TextureOptions {
                magnification: TextureFilter::Nearest,
                minification: TextureFilter::Nearest,
                ..Default::default()
            },
        ),
    );
    raster.apply_textures_delta(&mut delta1);
    assert_eq!(raster.texture_count(), 1);

    // Patch pixel [2, 1] only
    let mut delta2 = TexturesDelta::default();
    delta2.push(
        tex_id,
        egui::epaint::ImageDelta::partial(
            [2, 1],
            ColorImage::filled([1, 1], Color32::from_rgba_premultiplied(200, 210, 220, 255)),
            TextureOptions {
                magnification: TextureFilter::Nearest,
                minification: TextureFilter::Nearest,
                ..Default::default()
            },
        ),
    );
    raster.apply_textures_delta(&mut delta2);

    let mut mesh = Mesh::with_texture(tex_id);
    push_quad(
        &mut mesh,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 4.0)),
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    let prims = vec![egui::ClippedPrimitive {
        clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 4.0)),
        primitive: egui::epaint::Primitive::Mesh(mesh),
    }];
    let bytes = raster.rasterize_if_dirty(4, 4, 1.0, &prims).unwrap();
    let patched = 6 * 4;
    let untouched = 0;
    assert_eq!(&bytes[patched..patched + 4], &[220, 210, 200, 255]);
    assert_eq!(&bytes[untouched..untouched + 4], &[30, 20, 10, 255]);

    let mut free_delta = TexturesDelta::default();
    free_delta.free(tex_id);
    raster.apply_textures_delta(&mut free_delta);
    assert_eq!(raster.texture_count(), 0);
}

#[test]
fn layer_shell_overlay_headless_tessellation_and_rasterization_picks_and_build() {
    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    let axe = session
        .matrix
        .heroes()
        .find(|h| h.display_name.eq_ignore_ascii_case("Axe"))
        .unwrap()
        .id;
    let invoker = session
        .matrix
        .heroes()
        .find(|h| h.display_name.eq_ignore_ascii_case("Invoker"))
        .unwrap()
        .id;
    session.set_slot_manual(Side::Allies, 0, axe);
    session.set_slot_manual(Side::Enemies, 0, invoker);

    let mut app = TrueSightGuiApp::new(session, None, None);
    let overlay_ctx = egui::Context::default();
    let mut raster = OverlayRasterizer::new();

    // 1. Draft Picks mode
    let mut out_picks = overlay_ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(380.0, 230.0))),
            ..Default::default()
        },
        |ui| {
            TrueSightGuiApp::configure_visuals_for(ui.ctx());
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let _ = app.render_overlay_panel_with_host(ui, OverlayHostMode::LayerShell);
                });
        },
    );
    raster.apply_textures_delta(&mut out_picks.textures_delta);
    let prims_picks = overlay_ctx.tessellate(out_picks.shapes, out_picks.pixels_per_point);
    let buf_picks = raster
        .rasterize_if_dirty(380, 230, out_picks.pixels_per_point, &prims_picks)
        .expect("picks overlay should rasterize");
    let mut saw_translucent_bg = false;
    let mut saw_opaque_fg = false;
    for px in buf_picks.as_chunks::<4>().0 {
        if px[3] > 140 && px[3] < 250 {
            saw_translucent_bg = true;
        }
        if px[3] == 255 {
            saw_opaque_fg = true;
        }
    }
    assert!(saw_translucent_bg, "overlay interior should be translucent");
    assert!(saw_opaque_fg, "overlay should have opaque borders/text");

    // 2. Post-Draft Build mode
    app.session.toggle_post_draft_build_view();
    let mut out_build = overlay_ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(420.0, 168.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let _ = app.render_overlay_panel_with_host(ui, OverlayHostMode::LayerShell);
                });
        },
    );
    raster.apply_textures_delta(&mut out_build.textures_delta);
    let prims_build = overlay_ctx.tessellate(out_build.shapes, out_build.pixels_per_point);
    let buf_build = raster
        .rasterize_if_dirty(420, 168, out_build.pixels_per_point, &prims_build)
        .expect("build overlay should rasterize");
    assert!(buf_build.as_chunks::<4>().0.iter().any(|px| px[3] > 0));
}

#[test]
fn rasterizer_clears_buffer_on_resize_without_stale_noise() {
    let mut raster = OverlayRasterizer::new();
    let white_tex = TextureId::Managed(0);
    let mut delta = TexturesDelta::default();
    delta.push(
        white_tex,
        egui::epaint::ImageDelta::full(
            ColorImage::filled([1, 1], Color32::WHITE),
            TextureOptions::NEAREST,
        ),
    );
    raster.apply_textures_delta(&mut delta);

    let mut solid_mesh = Mesh::with_texture(white_tex);
    push_quad(
        &mut solid_mesh,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)),
        Rect::ZERO,
        Color32::from_rgba_premultiplied(20, 220, 80, 255),
    );
    let prims1 = vec![egui::ClippedPrimitive {
        clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)),
        primitive: egui::epaint::Primitive::Mesh(solid_mesh),
    }];
    let first = raster.rasterize_if_dirty(10, 10, 1.0, &prims1).unwrap();
    assert!(first.as_chunks::<4>().0.iter().all(|px| px[3] == 255));

    // Grow width & shrink height with only a tiny 2x2 quad in top-left.
    let mut small_mesh = Mesh::with_texture(white_tex);
    push_quad(
        &mut small_mesh,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(2.0, 2.0)),
        Rect::ZERO,
        Color32::from_rgba_premultiplied(50, 100, 150, 255),
    );
    let prims2 = vec![egui::ClippedPrimitive {
        clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(14.0, 8.0)),
        primitive: egui::epaint::Primitive::Mesh(small_mesh),
    }];
    let resized = raster.rasterize_if_dirty(14, 8, 1.0, &prims2).unwrap();
    assert_eq!(resized.len(), 14 * 8 * 4);
    for y in 0..8 {
        for x in 0..14 {
            let off = (y * 14 + x) * 4;
            if x < 2 && y < 2 {
                assert_eq!(&resized[off..off + 4], &[150, 100, 50, 255]);
            } else {
                assert_eq!(
                    &resized[off..off + 4],
                    &[0, 0, 0, 0],
                    "stale pixel survived resize at ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn cross_monitor_drag_migrates_across_portrait_and_landscape_outputs() {
    let outputs = [
        LogicalOutputBounds {
            id: 2,
            x: 0,
            y: 0,
            width: 1440,
            height: 2560,
        },
        LogicalOutputBounds {
            id: 1,
            x: 1440,
            y: 560,
            width: 2560,
            height: 1440,
        },
    ];
    let ov_sz = (380, 230);

    // Small overshoot (10px < 48px) with pointer still inside output 2 must stay put.
    assert!(
        resolve_cross_monitor_drag_target(
            Some(2),
            (0, 0),
            (1440, 2560),
            (1440 - 380 + 10, 900),
            (1430, 910),
            ov_sz,
            &outputs,
        )
        .is_none()
    );

    // Dragging pointer into output 1 at y=900 migrates to DP-1 (y relative to 560 -> 340).
    let to_dp1 = resolve_cross_monitor_drag_target(
        Some(2),
        (0, 0),
        (1440, 2560),
        (1440, 900),
        (1460, 900),
        ov_sz,
        &outputs,
    )
    .expect("crossing into DP-1 must migrate");
    assert_eq!(to_dp1.output_id, 1);
    assert_eq!(to_dp1.origin, (1440, 560));
    assert_eq!(to_dp1.margin, (0, 340));

    // Dragging right above DP-1 top offset (y=112 < 560) still crosses onto DP-1 clamped at top=0.
    let to_dp1_top = resolve_cross_monitor_drag_target(
        Some(2),
        (0, 0),
        (1440, 2560),
        (1440 - 380 + 60, 112),
        (1425, 120),
        ov_sz,
        &outputs,
    )
    .expect("pushing right above offset monitor should still migrate");
    assert_eq!(to_dp1_top.output_id, 1);
    assert_eq!(to_dp1_top.margin, (0, 0));

    // Dragging back left from DP-1 onto HDMI-A-1 lands on right edge of output 2.
    let back_to_hdmi = resolve_cross_monitor_drag_target(
        Some(1),
        (1440, 560),
        (2560, 1440),
        (1440 - 60, 800),
        (1420, 810),
        ov_sz,
        &outputs,
    )
    .expect("dragging left from DP-1 must migrate back to HDMI-A-1");
    assert_eq!(back_to_hdmi.output_id, 2);
    assert_eq!(back_to_hdmi.origin, (0, 0));
    assert_eq!(back_to_hdmi.margin, (1440 - 380, 800));
}

#[test]
fn next_monitor_geometry_cycles_cleanly_between_outputs() {
    let monitors = [
        MonitorRect::new(0.0, 0.0, 1440.0, 2560.0),
        MonitorRect::new(1440.0, 560.0, 2560.0, 1440.0),
    ];
    let on_hdmi = OverlayGeometry {
        x: 24.0,
        y: 112.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    let moved_to_dp1 = compute_next_monitor_geometry(on_hdmi, &monitors, 1.0)
        .expect("should cycle to second monitor");
    assert_eq!(moved_to_dp1.x, 1440.0 + 24.0);
    assert_eq!(moved_to_dp1.y, 560.0 + 112.0);
    assert_eq!(moved_to_dp1.width, 380.0);
    assert_eq!(moved_to_dp1.height, 230.0);

    let cycled_back = compute_next_monitor_geometry(moved_to_dp1, &monitors, 1.0)
        .expect("should cycle back to first monitor");
    assert_eq!(cycled_back.x, 24.0);
    assert_eq!(cycled_back.y, 112.0);
}

#[test]
fn anchored_continuous_drag_tracks_global_pointer_across_monitors() {
    let outputs = [
        LogicalOutputBounds {
            id: 2,
            x: 0,
            y: 0,
            width: 1440,
            height: 2560,
        },
        LogicalOutputBounds {
            id: 1,
            x: 1440,
            y: 560,
            width: 2560,
            height: 1440,
        },
    ];
    let anchor_origin = (1060, 938);
    let grab_local = (40.0, 12.0);

    // Moving far right on DP-1 while still grabbing the frozen HDMI-A-1 anchor surface
    let (ptr_dp1, des_dp1) =
        compute_anchored_drag_desktop_pos(anchor_origin, grab_local, (1740.0, -238.0));
    assert_eq!(ptr_dp1, (2800, 700));
    assert_eq!(des_dp1, (2760, 688));
    let dp1_margin = compute_drag_margin_step(
        (des_dp1.0 - 1440, des_dp1.1 - 560),
        (0.0, 0.0),
        (380, 230),
        Some((2560, 1440)),
    );
    assert_eq!(dp1_margin, (1320, 128));

    // Dragging back left onto HDMI-A-1 resolves HDMI-A-1 as the new target
    let (ptr_hdmi, des_hdmi) =
        compute_anchored_drag_desktop_pos(anchor_origin, grab_local, (-200.0, -100.0));
    assert_eq!(ptr_hdmi, (860, 838));
    assert_eq!(des_hdmi, (820, 826));
    let back = resolve_cross_monitor_drag_target(
        Some(1),
        (1440, 560),
        (2560, 1440),
        des_hdmi,
        ptr_hdmi,
        (380, 230),
        &outputs,
    )
    .expect("continuing drag back onto HDMI-A-1 must migrate back");
    assert_eq!(back.output_id, 2);
    assert_eq!(back.margin, (820, 826));
}

#[test]
fn next_logical_output_geometry_cycles_mixed_scale_outputs_both_ways() {
    let outputs = [
        LogicalOutputBounds {
            id: 1,
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        },
        LogicalOutputBounds {
            id: 2,
            x: 1280,
            y: 0,
            width: 1920,
            height: 1080,
        },
    ];
    let on_2x = OverlayGeometry {
        x: 100.0,
        y: 80.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };

    let to_1x = compute_next_logical_output_geometry(on_2x, &outputs)
        .expect("should switch from 2x logical output to 1x logical output");
    assert_eq!(to_1x.x, 1380.0);
    assert_eq!(to_1x.y, 80.0);

    let back_to_2x = compute_next_logical_output_geometry(to_1x, &outputs)
        .expect("should switch back from 1x logical output to 2x logical output");
    assert_eq!(back_to_2x.x, 100.0);
    assert_eq!(back_to_2x.y, 80.0);

    // Oversized relative offset on wider 1x monitor clamps within narrower 2x monitor.
    let far_on_1x = OverlayGeometry {
        x: 2680.0,
        y: 900.0,
        ..on_2x
    };
    let clamped_on_2x = compute_next_logical_output_geometry(far_on_1x, &outputs)
        .expect("clamped 1x -> 2x switch should succeed");
    assert_eq!(clamped_on_2x.x, 1280.0 - 380.0);
    assert_eq!(clamped_on_2x.y, 720.0 - 230.0);
}

#[test]
fn wayland_overlay_context_matches_rasterizer_scale_on_first_dpi_transition_frame() {
    let ctx = egui::Context::default();

    let raw_1x = build_wayland_overlay_raw_input((380, 230), 1, Some(0.0), Vec::new(), true);
    let mut out_1x = ctx.run_ui(raw_1x, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("scale 1x");
        });
    });
    out_1x.textures_delta.clear();
    assert_eq!(out_1x.pixels_per_point, 1.0);

    let raw_2x = build_wayland_overlay_raw_input((380, 230), 2, Some(0.016), Vec::new(), true);
    let mut out_2x = ctx.run_ui(raw_2x, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("scale 2x first frame");
        });
    });
    out_2x.textures_delta.clear();
    assert_eq!(
        out_2x.pixels_per_point, 2.0,
        "1x -> 2x first frame must use 2.0 to match rasterizer"
    );

    let raw_back_1x = build_wayland_overlay_raw_input((380, 230), 1, Some(0.032), Vec::new(), true);
    let mut out_back_1x = ctx.run_ui(raw_back_1x, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("scale 1x first frame");
        });
    });
    out_back_1x.textures_delta.clear();
    assert_eq!(
        out_back_1x.pixels_per_point, 1.0,
        "2x -> 1x first frame must use 1.0 to match rasterizer"
    );
}

#[test]
fn rasterizer_textured_semi_transparent_quad_has_no_diagonal_seam() {
    let mut raster = OverlayRasterizer::new();
    let tex_id = TextureId::User(7);
    let mut delta = TexturesDelta::default();
    delta.push(
        tex_id,
        egui::epaint::ImageDelta::full(
            ColorImage::filled([2, 2], Color32::from_rgba_premultiplied(100, 50, 25, 128)),
            TextureOptions::NEAREST,
        ),
    );
    raster.apply_textures_delta(&mut delta);

    let quad = Rect::from_min_max(pos2(0.0, 0.0), pos2(8.0, 8.0));
    let uvs = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let mut mesh_a = Mesh::with_texture(tex_id);
    push_quad(&mut mesh_a, quad, uvs, Color32::WHITE);
    let prims_a = vec![egui::ClippedPrimitive {
        clip_rect: quad,
        primitive: egui::epaint::Primitive::Mesh(mesh_a),
    }];
    let buf_a = raster
        .rasterize_if_dirty(8, 8, 1.0, &prims_a)
        .expect("textured 8x8 quad must rasterize");
    for y in 0..8 {
        for x in 0..8 {
            let alpha = buf_a[(y * 8 + x) * 4 + 3];
            assert_eq!(
                alpha, 128,
                "diagonal/fill mismatch at ({x}, {y}) in standard split: got alpha {alpha}"
            );
        }
    }

    // Also test the alternate diagonal split [0, 1, 3, 1, 2, 3].
    let mut mesh_b = Mesh::with_texture(tex_id);
    push_quad(&mut mesh_b, quad, uvs, Color32::WHITE);
    mesh_b.indices = vec![0, 1, 3, 1, 2, 3];
    let prims_b = vec![egui::ClippedPrimitive {
        clip_rect: quad,
        primitive: egui::epaint::Primitive::Mesh(mesh_b),
    }];
    raster.invalidate();
    let buf_b = raster
        .rasterize_if_dirty(8, 8, 1.0, &prims_b)
        .expect("alternate-diagonal textured 8x8 quad must rasterize");
    for y in 0..8 {
        for x in 0..8 {
            let alpha = buf_b[(y * 8 + x) * 4 + 3];
            assert_eq!(
                alpha, 128,
                "diagonal/fill mismatch at ({x}, {y}) in alternate split: got alpha {alpha}"
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn queued_picks_geometry_event_does_not_overwrite_build_geometry_on_gsi_transition() {
    use super::overlay_backend::{LayerCommand, LayerEvent};
    use super::wayland_layer::WaylandLayerHandle;
    use truesight_core::ports::DraftEvent;

    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.overlay_geometry = OverlayGeometry {
        x: 24.0,
        y: 96.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    session.build_overlay_geometry = OverlayGeometry {
        x: 600.0,
        y: 450.0,
        width: 420.0,
        height: 168.0,
        click_through: false,
    };

    let (cmd_tx, cmd_rx) =
        smithay_client_toolkit::reexports::calloop::channel::channel::<LayerCommand>();
    let (event_tx, event_rx) = std::sync::mpsc::channel::<LayerEvent>();

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.wayland_layer = Some(WaylandLayerHandle::from_test_channels(cmd_tx, event_rx));
    app.wayland_layer_visible = true;
    app.last_overlay_is_post_draft = Some(false);
    app.wayland_layer_size = (380, 230);

    // GSI transitions from Hero Selection (Picks) to Game In Progress (Build).
    app.session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 0,
    });
    assert!(app.session.is_post_draft_build_view());

    // A final Picks drag/resize event from the compositor was still queued in event_rx.
    event_tx
        .send(LayerEvent::GeometryChanged {
            is_post_draft: false,
            x: 120,
            y: 230,
            width: 380,
            height: 230,
            persist: true,
        })
        .unwrap();

    let main_ctx = egui::Context::default();
    app.step_wayland_layer_overlay(&main_ctx);

    assert_eq!(app.session.overlay_geometry.x, 120.0);
    assert_eq!(app.session.overlay_geometry.y, 230.0);
    assert_eq!(app.session.overlay_geometry.width, 380.0);
    assert_eq!(app.session.overlay_geometry.height, 230.0);

    assert_eq!(app.session.build_overlay_geometry.x, 600.0);
    assert_eq!(app.session.build_overlay_geometry.y, 450.0);
    assert_eq!(app.session.build_overlay_geometry.width, 420.0);
    assert_eq!(app.session.build_overlay_geometry.height, 168.0);
    assert_eq!(app.wayland_layer_size, (420, 168));

    let mut saw_build_set_geometry = false;
    while let Ok(cmd) = cmd_rx.try_recv() {
        if let LayerCommand::SetGeometry {
            is_post_draft,
            x,
            y,
            width,
            height,
            min_w,
            min_h,
        } = cmd
        {
            assert!(is_post_draft);
            assert_eq!(
                (x, y, width, height, min_w, min_h),
                (600, 450, 420, 168, 220, 104)
            );
            saw_build_set_geometry = true;
        }
    }
    assert!(
        saw_build_set_geometry,
        "transition to Build must send Build's saved geometry to the presenter"
    );

    // Even a late in-flight non-persisted Picks drag arriving one frame later must not clobber Build.
    event_tx
        .send(LayerEvent::GeometryChanged {
            is_post_draft: false,
            x: 140,
            y: 250,
            width: 380,
            height: 230,
            persist: false,
        })
        .unwrap();
    app.step_wayland_layer_overlay(&main_ctx);

    assert_eq!(app.session.overlay_geometry.x, 140.0);
    assert_eq!(app.session.overlay_geometry.y, 250.0);
    assert_eq!(app.session.build_overlay_geometry.x, 600.0);
    assert_eq!(app.session.build_overlay_geometry.y, 450.0);
    assert_eq!(app.wayland_layer_size, (420, 168));
}

#[cfg(target_os = "linux")]
#[test]
fn deferred_build_set_geometry_applies_after_picks_drag_and_accepts_build_frame() {
    use super::overlay_backend::LayerEvent;
    use super::wayland_layer::{
        DeferredGeometryQueue, PendingSetGeometry, WaylandLayerHandle, frame_matches_surface_dims,
        surface_geometry_after_set,
    };
    use truesight_core::ports::DraftEvent;

    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.overlay_geometry = OverlayGeometry {
        x: 24.0,
        y: 96.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    session.build_overlay_geometry = OverlayGeometry {
        x: 600.0,
        y: 450.0,
        width: 420.0,
        height: 168.0,
        click_through: false,
    };

    let (cmd_tx, cmd_rx) = smithay_client_toolkit::reexports::calloop::channel::channel::<
        super::overlay_backend::LayerCommand,
    >();
    let (event_tx, event_rx) = std::sync::mpsc::channel::<LayerEvent>();
    let mut app = TrueSightGuiApp::new(session, None, None);
    app.wayland_layer = Some(WaylandLayerHandle::from_test_channels(cmd_tx, event_rx));
    app.wayland_layer_visible = true;
    app.last_overlay_is_post_draft = Some(false);
    app.wayland_layer_size = (380, 230);

    // GSI transitions to Build while the user is still holding down a Picks drag.
    app.session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 0,
    });
    let main_ctx = egui::Context::default();
    app.step_wayland_layer_overlay(&main_ctx);

    let mut pending_cmd = None;
    while let Ok(cmd) = cmd_rx.try_recv() {
        if let super::overlay_backend::LayerCommand::SetGeometry {
            is_post_draft,
            x,
            y,
            width,
            height,
            min_w,
            min_h,
        } = cmd
        {
            pending_cmd = Some(PendingSetGeometry {
                is_post_draft,
                x,
                y,
                width,
                height,
                min_w,
                min_h,
            });
        }
    }
    let build_req = pending_cmd.expect("Build SetGeometry command must be sent on GSI transition");

    let mut queue = DeferredGeometryQueue::default();
    assert!(
        queue.on_set_geometry(true, build_req).is_none(),
        "SetGeometry arriving mid-drag must be deferred until release"
    );
    // While drag is active, surface remains Picks (380x230), so Build 420x168 frame does not match yet.
    assert!(!frame_matches_surface_dims(420, 168, 380, 230, 1));
    assert!(
        queue.on_gesture_finished(true).is_none(),
        "anchored/active gesture still holding must not flush deferred SetGeometry early"
    );

    // Drag finishes at (120, 230): pointer emits Picks GeometryChanged(persist: true) first.
    let drag_end =
        compute_drag_margin_step((24, 96), (96.0, 134.0), (380, 230), Some((2560, 1440)));
    assert_eq!(drag_end, (120, 230));
    event_tx
        .send(LayerEvent::GeometryChanged {
            is_post_draft: false,
            x: drag_end.0,
            y: drag_end.1,
            width: 380,
            height: 230,
            persist: true,
        })
        .unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert_eq!(app.session.overlay_geometry.x, 120.0);
    assert_eq!(app.session.overlay_geometry.y, 230.0);
    assert_eq!(app.session.build_overlay_geometry.x, 600.0);
    assert_eq!(app.session.build_overlay_geometry.y, 450.0);
    assert_eq!(app.wayland_layer_size, (420, 168));

    // Then the presenter flushes the deferred Build SetGeometry command on release.
    let flushed = queue
        .on_gesture_finished(false)
        .expect("releasing Picks drag must flush deferred Build SetGeometry");
    let applied = surface_geometry_after_set(flushed, (0, 0), Some((2560, 1440)));
    assert_eq!(applied, (600, 450, 420, 168, 220, 104, true));
    assert!(frame_matches_surface_dims(
        420, 168, applied.2, applied.3, 1
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn deferred_build_set_geometry_applies_after_picks_resize_and_accepts_build_frame() {
    use super::overlay_backend::LayerEvent;
    use super::wayland_layer::{
        DeferredGeometryQueue, PendingSetGeometry, WaylandLayerHandle, frame_matches_surface_dims,
        surface_geometry_after_set,
    };
    use truesight_core::ports::DraftEvent;

    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.overlay_geometry = OverlayGeometry {
        x: 24.0,
        y: 96.0,
        width: 380.0,
        height: 230.0,
        click_through: false,
    };
    session.build_overlay_geometry = OverlayGeometry {
        x: 600.0,
        y: 450.0,
        width: 420.0,
        height: 168.0,
        click_through: false,
    };

    let (cmd_tx, cmd_rx) = smithay_client_toolkit::reexports::calloop::channel::channel::<
        super::overlay_backend::LayerCommand,
    >();
    let (event_tx, event_rx) = std::sync::mpsc::channel::<LayerEvent>();
    let mut app = TrueSightGuiApp::new(session, None, None);
    app.wayland_layer = Some(WaylandLayerHandle::from_test_channels(cmd_tx, event_rx));
    app.wayland_layer_visible = true;
    app.last_overlay_is_post_draft = Some(false);
    app.wayland_layer_size = (380, 230);

    // GSI switches to Build while user is actively resizing Picks (380x230 -> 500x300).
    app.session.apply_event(DraftEvent::GameRulesStateChanged {
        state_name: "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS".into(),
        game_time_secs: 0,
    });
    let main_ctx = egui::Context::default();
    app.step_wayland_layer_overlay(&main_ctx);

    let mut build_req = None;
    while let Ok(cmd) = cmd_rx.try_recv() {
        if let super::overlay_backend::LayerCommand::SetGeometry {
            is_post_draft,
            x,
            y,
            width,
            height,
            min_w,
            min_h,
        } = cmd
        {
            build_req = Some(PendingSetGeometry {
                is_post_draft,
                x,
                y,
                width,
                height,
                min_w,
                min_h,
            });
        }
    }
    let build_req = build_req.expect("Build SetGeometry command must be queued on GSI switch");

    let mut queue = DeferredGeometryQueue::default();
    assert!(queue.on_set_geometry(true, build_req).is_none());

    // Picks resize continues to obey Picks min bounds (280x160) until released.
    let resized = compute_resize_step((380, 230), (120.0, 70.0), (280, 160), (840, 720));
    assert_eq!(resized, (500, 300));

    // Release emits Picks GeometryChanged(500x300, persist: true) first.
    event_tx
        .send(LayerEvent::GeometryChanged {
            is_post_draft: false,
            x: 24,
            y: 96,
            width: resized.0,
            height: resized.1,
            persist: true,
        })
        .unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert_eq!(app.session.overlay_geometry.width, 500.0);
    assert_eq!(app.session.overlay_geometry.height, 300.0);
    assert_eq!(app.session.build_overlay_geometry.width, 420.0);
    assert_eq!(app.session.build_overlay_geometry.height, 168.0);
    assert_eq!(app.wayland_layer_size, (420, 168));

    // Flushing deferred SetGeometry switches surface dimensions to Build (420x168) so frames commit.
    let flushed = queue
        .on_gesture_finished(false)
        .expect("releasing Picks resize must flush deferred Build SetGeometry");
    let applied = surface_geometry_after_set(flushed, (0, 0), Some((2560, 1440)));
    assert_eq!(applied, (600, 450, 420, 168, 220, 104, true));
    assert!(frame_matches_surface_dims(
        420, 168, applied.2, applied.3, 1
    ));
    assert!(!frame_matches_surface_dims(
        500, 300, applied.2, applied.3, 1
    ));

    // Show/Hide lifecycle clear must drop stale pending SetGeometry.
    assert!(queue.on_set_geometry(true, build_req).is_none());
    queue.clear();
    assert!(queue.on_gesture_finished(false).is_none());
}

#[test]
fn overlay_rasterizer_tiles_repeat_wrapped_textures_seamlessly() {
    let mut raster = OverlayRasterizer::new();
    let tex_id = TextureId::Managed(42);
    let checkerboard = ColorImage {
        size: [2, 2],
        source_size: vec2(2.0, 2.0),
        pixels: vec![
            Color32::from_rgba_premultiplied(240, 240, 240, 255),
            Color32::from_rgba_premultiplied(20, 20, 20, 255),
            Color32::from_rgba_premultiplied(20, 20, 20, 255),
            Color32::from_rgba_premultiplied(240, 240, 240, 255),
        ],
    };
    let mut delta = TexturesDelta::default();
    delta.push(
        tex_id,
        egui::epaint::ImageDelta::full(
            checkerboard,
            TextureOptions {
                magnification: TextureFilter::Nearest,
                minification: TextureFilter::Nearest,
                wrap_mode: egui::TextureWrapMode::Repeat,
                mipmap_mode: None,
            },
        ),
    );
    raster.apply_textures_delta(&mut delta);

    let mut mesh = Mesh::with_texture(tex_id);
    push_quad(
        &mut mesh,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 4.0)),
        Rect::from_min_max(pos2(0.0, 0.0), pos2(2.0, 2.0)),
        Color32::WHITE,
    );
    let prims = vec![egui::ClippedPrimitive {
        clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 4.0)),
        primitive: egui::epaint::Primitive::Mesh(mesh),
    }];
    let bytes = raster.rasterize_if_dirty(4, 4, 1.0, &prims).unwrap();
    // Row 0 wraps checkerboard [bright, dark, bright, dark] instead of clamping to the edge texel.
    let p0 = 0;
    let p1 = 4;
    let p2 = 8;
    let p3 = 12;
    assert_eq!(bytes[p0], 240);
    assert_eq!(bytes[p1], 20);
    assert_eq!(bytes[p2], 240);
    assert_eq!(bytes[p3], 20);
}

#[test]
fn wayland_overlay_glass_surface_renders_lower_default_alpha_and_responds_to_opacity() {
    let mut session = AppSession::new().unwrap();
    session.overlay_mode = true;
    session.set_slot_manual(Side::Allies, 0, truesight_core::domain::HeroId(2));
    session.set_slot_manual(Side::Enemies, 0, truesight_core::domain::HeroId(74));

    let mut app = TrueSightGuiApp::new(session, None, None);
    let overlay_ctx = egui::Context::default();
    let mut raster = OverlayRasterizer::new();

    let render_at_opacity = |app: &mut TrueSightGuiApp,
                             overlay_ctx: &egui::Context,
                             raster: &mut OverlayRasterizer,
                             opacity: f32|
     -> (u32, u32) {
        app.session.set_overlay_opacity(opacity);
        let mut out = overlay_ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(380.0, 230.0))),
                ..Default::default()
            },
            |ui| {
                TrueSightGuiApp::configure_visuals_for(ui.ctx());
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        let _ = app.render_overlay_panel_with_host(ui, OverlayHostMode::LayerShell);
                    });
            },
        );
        raster.apply_textures_delta(&mut out.textures_delta);
        let prims = overlay_ctx.tessellate(out.shapes, out.pixels_per_point);
        raster.invalidate();
        let bytes = raster
            .rasterize_if_dirty(380, 230, out.pixels_per_point, &prims)
            .expect("glass overlay should rasterize");
        // Measure translucent interior background pixels (excluding opaque borders/portraits/text).
        let mut glass_alpha_sum = 0_u64;
        let mut glass_count = 0_u64;
        for y in 26..210_usize {
            for x in 16..364_usize {
                let off = (y * 380 + x) * 4;
                let a = bytes[off + 3];
                if a > 0 && a < 225 {
                    glass_alpha_sum += u64::from(a);
                    glass_count += 1;
                }
            }
        }
        assert!(
            glass_count > 1000,
            "overlay must contain broad translucent glass field"
        );
        let max_border_alpha = (0..380_usize)
            .map(|x| u32::from(bytes[x * 4 + 3]))
            .max()
            .unwrap_or(0);
        ((glass_alpha_sum / glass_count) as u32, max_border_alpha)
    };

    let (default_alpha, border_alpha) = render_at_opacity(
        &mut app,
        &overlay_ctx,
        &mut raster,
        crate::session::DEFAULT_OVERLAY_OPACITY,
    );
    assert!(
        (120..=185).contains(&default_alpha),
        "default glass overlay interior alpha must be noticeably more transparent than legacy 235 (expected 120..=185), got {default_alpha}"
    );
    assert!(
        border_alpha >= 220,
        "outer emerald glass frame must stay crisp even when base fill is more transparent: {border_alpha}"
    );

    let (lower_alpha, _) = render_at_opacity(&mut app, &overlay_ctx, &mut raster, 0.42);
    assert!(
        lower_alpha + 20 < default_alpha,
        "lowering overlay_opacity to 0.42 must visibly reduce glass alpha ({lower_alpha} vs {default_alpha})"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn resize_pacer_coalesces_in_flight_motion_and_advances_on_commit() {
    use super::wayland_layer::ResizePacer;

    let mut pacer = ResizePacer::new();

    // No-op when size has not changed.
    assert_eq!(pacer.on_motion((380, 230), (380, 230)), None);

    // First resize step fires immediately and marks a frame in flight.
    assert_eq!(pacer.on_motion((380, 230), (390, 235)), Some((390, 235)));

    // Subsequent high-rate pointer motions while frame (390, 235) is rendering must coalesce
    // instead of bumping active.width/height prematurely (which would reject the in-flight frame).
    assert_eq!(pacer.on_motion((390, 235), (400, 240)), None);
    assert_eq!(pacer.on_motion((390, 235), (412, 248)), None);

    // Committing the in-flight frame advances immediately to the coalesced target (412, 248).
    assert_eq!(pacer.on_frame_committed((390, 235)), Some((412, 248)));

    // Committing (412, 248) with nothing newer queued clears the in-flight gate.
    assert_eq!(pacer.on_frame_committed((412, 248)), None);
    assert_eq!(pacer.on_motion((412, 248), (416, 250)), Some((416, 250)));

    // Finishing the gesture while a newer target (420, 255) is queued flushes it once and resets.
    assert_eq!(pacer.on_motion((416, 250), (420, 255)), None);
    assert_eq!(pacer.on_finish((416, 250)), Some((420, 255)));
    assert_eq!(pacer.on_motion((420, 255), (424, 258)), Some((424, 258)));
}

#[cfg(target_os = "linux")]
#[test]
fn resize_pacer_advances_after_compositor_adjusted_configure() {
    use super::wayland_layer::ResizePacer;

    let mut pacer = ResizePacer::new();
    // Initial configure before any motion must remain idle.
    pacer.on_configured((380, 230));
    assert_eq!(pacer.on_motion((380, 230), (450, 260)), Some((450, 260)));

    // Queue a newer pointer motion while (450, 260) is in flight, then simulate compositor
    // clamping configure to (440, 260).
    assert_eq!(pacer.on_motion((450, 260), (460, 270)), None);
    pacer.on_configured((440, 260));

    // Committing the compositor-accepted (440, 260) frame must advance to the queued (460, 270).
    assert_eq!(pacer.on_frame_committed((440, 260)), Some((460, 270)));
}

#[test]
fn rasterizer_fast_paths_match_uniform_uv_flipped_glow_and_repeat_grain() {
    let mut raster = OverlayRasterizer::new();

    // 1. Multi-pixel atlas texture (4x4) with constant UV on pixel (2, 1) -> green
    let atlas_id = TextureId::Managed(10);
    let mut atlas_pixels = vec![Color32::BLACK; 16];
    atlas_pixels[4 + 2] = Color32::from_rgba_premultiplied(20, 200, 60, 255);

    // 2. Linear ClampToEdge glow texture (4x4) peaking at UV (0, 0) -> (1, 1) transparent
    let glow_id = TextureId::Managed(11);
    let mut glow_pixels = Vec::with_capacity(16);
    for y in 0..4_usize {
        for x in 0..4_usize {
            let a = (255usize.saturating_sub((x + y) * 42)) as u8;
            glow_pixels.push(Color32::from_rgba_premultiplied(a, a, a, a));
        }
    }

    // 3. Linear Repeat grain texture (4x4) with distinct columns to verify wrap tiling
    let grain_id = TextureId::Managed(12);
    let mut grain_pixels = Vec::with_capacity(16);
    for _y in 0..4_usize {
        for x in 0..4_usize {
            let v = match x {
                0 => 40,
                1 => 120,
                2 => 200,
                _ => 160,
            };
            grain_pixels.push(Color32::from_rgba_premultiplied(v, v, v, 255));
        }
    }

    let mut delta = TexturesDelta::default();
    delta.push(
        atlas_id,
        egui::epaint::ImageDelta::full(
            ColorImage {
                size: [4, 4],
                source_size: vec2(4.0, 4.0),
                pixels: atlas_pixels,
            },
            TextureOptions::NEAREST,
        ),
    );
    delta.push(
        glow_id,
        egui::epaint::ImageDelta::full(
            ColorImage {
                size: [4, 4],
                source_size: vec2(4.0, 4.0),
                pixels: glow_pixels,
            },
            TextureOptions {
                magnification: TextureFilter::Linear,
                minification: TextureFilter::Linear,
                wrap_mode: egui::TextureWrapMode::ClampToEdge,
                mipmap_mode: None,
            },
        ),
    );
    delta.push(
        grain_id,
        egui::epaint::ImageDelta::full(
            ColorImage {
                size: [4, 4],
                source_size: vec2(4.0, 4.0),
                pixels: grain_pixels,
            },
            TextureOptions {
                magnification: TextureFilter::Linear,
                minification: TextureFilter::Linear,
                wrap_mode: egui::TextureWrapMode::Repeat,
                mipmap_mode: None,
            },
        ),
    );
    raster.apply_textures_delta(&mut delta);

    let mut atlas_mesh = Mesh::with_texture(atlas_id);
    push_quad(
        &mut atlas_mesh,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 4.0)),
        Rect::from_min_max(pos2(0.625, 0.375), pos2(0.625, 0.375)),
        Color32::WHITE,
    );

    let mut flipped_glow = Mesh::with_texture(glow_id);
    flipped_glow.vertices.extend_from_slice(&[
        Vertex {
            pos: pos2(0.0, 4.0),
            uv: pos2(1.0, 1.0),
            color: Color32::from_rgba_unmultiplied(255, 255, 255, 180),
        },
        Vertex {
            pos: pos2(8.0, 4.0),
            uv: pos2(0.0, 1.0),
            color: Color32::from_rgba_unmultiplied(255, 255, 255, 180),
        },
        Vertex {
            pos: pos2(8.0, 12.0),
            uv: pos2(0.0, 0.0),
            color: Color32::from_rgba_unmultiplied(255, 255, 255, 180),
        },
        Vertex {
            pos: pos2(0.0, 12.0),
            uv: pos2(1.0, 0.0),
            color: Color32::from_rgba_unmultiplied(255, 255, 255, 180),
        },
    ]);
    flipped_glow.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);

    let mut tiled_grain = Mesh::with_texture(grain_id);
    push_quad(
        &mut tiled_grain,
        Rect::from_min_max(pos2(8.0, 0.0), pos2(16.0, 8.0)),
        Rect::from_min_max(pos2(0.0, 0.0), pos2(2.0, 2.0)),
        Color32::WHITE,
    );

    let prims = vec![
        egui::ClippedPrimitive {
            clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(16.0, 12.0)),
            primitive: egui::epaint::Primitive::Mesh(atlas_mesh),
        },
        egui::ClippedPrimitive {
            clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(16.0, 12.0)),
            primitive: egui::epaint::Primitive::Mesh(flipped_glow),
        },
        egui::ClippedPrimitive {
            clip_rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(16.0, 12.0)),
            primitive: egui::epaint::Primitive::Mesh(tiled_grain),
        },
    ];
    let bytes = raster.rasterize_if_dirty(16, 12, 1.0, &prims).unwrap();

    // Uniform-UV atlas quad pixel (1, 1) -> exact [B=60, G=200, R=20, A=255]
    let atlas_off = (16 + 1) * 4;
    assert_eq!(&bytes[atlas_off..atlas_off + 4], &[60, 200, 20, 255]);

    // Flipped glow: bottom-right pixel (7, 11) has UV closest to (0, 0) and must be brighter
    // than top-left pixel (0, 4) whose UV is closest to (1, 1).
    let glow_tl = (4 * 16) * 4;
    let glow_br = (11 * 16 + 7) * 4;
    assert!(
        bytes[glow_br + 3] > bytes[glow_tl + 3] + 80,
        "flipped bottom-right glow alpha {} must dominate top-left {}",
        bytes[glow_br + 3],
        bytes[glow_tl + 3]
    );

    // Tiled Repeat grain in [8..16, 0..8] repeats with period 4px: col (8+k) matches col (12+k).
    for k in 0..4_usize {
        let first_tile = (8 + k) * 4;
        let second_tile = (12 + k) * 4;
        assert_eq!(bytes[first_tile], bytes[second_tile]);
        assert_ne!(bytes[(8) * 4], bytes[(10) * 4]);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn wayland_compositor_closed_keeps_auto_overlay_eligible_and_recovers_on_available_output() {
    use super::overlay_backend::{LayerCommand, LayerEvent};
    use super::wayland_layer::WaylandLayerHandle;

    let mut session = AppSession::new().unwrap();
    session.overlay_mode = false;
    session.auto_overlay_with_dota = true;
    session.set_dota_process_running(true);

    let (cmd_tx, cmd_rx) =
        smithay_client_toolkit::reexports::calloop::channel::channel::<LayerCommand>();
    let (event_tx, event_rx) = std::sync::mpsc::channel::<LayerEvent>();

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.overlay_backend = OverlayBackendKind::WaylandLayerShell;
    app.wayland_layer = Some(WaylandLayerHandle::from_test_channels(cmd_tx, event_rx));

    let main_ctx = egui::Context::default();
    let drain_show_count = || -> usize {
        let mut shows = 0;
        while let Ok(cmd) = cmd_rx.try_recv() {
            if matches!(cmd, LayerCommand::Show { .. }) {
                shows += 1;
            }
        }
        shows
    };

    // Initial auto-open emits Show while Dota 2 is running.
    app.step_wayland_layer_overlay(&main_ctx);
    assert_eq!(drain_show_count(), 1, "initial step must show auto-overlay");
    assert!(app.session.is_overlay_visible());

    // Compositor teardown (`LayerEvent::Closed`) must reset layer surface state WITHOUT
    // setting `auto_overlay_dismissed`, allowing immediate recovery on an available output.
    event_tx.send(LayerEvent::Closed).unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert!(
        app.session.is_overlay_visible(),
        "compositor-driven Closed must keep auto-overlay eligible while Dota is running"
    );
    assert!(
        !app.session.auto_overlay_dismissed,
        "compositor Closed must not set session-long user dismissal"
    );
    assert_eq!(
        drain_show_count(),
        1,
        "overlay must recover and emit Show after compositor Closed while outputs exist"
    );

    // When all outputs temporarily disappear on Wayland, wait until an output returns before showing.
    event_tx
        .send(LayerEvent::MonitorsUpdated {
            outputs: Vec::new(),
            has_any_output: false,
        })
        .unwrap();
    event_tx.send(LayerEvent::Closed).unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert!(app.session.is_overlay_visible());
    assert_eq!(
        drain_show_count(),
        0,
        "must not respawn layer surface while compositor reports zero outputs"
    );

    event_tx
        .send(LayerEvent::MonitorsUpdated {
            outputs: vec![LogicalOutputBounds {
                id: 1,
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }],
            has_any_output: true,
        })
        .unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert_eq!(
        drain_show_count(),
        1,
        "auto-overlay must recover as soon as an output becomes available again"
    );

    // Explicit user dismissal (`X` / `F8`) still suppresses auto-reopen until Dota exits.
    app.session.dismiss_overlay();
    app.step_wayland_layer_overlay(&main_ctx);
    assert!(!app.session.is_overlay_visible());
    assert!(app.session.auto_overlay_dismissed);
    assert_eq!(drain_show_count(), 0);
}

#[cfg(target_os = "linux")]
#[test]
fn wayland_presenter_last_output_destroyed_emits_empty_monitors_and_waits_for_recovery() {
    use super::overlay_backend::{LayerCommand, LayerEvent};
    use super::wayland_layer::{WaylandLayerHandle, build_monitors_updated_event};

    let out1 = LogicalOutputBounds {
        id: 1,
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let out2 = LogicalOutputBounds {
        id: 2,
        x: 1920,
        y: 0,
        width: 2560,
        height: 1440,
    };

    // SCTK 0.20 invokes output_destroyed while output 1 is still in OutputState:
    // presenter must exclude output 1 and report zero available outputs.
    let empty_ev = build_monitors_updated_event(&[1], vec![out1], Some(1));
    assert!(matches!(
        &empty_ev,
        LayerEvent::MonitorsUpdated {
            outputs,
            has_any_output: false
        } if outputs.is_empty()
    ));

    let mut session = AppSession::new().unwrap();
    session.overlay_mode = false;
    session.auto_overlay_with_dota = true;
    session.set_dota_process_running(true);

    let (cmd_tx, cmd_rx) =
        smithay_client_toolkit::reexports::calloop::channel::channel::<LayerCommand>();
    let (event_tx, event_rx) = std::sync::mpsc::channel::<LayerEvent>();

    let mut app = TrueSightGuiApp::new(session, None, None);
    app.overlay_backend = OverlayBackendKind::WaylandLayerShell;
    app.wayland_layer = Some(WaylandLayerHandle::from_test_channels(cmd_tx, event_rx));

    let main_ctx = egui::Context::default();
    let drain_show_count = || -> usize {
        let mut shows = 0;
        while let Ok(cmd) = cmd_rx.try_recv() {
            if matches!(cmd, LayerCommand::Show { .. }) {
                shows += 1;
            }
        }
        shows
    };

    app.step_wayland_layer_overlay(&main_ctx);
    assert_eq!(drain_show_count(), 1);

    // Replay exact presenter-to-GUI events on last output unplug: output_destroyed then Closed.
    event_tx.send(empty_ev).unwrap();
    event_tx.send(LayerEvent::Closed).unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert!(app.session.is_overlay_visible());
    assert!(!app.session.auto_overlay_dismissed);
    assert_eq!(
        drain_show_count(),
        0,
        "GUI must wait without re-creating layer while 0 outputs remain"
    );

    // Reconnect a wl_output WITHOUT optional xdg-output logical bounds: availability still clears
    // wayland_outputs_unavailable so the fallback surface can reopen without logical rects.
    let no_bounds_ev = build_monitors_updated_event(&[3], Vec::new(), None);
    assert!(matches!(
        &no_bounds_ev,
        LayerEvent::MonitorsUpdated {
            outputs,
            has_any_output: true
        } if outputs.is_empty()
    ));
    event_tx.send(no_bounds_ev).unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert!(app.wayland_logical_outputs.is_empty());
    assert_eq!(
        drain_show_count(),
        1,
        "GUI must recover when a wl_output reconnects without optional xdg-output bounds"
    );

    // Unplug again, then reconnect output 2 WITH logical bounds -> recovers too.
    event_tx
        .send(build_monitors_updated_event(&[3], Vec::new(), Some(3)))
        .unwrap();
    event_tx.send(LayerEvent::Closed).unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert_eq!(drain_show_count(), 0);

    let new_ev = build_monitors_updated_event(&[2], vec![out2], None);
    event_tx.send(new_ev).unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert_eq!(
        drain_show_count(),
        1,
        "GUI must recover auto-overlay as soon as new_output reports an available output"
    );

    // Removing 1 of 2 outputs keeps output 2 available and immediately recovers after Closed.
    let remaining_ev = build_monitors_updated_event(&[1, 2], vec![out1, out2], Some(1));
    assert!(matches!(
        &remaining_ev,
        LayerEvent::MonitorsUpdated {
            outputs,
            has_any_output: true
        } if outputs.len() == 1 && outputs[0].id == 2
    ));
    event_tx.send(remaining_ev).unwrap();
    event_tx.send(LayerEvent::Closed).unwrap();
    app.step_wayland_layer_overlay(&main_ctx);
    assert_eq!(
        drain_show_count(),
        1,
        "auto-overlay must immediately recover onto remaining output after Closed"
    );
}
