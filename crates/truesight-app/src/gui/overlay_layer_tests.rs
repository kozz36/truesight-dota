use super::TrueSightGuiApp;
use super::overlay_backend::{
    OverlayBackendKind, OverlayHostMode, OverlayRect, compute_drag_margin_step,
    compute_resize_step, resolve_overlay_backend,
};
use super::overlay_raster::OverlayRasterizer;
use crate::AppSession;
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
    delta.set.push((
        white_tex,
        egui::epaint::ImageDelta::full(
            ColorImage::filled([1, 1], Color32::WHITE),
            TextureOptions::NEAREST,
        ),
    ));
    raster.apply_textures_delta(&delta);

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
    delta1.set.push((
        tex_id,
        egui::epaint::ImageDelta::full(
            ColorImage::filled([4, 4], Color32::from_rgba_premultiplied(10, 20, 30, 255)),
            TextureOptions {
                magnification: TextureFilter::Nearest,
                minification: TextureFilter::Nearest,
                ..Default::default()
            },
        ),
    ));
    raster.apply_textures_delta(&delta1);
    assert_eq!(raster.texture_count(), 1);

    // Patch pixel [2, 1] only
    let mut delta2 = TexturesDelta::default();
    delta2.set.push((
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
    ));
    raster.apply_textures_delta(&delta2);

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
    free_delta.free.push(tex_id);
    raster.apply_textures_delta(&free_delta);
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
    let out_picks = overlay_ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(380.0, 230.0))),
            ..Default::default()
        },
        |ctx| {
            TrueSightGuiApp::configure_visuals_for(ctx);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let _ = app.render_overlay_panel_with_host(ui, OverlayHostMode::LayerShell);
                });
        },
    );
    raster.apply_textures_delta(&out_picks.textures_delta);
    let prims_picks = overlay_ctx.tessellate(out_picks.shapes, out_picks.pixels_per_point);
    let buf_picks = raster
        .rasterize_if_dirty(380, 230, out_picks.pixels_per_point, &prims_picks)
        .expect("picks overlay should rasterize");
    let mut saw_translucent_bg = false;
    let mut saw_opaque_fg = false;
    for px in buf_picks.chunks_exact(4) {
        if px[3] > 180 && px[3] < 250 {
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
    let out_build = overlay_ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(420.0, 168.0))),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let _ = app.render_overlay_panel_with_host(ui, OverlayHostMode::LayerShell);
                });
        },
    );
    raster.apply_textures_delta(&out_build.textures_delta);
    let prims_build = overlay_ctx.tessellate(out_build.shapes, out_build.pixels_per_point);
    let buf_build = raster
        .rasterize_if_dirty(420, 168, out_build.pixels_per_point, &prims_build)
        .expect("build overlay should rasterize");
    assert!(buf_build.chunks_exact(4).any(|px| px[3] > 0));
}
