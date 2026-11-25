use super::theme::{P_GEM, P_GEM_DEEP, P_INDIGO_1, P_VOID, paint_hud_corner_ticks};
use crate::session::AppSession;
use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, Mesh, Rect, Shape, Stroke, TextureFilter, TextureId,
    TextureOptions, TextureWrapMode, epaint::Vertex, pos2,
};

/// Pixel width and height of the procedural tileable gemstone micro-grain texture.
pub(super) const MICRO_GRAIN_SIZE: usize = 128;

const GRAIN_CTX_DATA_KEY: &str = "truesight_gem_micro_grain_tex_id";

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

/// Returns seamless `Repeat`-wrapped linear sampling options for the procedural micro-grain texture.
pub(super) fn micro_grain_texture_options() -> TextureOptions {
    TextureOptions {
        magnification: TextureFilter::Linear,
        minification: TextureFilter::Linear,
        wrap_mode: TextureWrapMode::Repeat,
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
    // Map upper 24 bits into [-1.0, 1.0].
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
            // Subtle directional crystalline striation + multi-octave periodic grain.
            let diag = hash_lattice_2d(x, y, 0x517C_C1B7_2722_0A95) * 0.42;
            let mid = smooth_periodic_sample(x, y, 8, 0x2545_F491_4F6C_DD1D) * 0.38;
            let broad = smooth_periodic_sample(x, y, 32, 0x8F4C_A113_B7D2_901B) * 0.20;
            let n = (diag + mid + broad).clamp(-1.0, 1.0);
            let lum = (132.0 + n * 44.0).round().clamp(64.0, 208.0) as u8;
            // Subtle crisp variance in alpha keeps grain feather-light under white vertex tint.
            let alpha = (232.0 + n * 18.0).round().clamp(205.0, 255.0) as u8;
            rgba.extend_from_slice(&[lum, lum, lum, alpha]);
        }
    }
    ColorImage::from_rgba_unmultiplied([MICRO_GRAIN_SIZE, MICRO_GRAIN_SIZE], &rgba)
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

/// Builds a multi-layer faceted Gem of True Sight dark-crystal card surface background
/// intended to be inserted behind card content.
pub(super) fn build_gemstone_card_background(
    rect: Rect,
    grain_tex: Option<TextureId>,
    corner_radius: CornerRadius,
) -> Shape {
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);
    let tl = rect.min;
    let tr = rect.right_top();
    let br = rect.max;
    let bl = rect.left_bottom();

    let mut shapes = Vec::with_capacity(8);
    shapes.push(Shape::rect_filled(rect, corner_radius, P_INDIGO_1));

    // Upper-left emerald crystal sheen facet
    shapes.push(Shape::convex_polygon(
        vec![
            tl,
            pos2(tl.x + w * 0.56, tl.y),
            pos2(tl.x + w * 0.24, tl.y + h * 0.58),
            pos2(tl.x, tl.y + h * 0.42),
        ],
        Color32::from_rgba_unmultiplied(46, 230, 122, 11),
        Stroke::NONE,
    ));

    // Upper-right cool prism catch facet
    shapes.push(Shape::convex_polygon(
        vec![
            pos2(tl.x + w * 0.56, tl.y),
            tr,
            pos2(tr.x, tl.y + h * 0.36),
            pos2(tl.x + w * 0.68, tl.y + h * 0.48),
        ],
        Color32::from_rgba_unmultiplied(128, 148, 224, 10),
        Stroke::NONE,
    ));

    // Lower-right obsidian depth shadow facet
    shapes.push(Shape::convex_polygon(
        vec![
            pos2(tl.x + w * 0.38, br.y),
            pos2(tl.x + w * 0.72, tl.y + h * 0.46),
            pos2(br.x, tl.y + h * 0.34),
            br,
        ],
        Color32::from_rgba_unmultiplied(6, 5, 16, 58),
        Stroke::NONE,
    ));

    push_tiled_grain_mesh(&mut shapes, rect, grain_tex, 16);

    // Top-left crystal specular bevel highlights
    shapes.push(Shape::line_segment(
        [pos2(tl.x + 1.0, tl.y + 1.0), pos2(tr.x - 1.0, tr.y + 1.0)],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(244, 250, 252, 26)),
    ));
    shapes.push(Shape::line_segment(
        [
            pos2(tl.x + 1.0, tl.y + 1.0),
            pos2(bl.x + 1.0, tl.y + h * 0.55),
        ],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(46, 230, 122, 34)),
    ));

    Shape::Vec(shapes)
}

/// Builds a translucent multi-layer emerald glass background for the compact overlay backed by
/// true per-pixel alpha (`ARGB8888`) scaled by user-configurable `opacity`.
pub(super) fn build_overlay_glass_background(
    rect: Rect,
    opacity: f32,
    grain_tex: Option<TextureId>,
) -> Shape {
    let op = AppSession::sanitize_overlay_opacity(opacity);
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);
    let tl = rect.min;
    let br = rect.max;

    let base_alpha = (255.0 * op).round().clamp(40.0, 245.0) as u8;
    let obsidian_alpha = (28.0 * op).round().clamp(6.0, 48.0) as u8;
    let emerald_sheen_alpha = (18.0 * op + 4.0).round().clamp(6.0, 32.0) as u8;
    let glint_sheen_alpha = (14.0 * op + 3.0).round().clamp(5.0, 26.0) as u8;
    let grain_alpha = (11.0 * op).round().clamp(3.0, 18.0) as u8;

    let mut shapes = Vec::with_capacity(6);
    shapes.push(Shape::rect_filled(
        rect,
        CornerRadius::same(2),
        Color32::from_rgba_unmultiplied(P_VOID.r(), P_VOID.g(), P_VOID.b(), base_alpha),
    ));

    // Subtle lower-right obsidian shadow facet for depth without raising overall fill opacity.
    shapes.push(Shape::convex_polygon(
        vec![
            pos2(tl.x + w * 0.34, br.y),
            pos2(tl.x + w * 0.70, tl.y + h * 0.44),
            pos2(br.x, tl.y + h * 0.30),
            br,
        ],
        Color32::from_rgba_unmultiplied(6, 5, 16, obsidian_alpha),
        Stroke::NONE,
    ));

    // Upper-left emerald glass sheen
    shapes.push(Shape::convex_polygon(
        vec![
            tl,
            pos2(tl.x + w * 0.54, tl.y),
            pos2(tl.x + w * 0.18, tl.y + h * 0.62),
            pos2(tl.x, tl.y + h * 0.44),
        ],
        Color32::from_rgba_unmultiplied(P_GEM.r(), P_GEM.g(), P_GEM.b(), emerald_sheen_alpha),
        Stroke::NONE,
    ));

    // Diagonal glass specular streak
    shapes.push(Shape::convex_polygon(
        vec![
            pos2(tl.x + w * 0.14, tl.y),
            pos2(tl.x + w * 0.32, tl.y),
            pos2(tl.x + w * 0.08, tl.y + h * 0.48),
            pos2(tl.x, tl.y + h * 0.30),
        ],
        Color32::from_rgba_unmultiplied(244, 250, 252, glint_sheen_alpha),
        Stroke::NONE,
    ));

    push_tiled_grain_mesh(&mut shapes, rect, grain_tex, grain_alpha);

    Shape::Vec(shapes)
}

/// Paints crisp emerald glass edge framing (outer border, inner hairline, specular bevel, corner ticks)
/// over the translucent overlay background.
pub(super) fn paint_overlay_glass_frame(painter: &egui::Painter, rect: Rect) {
    let outer_col =
        Color32::from_rgba_unmultiplied(P_GEM_DEEP.r(), P_GEM_DEEP.g(), P_GEM_DEEP.b(), 235);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.5_f32, outer_col),
        egui::StrokeKind::Inside,
    );

    let inner = rect.shrink(2.0);
    if inner.width() > 4.0 && inner.height() > 4.0 {
        painter.rect_stroke(
            inner,
            CornerRadius::same(1),
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(46, 230, 122, 76)),
            egui::StrokeKind::Inside,
        );
        painter.line_segment(
            [
                pos2(inner.min.x + 1.0, inner.min.y + 0.5),
                pos2(inner.min.x + (inner.width() * 0.48), inner.min.y + 0.5),
            ],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(244, 250, 252, 48)),
        );
    }

    paint_hud_corner_ticks(painter, rect, P_GEM);
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
