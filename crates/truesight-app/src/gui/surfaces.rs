use super::theme::{P_GEM, P_GEM_DEEP, P_INDIGO_1, P_VOID, paint_hud_corner_ticks};
use crate::session::AppSession;
use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, Mesh, Rect, Shape, Stroke, TextureFilter, TextureId,
    TextureOptions, TextureWrapMode, epaint::Vertex, pos2,
};

/// Pixel width and height of the procedural tileable gemstone micro-grain texture.
pub(super) const MICRO_GRAIN_SIZE: usize = 128;

/// Pixel width and height of the continuous radial gemstone glow falloff texture.
pub(super) const RADIAL_GLOW_SIZE: usize = 64;

const GRAIN_CTX_DATA_KEY: &str = "truesight_gem_micro_grain_tex_id";
const GLOW_CTX_DATA_KEY: &str = "truesight_gem_radial_glow_tex_id";

/// Stores the uploaded micro-grain `TextureId` inside `egui::Context` temporary state so stateless
/// surface helpers (`hud_card`, `hud_card_fill`) can composite the texture without changing call signatures.
pub(super) fn store_gem_grain_texture_id(ctx: &egui::Context, id: TextureId) {
    ctx.data_mut(|d| {
        d.insert_temp(egui::Id::new(GRAIN_CTX_DATA_KEY), id);
    });
}

/// Retrieves the uploaded micro-grain `TextureId` from `egui::Context` when initialized.
pub(super) fn gem_grain_texture_id(ctx: &egui::Context) -> Option<TextureId> {
    ctx.data(|d| d.get_temp::<TextureId>(egui::Id::new(GRAIN_CTX_DATA_KEY)))
}

/// Stores the uploaded continuous radial glow `TextureId` inside `egui::Context` temporary state.
pub(super) fn store_gem_glow_texture_id(ctx: &egui::Context, id: TextureId) {
    ctx.data_mut(|d| {
        d.insert_temp(egui::Id::new(GLOW_CTX_DATA_KEY), id);
    });
}

/// Retrieves the uploaded continuous radial glow `TextureId` from `egui::Context` when initialized.
pub(super) fn gem_glow_texture_id(ctx: &egui::Context) -> Option<TextureId> {
    ctx.data(|d| d.get_temp::<TextureId>(egui::Id::new(GLOW_CTX_DATA_KEY)))
}

/// Returns seamless `Repeat`-wrapped linear sampling options for the procedural micro-grain texture.
pub(super) fn micro_grain_texture_options() -> TextureOptions {
    TextureOptions {
        magnification: TextureFilter::Linear,
        minification: TextureFilter::Linear,
        wrap_mode: TextureWrapMode::Repeat,
        mipmap_mode: None,
    }
}

/// Returns bilinear `ClampToEdge` sampling options for the continuous radial gemstone glow texture.
pub(super) fn radial_glow_texture_options() -> TextureOptions {
    TextureOptions {
        magnification: TextureFilter::Linear,
        minification: TextureFilter::Linear,
        wrap_mode: TextureWrapMode::ClampToEdge,
        mipmap_mode: None,
    }
}

#[inline]
fn hash_lattice_2d(x: usize, y: usize, seed: u64) -> f32 {
    let tx = (x & (MICRO_GRAIN_SIZE - 1)) as u64;
    let ty = (y & (MICRO_GRAIN_SIZE - 1)) as u64;
    let mut v =
        tx.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ ty.wrapping_mul(0xBF58_476D_1CE4_E5B9) ^ seed;
    v ^= v >> 30;
    v = v.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    v ^= v >> 27;
    v = v.wrapping_mul(0x94D0_49BB_1331_11EB);
    v ^= v >> 31;
    let unit = ((v >> 40) as f32) / ((1u32 << 24) - 1) as f32;
    unit * 2.0 - 1.0
}

#[inline]
fn smooth_periodic_sample(x: usize, y: usize, cell: usize, seed: u64) -> f32 {
    let gx0 = (x / cell) * cell;
    let gy0 = (y / cell) * cell;
    let gx1 = gx0 + cell;
    let gy1 = gy0 + cell;
    let fx = (x - gx0) as f32 / cell as f32;
    let fy = (y - gy0) as f32 / cell as f32;
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);

    let n00 = hash_lattice_2d(gx0, gy0, seed);
    let n10 = hash_lattice_2d(gx1, gy0, seed);
    let n01 = hash_lattice_2d(gx0, gy1, seed);
    let n11 = hash_lattice_2d(gx1, gy1, seed);

    let top = n00 + (n10 - n00) * sx;
    let bot = n01 + (n11 - n01) * sx;
    top + (bot - top) * sy
}

/// Generates a deterministic, seamlessly tileable `128x128` subtle crystalline grain texture.
pub(super) fn generate_micro_grain_image() -> ColorImage {
    let mut rgba = Vec::with_capacity(MICRO_GRAIN_SIZE * MICRO_GRAIN_SIZE * 4);
    for y in 0..MICRO_GRAIN_SIZE {
        for x in 0..MICRO_GRAIN_SIZE {
            let diag = hash_lattice_2d(x, y, 0x517C_C1B7_2722_0A95) * 0.42;
            let mid = smooth_periodic_sample(x, y, 8, 0x2545_F491_4F6C_DD1D) * 0.38;
            let broad = smooth_periodic_sample(x, y, 32, 0x8F4C_A113_B7D2_901B) * 0.20;
            let n = (diag + mid + broad).clamp(-1.0, 1.0);
            let lum = (132.0 + n * 44.0).round().clamp(64.0, 208.0) as u8;
            let alpha = (232.0 + n * 18.0).round().clamp(205.0, 255.0) as u8;
            rgba.extend_from_slice(&[lum, lum, lum, alpha]);
        }
    }
    ColorImage::from_rgba_unmultiplied([MICRO_GRAIN_SIZE, MICRO_GRAIN_SIZE], &rgba)
}

/// Generates a continuous `64x64` smooth radial falloff texture anchored at UV `(0, 0)` that
/// decays smoothly to `0` before reaching the opposite edges so GPU and `wl_shm` rasterization
/// render true optical light without stepped polygon bands.
pub(super) fn generate_radial_glow_image() -> ColorImage {
    let mut rgba = Vec::with_capacity(RADIAL_GLOW_SIZE * RADIAL_GLOW_SIZE * 4);
    for y in 0..RADIAL_GLOW_SIZE {
        let v = (y as f32 + 0.5) / RADIAL_GLOW_SIZE as f32;
        for x in 0..RADIAL_GLOW_SIZE {
            let u = (x as f32 + 0.5) / RADIAL_GLOW_SIZE as f32;
            let r = ((u / 0.92).powi(2) + (v / 0.92).powi(2))
                .sqrt()
                .clamp(0.0, 1.0);
            let t = 1.0 - r;
            let weight = t * t * (3.0 - 2.0 * t) * t;
            let alpha = (weight * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba.extend_from_slice(&[255, 255, 255, alpha]);
        }
    }
    ColorImage::from_rgba_unmultiplied([RADIAL_GLOW_SIZE, RADIAL_GLOW_SIZE], &rgba)
}

fn push_radial_glow_mesh(
    shapes: &mut Vec<Shape>,
    rect: Rect,
    glow_tex: Option<TextureId>,
    tint: Color32,
    flip_br: bool,
) {
    let Some(tex_id) = glow_tex else {
        return;
    };
    if tint.a() == 0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let (uv_lt, uv_rt, uv_rb, uv_lb) = if flip_br {
        (
            pos2(1.0, 1.0),
            pos2(0.0, 1.0),
            pos2(0.0, 0.0),
            pos2(1.0, 0.0),
        )
    } else {
        (
            pos2(0.0, 0.0),
            pos2(1.0, 0.0),
            pos2(1.0, 1.0),
            pos2(0.0, 1.0),
        )
    };
    let mut mesh = Mesh::with_texture(tex_id);
    mesh.vertices.extend_from_slice(&[
        Vertex {
            pos: rect.left_top(),
            uv: uv_lt,
            color: tint,
        },
        Vertex {
            pos: rect.right_top(),
            uv: uv_rt,
            color: tint,
        },
        Vertex {
            pos: rect.right_bottom(),
            uv: uv_rb,
            color: tint,
        },
        Vertex {
            pos: rect.left_bottom(),
            uv: uv_lb,
            color: tint,
        },
    ]);
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    shapes.push(Shape::mesh(mesh));
}

fn push_tiled_grain_mesh(
    shapes: &mut Vec<Shape>,
    rect: Rect,
    grain_tex: Option<TextureId>,
    alpha: u8,
) {
    let Some(tex_id) = grain_tex else {
        return;
    };
    if alpha == 0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let u_max = rect.width() / MICRO_GRAIN_SIZE as f32;
    let v_max = rect.height() / MICRO_GRAIN_SIZE as f32;
    let tint = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
    let mut mesh = Mesh::with_texture(tex_id);
    mesh.vertices.extend_from_slice(&[
        Vertex {
            pos: rect.left_top(),
            uv: pos2(0.0, 0.0),
            color: tint,
        },
        Vertex {
            pos: rect.right_top(),
            uv: pos2(u_max, 0.0),
            color: tint,
        },
        Vertex {
            pos: rect.right_bottom(),
            uv: pos2(u_max, v_max),
            color: tint,
        },
        Vertex {
            pos: rect.left_bottom(),
            uv: pos2(0.0, v_max),
            color: tint,
        },
    ]);
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    shapes.push(Shape::mesh(mesh));
}

/// Builds a multi-layer Gem of True Sight dark-crystal card surface background combining
/// smooth continuous radial falloff light, sculpted cut-stone facet planes with specular
/// crystal ridges, and subtle tiled micro-grain behind card content.
pub(super) fn build_gemstone_card_background(
    rect: Rect,
    grain_tex: Option<TextureId>,
    glow_tex: Option<TextureId>,
    corner_radius: CornerRadius,
) -> Shape {
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);
    let tl = rect.min;
    let tr = rect.right_top();
    let br = rect.max;
    let bl = rect.left_bottom();

    let mut shapes = Vec::with_capacity(11);
    shapes.push(Shape::rect_filled(rect, corner_radius, P_INDIGO_1));

    // Smooth upper-left emerald inner radiance + lower-right obsidian depth vignette
    push_radial_glow_mesh(
        &mut shapes,
        rect,
        glow_tex,
        Color32::from_rgba_unmultiplied(P_GEM.r(), P_GEM.g(), P_GEM.b(), 22),
        false,
    );
    push_radial_glow_mesh(
        &mut shapes,
        rect,
        glow_tex,
        Color32::from_rgba_unmultiplied(6, 5, 16, 36),
        true,
    );

    // Sculpted cut-stone crystal facet planes + faint 1px specular ridge creases
    let p_top = pos2(tl.x + w * 0.58, tl.y);
    let p_mid = pos2(tl.x + w * 0.34, tl.y + h * 0.52);
    let p_left = pos2(tl.x, tl.y + h * 0.38);
    let p_right = pos2(tr.x, tl.y + h * 0.30);
    let p_piv = pos2(tl.x + w * 0.66, tl.y + h * 0.44);
    let p_bot = pos2(tl.x + w * 0.44, br.y);

    shapes.push(Shape::convex_polygon(
        vec![tl, p_top, p_mid, p_left],
        Color32::from_rgba_unmultiplied(P_GEM.r(), P_GEM.g(), P_GEM.b(), 11),
        Stroke::NONE,
    ));
    shapes.push(Shape::convex_polygon(
        vec![p_top, tr, p_right, p_piv],
        Color32::from_rgba_unmultiplied(150, 178, 236, 9),
        Stroke::NONE,
    ));
    shapes.push(Shape::convex_polygon(
        vec![p_bot, p_piv, p_right, br],
        Color32::from_rgba_unmultiplied(5, 4, 15, 54),
        Stroke::NONE,
    ));

    shapes.push(Shape::line_segment(
        [p_top, p_mid],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(244, 250, 252, 18)),
    ));
    shapes.push(Shape::line_segment(
        [p_piv, p_right],
        Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(P_GEM.r(), P_GEM.g(), P_GEM.b(), 16),
        ),
    ));

    push_tiled_grain_mesh(&mut shapes, rect, grain_tex, 13);

    // Subtle crystal edge glint along top and left rim
    shapes.push(Shape::line_segment(
        [pos2(tl.x + 3.0, tl.y + 1.0), pos2(tr.x - 3.0, tr.y + 1.0)],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(244, 250, 252, 24)),
    ));
    shapes.push(Shape::line_segment(
        [
            pos2(tl.x + 1.0, tl.y + 3.0),
            pos2(bl.x + 1.0, tl.y + h * 0.50),
        ],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(46, 230, 122, 28)),
    ));

    Shape::Vec(shapes)
}

/// Builds a translucent sculpted emerald glass background for the compact overlay combining true
/// per-pixel alpha (`ARGB8888`), continuous smooth radial glow, subtle facet planes, and fine grain.
pub(super) fn build_overlay_glass_background(
    rect: Rect,
    opacity: f32,
    grain_tex: Option<TextureId>,
    glow_tex: Option<TextureId>,
) -> Shape {
    let op = AppSession::sanitize_overlay_opacity(opacity);
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);
    let tl = rect.min;
    let br = rect.max;

    let base_alpha = (255.0 * op).round().clamp(40.0, 245.0) as u8;
    let emerald_alpha = (22.0 * op + 4.0).round().clamp(8.0, 32.0) as u8;
    let obsidian_alpha = (22.0 * op).round().clamp(6.0, 36.0) as u8;
    let crown_alpha = (16.0 * op + 4.0).round().clamp(6.0, 28.0) as u8;
    let pavilion_alpha = (24.0 * op).round().clamp(6.0, 40.0) as u8;
    let ridge_alpha = (20.0 * op + 6.0).round().clamp(8.0, 32.0) as u8;
    let grain_alpha = (10.0 * op).round().clamp(3.0, 16.0) as u8;

    let mut shapes = Vec::with_capacity(7);
    shapes.push(Shape::rect_filled(
        rect,
        CornerRadius::same(6),
        Color32::from_rgba_unmultiplied(P_VOID.r(), P_VOID.g(), P_VOID.b(), base_alpha),
    ));

    push_radial_glow_mesh(
        &mut shapes,
        rect,
        glow_tex,
        Color32::from_rgba_unmultiplied(P_GEM.r(), P_GEM.g(), P_GEM.b(), emerald_alpha),
        false,
    );
    push_radial_glow_mesh(
        &mut shapes,
        rect,
        glow_tex,
        Color32::from_rgba_unmultiplied(6, 5, 16, obsidian_alpha),
        true,
    );

    // Sculpted glass crown & pavilion planes + faint specular ridge crease
    let g_top = pos2(tl.x + w * 0.56, tl.y);
    let g_mid = pos2(tl.x + w * 0.28, tl.y + h * 0.54);
    shapes.push(Shape::convex_polygon(
        vec![tl, g_top, g_mid, pos2(tl.x, tl.y + h * 0.36)],
        Color32::from_rgba_unmultiplied(P_GEM.r(), P_GEM.g(), P_GEM.b(), crown_alpha),
        Stroke::NONE,
    ));
    shapes.push(Shape::convex_polygon(
        vec![
            pos2(tl.x + w * 0.42, br.y),
            pos2(tl.x + w * 0.68, tl.y + h * 0.44),
            pos2(br.x, tl.y + h * 0.28),
            br,
        ],
        Color32::from_rgba_unmultiplied(5, 4, 15, pavilion_alpha),
        Stroke::NONE,
    ));
    shapes.push(Shape::line_segment(
        [g_top, g_mid],
        Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(244, 250, 252, ridge_alpha),
        ),
    ));

    push_tiled_grain_mesh(&mut shapes, rect, grain_tex, grain_alpha);

    Shape::Vec(shapes)
}

/// Paints crisp polished emerald cabochon glass framing (rounded outer border, continuous inner hairline,
/// top specular highlight, and emerald corner ticks) over the translucent overlay background.
pub(super) fn paint_overlay_glass_frame(painter: &egui::Painter, rect: Rect) {
    let outer_col =
        Color32::from_rgba_unmultiplied(P_GEM_DEEP.r(), P_GEM_DEEP.g(), P_GEM_DEEP.b(), 235);
    painter.rect_stroke(
        rect,
        CornerRadius::same(6),
        Stroke::new(1.5_f32, outer_col),
        egui::StrokeKind::Inside,
    );

    let inner = rect.shrink(2.0);
    if inner.width() > 4.0 && inner.height() > 4.0 {
        painter.rect_stroke(
            inner,
            CornerRadius::same(4),
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(46, 230, 122, 84)),
            egui::StrokeKind::Inside,
        );
        painter.line_segment(
            [
                pos2(inner.min.x + 5.0, inner.min.y + 0.5),
                pos2(inner.min.x + (inner.width() * 0.44), inner.min.y + 0.5),
            ],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(244, 250, 252, 52)),
        );
    }

    paint_hud_corner_ticks(painter, rect.shrink(1.0), P_GEM);
}

#[inline]
pub(super) fn translucent(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

/// Renders a translucent matrix/header placeholder cell with optional centered label.
pub(super) fn paint_empty_matrix_slot(
    painter: &egui::Painter,
    rect: Rect,
    fill_alpha: u8,
    border_alpha: u8,
    label: Option<&str>,
    font_size: f32,
) {
    painter.rect_filled(rect, CornerRadius::same(2), translucent(P_VOID, fill_alpha));
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, translucent(super::theme::P_LINE, border_alpha)),
        egui::StrokeKind::Inside,
    );
    if let Some(txt) = label {
        let galley = painter.layout_no_wrap(
            txt.to_string(),
            egui::FontId::proportional(font_size),
            super::theme::P_SILVER,
        );
        let max_sz = rect.shrink(1.0).size().max(egui::vec2(1.0, 1.0));
        let badge_sz = (galley.size() + egui::vec2(4.0, 2.0)).min(max_sz);
        let badge_rect = Rect::from_center_size(rect.center(), badge_sz);
        painter.rect_filled(
            badge_rect,
            CornerRadius::same(2),
            translucent(P_VOID, OVERLAY_TEXT_SCRIM_ALPHA),
        );
        painter.galley(
            badge_rect.center() - galley.size() * 0.5,
            galley,
            super::theme::P_SILVER,
        );
    }
}

/// Localized dark obsidian scrim alpha behind compact overlay labels/stats so small text stays
/// WCAG AA legible over bright game scenes while preserving surrounding translucent glass.
pub(super) const OVERLAY_TEXT_SCRIM_ALPHA: u8 = 216;
/// High-contrast coral-red used for `<50%` stats inside compact overlay text scrims (`>= 4.5:1` AA).
pub(super) const OVERLAY_DIRE_TEXT: Color32 = Color32::from_rgb(255, 132, 122);

/// Wraps compact overlay typography or stat rows in a localized dark scrim backing.
pub(super) fn overlay_text_scrim(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(
            P_VOID.r(),
            P_VOID.g(),
            P_VOID.b(),
            OVERLAY_TEXT_SCRIM_ALPHA,
        ))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(4, 1))
        .show(ui, add_contents);
}

/// Renders a single small label inside [`overlay_text_scrim`] with consistent contrast backing.
pub(super) fn overlay_scrim_label(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    size: f32,
    strong: bool,
    color: Color32,
) {
    overlay_text_scrim(ui, |ui| {
        let mut rt = egui::RichText::new(text).size(size).color(color);
        if strong {
            rt = rt.strong();
        }
        ui.label(rt);
    });
}

/// Paints top crystal specular glint and corner ticks on HUD modals (`OPTIONS`, `ACCOUNT`, `QUICK GUIDE`).
pub(super) fn paint_modal_gemstone_accents(painter: &egui::Painter, rect: Rect) {
    let inset = rect.shrink(1.5);
    if inset.width() > 8.0 && inset.height() > 8.0 {
        painter.line_segment(
            [inset.left_top(), inset.right_top()],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(244, 250, 252, 34)),
        );
        painter.line_segment(
            [
                inset.left_top(),
                pos2(inset.min.x, inset.min.y + inset.height() * 0.35),
            ],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(46, 230, 122, 42)),
        );
    }
    paint_hud_corner_ticks(painter, rect, P_GEM);
}
