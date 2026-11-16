use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense,
    Shape, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use std::collections::HashMap;
use truesight_adapters::vision::parse_hero_atlas;
use truesight_core::domain::HeroId;

// =============================================================================
// Gem of Truesight Color System (OKLCH tokens mapped to sRGB)
// =============================================================================
pub(super) const P_VOID: Color32 = Color32::from_rgb(10, 9, 24);
pub(super) const P_INDIGO_1: Color32 = Color32::from_rgb(17, 16, 38);
pub(super) const P_INDIGO_2: Color32 = Color32::from_rgb(25, 24, 54);
pub(super) const P_INDIGO_3: Color32 = Color32::from_rgb(38, 37, 74);
pub(super) const P_LINE: Color32 = Color32::from_rgb(53, 52, 92);
pub(super) const P_SILVER: Color32 = Color32::from_rgb(206, 210, 224);
pub(super) const P_GLINT: Color32 = Color32::from_rgb(244, 250, 252);
pub(super) const P_MUTED: Color32 = Color32::from_rgb(138, 142, 168);
pub(super) const P_GEM: Color32 = Color32::from_rgb(46, 230, 122);
pub(super) const P_GEM_DEEP: Color32 = Color32::from_rgb(18, 128, 72);
pub(super) const P_DIRE: Color32 = Color32::from_rgb(236, 78, 66);
pub(super) const P_DIRE_DEEP: Color32 = Color32::from_rgb(142, 42, 36);
pub(super) const P_NEUTRAL: Color32 = Color32::from_rgb(224, 194, 96);
pub(super) const P_CYAN: Color32 = Color32::from_rgb(76, 198, 232);

pub(super) const EPS_PCT: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Bad,
    Flat,
}

impl Tone {
    #[inline]
    pub fn color(self) -> Color32 {
        match self {
            Self::Good => P_GEM,
            Self::Bad => P_DIRE,
            Self::Flat => P_NEUTRAL,
        }
    }
}

#[inline]
pub fn tone_for_user(pct: f32, sign: f32) -> Tone {
    let x = pct * sign;
    if x >= EPS_PCT {
        Tone::Good
    } else if x <= -EPS_PCT {
        Tone::Bad
    } else {
        Tone::Flat
    }
}

#[inline]
pub fn fmt_val(pct: f32) -> String {
    if pct.abs() < EPS_PCT {
        "±0.0".into()
    } else if pct > 0.0 {
        format!("+{:.1}", pct.abs())
    } else {
        format!("-{:.1}", pct.abs())
    }
}

#[inline]
pub fn fmt_pct(pct: f32) -> String {
    if pct.abs() < EPS_PCT {
        "±0.0%".into()
    } else if pct > 0.0 {
        format!("+{:.1}%", pct.abs())
    } else {
        format!("-{:.1}%", pct.abs())
    }
}

// =============================================================================
// Texture Cache (Backed by Embedded Adapter Hero Atlas)
// =============================================================================

pub(super) struct HeroTextureSet {
    pub(super) color: TextureHandle,
    pub(super) toned: TextureHandle,
    pub(super) banned: TextureHandle,
}

#[derive(Default)]
pub(super) struct GuiTextureCache {
    pub(super) initialized: bool,
    pub(super) gem: Option<TextureHandle>,
    pub(super) heroes: HashMap<HeroId, HeroTextureSet>,
}

impl GuiTextureCache {
    pub(super) fn ensure_loaded(&mut self, ctx: &egui::Context) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        let Some(atlas) = parse_hero_atlas() else {
            return;
        };

        let gem_img =
            egui::ColorImage::from_rgb([atlas.gem_width, atlas.gem_height], &atlas.gem_rgb);
        self.gem = Some(ctx.load_texture("truesight_gem", gem_img, TextureOptions::LINEAR));

        let size = [atlas.hero_width, atlas.hero_height];
        for (hid, rgb) in atlas.heroes {
            let color_img = egui::ColorImage::from_rgb(size, &rgb);
            let toned_rgb = transform_rgb(&rgb, 0.60, 0.80);
            let toned_img = egui::ColorImage::from_rgb(size, &toned_rgb);
            let banned_rgb = transform_rgb(&rgb, 0.90, 0.55);
            let banned_img = egui::ColorImage::from_rgb(size, &banned_rgb);

            let color = ctx.load_texture(
                format!("h_col_{}", hid.0),
                color_img,
                TextureOptions::LINEAR,
            );
            let toned = ctx.load_texture(
                format!("h_ton_{}", hid.0),
                toned_img,
                TextureOptions::LINEAR,
            );
            let banned = ctx.load_texture(
                format!("h_ban_{}", hid.0),
                banned_img,
                TextureOptions::LINEAR,
            );
            self.heroes.insert(
                hid,
                HeroTextureSet {
                    color,
                    toned,
                    banned,
                },
            );
        }
    }
}

fn transform_rgb(rgb: &[u8], gray_mix: f32, brightness: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len());
    for px in rgb.chunks_exact(3) {
        let r = px[0] as f32;
        let g = px[1] as f32;
        let b = px[2] as f32;
        let lum = 0.299 * r + 0.587 * g + 0.114 * b;
        let nr = ((r * (1.0 - gray_mix) + lum * gray_mix) * brightness).clamp(0.0, 255.0) as u8;
        let ng = ((g * (1.0 - gray_mix) + lum * gray_mix) * brightness).clamp(0.0, 255.0) as u8;
        let nb = ((b * (1.0 - gray_mix) + lum * gray_mix) * brightness).clamp(0.0, 255.0) as u8;
        out.extend_from_slice(&[nr, ng, nb]);
    }
    out
}

// =============================================================================
// Custom Painter Primitives (HUD Cards, Portraits, Tinted Square Cells, Bars)
// =============================================================================

pub(super) fn uv_full() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

/// Center-crops a 16:9 (64x36) texture into a target rect aspect ratio without stretching.
pub(super) fn uv_aspect_crop(target_size: Vec2) -> Rect {
    let src_aspect = 16.0_f32 / 9.0_f32;
    let dst_aspect = (target_size.x / target_size.y.max(1.0)).max(0.5);
    if dst_aspect >= src_aspect {
        let keep_h = (src_aspect / dst_aspect).clamp(0.1, 1.0);
        let pad_y = (1.0 - keep_h) * 0.5;
        Rect::from_min_max(pos2(0.0, pad_y), pos2(1.0, 1.0 - pad_y))
    } else {
        let keep_w = (dst_aspect / src_aspect).clamp(0.1, 1.0);
        let pad_x = (1.0 - keep_w) * 0.5;
        Rect::from_min_max(pos2(pad_x, 0.0), pos2(1.0 - pad_x, 1.0))
    }
}

pub(super) fn uv_square_crop() -> Rect {
    uv_aspect_crop(vec2(1.0, 1.0))
}

pub(super) fn hud_card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) -> Rect {
    let frame = egui::Frame::NONE
        .fill(P_INDIGO_1)
        .stroke(Stroke::new(1.0_f32, P_LINE))
        .corner_radius(CornerRadius::same(2))
        .inner_margin(Margin::same(10));
    let prepared = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        add_contents(ui);
    });
    paint_hud_corner_ticks(ui.painter(), prepared.response.rect, P_GEM);
    prepared.response.rect
}

pub(super) fn hud_card_fill(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) -> Rect {
    let frame = egui::Frame::NONE
        .fill(P_INDIGO_1)
        .stroke(Stroke::new(1.0_f32, P_LINE))
        .corner_radius(CornerRadius::same(2))
        .inner_margin(Margin::same(10));
    let prepared = frame.show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        add_contents(ui);
    });
    paint_hud_corner_ticks(ui.painter(), prepared.response.rect, P_GEM);
    prepared.response.rect
}

pub(super) fn paint_hud_corner_ticks(painter: &egui::Painter, rect: Rect, color: Color32) {
    let tick = 9.0;
    let stroke = Stroke::new(2.0_f32, color);
    // Top-left L tick
    let tl = rect.min;
    painter.line_segment([tl, pos2(tl.x + tick, tl.y)], stroke);
    painter.line_segment([tl, pos2(tl.x, tl.y + tick)], stroke);
    // Bottom-right L tick
    let br = rect.max;
    painter.line_segment([pos2(br.x - tick, br.y), br], stroke);
    painter.line_segment([pos2(br.x, br.y - tick), br], stroke);
}

pub(super) fn paint_portrait_16_9(
    ui: &mut egui::Ui,
    tex: Option<&TextureHandle>,
    size: Vec2,
    border_color: Color32,
    border_width: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, P_INDIGO_3);
    if let Some(t) = tex {
        painter.image(t.id(), rect, uv_full(), Color32::WHITE);
    }
    if border_width > 0.0 {
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(border_width, border_color),
            StrokeKind::Inside,
        );
    }
    resp
}

pub(super) fn paint_tinted_cell(
    ui: &mut egui::Ui,
    cell_size: Vec2,
    tex: Option<&TextureHandle>,
    delta_pct: f32,
    font_size: f32,
    ring_width: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(cell_size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, P_INDIGO_2);

    if let Some(t) = tex {
        painter.image(t.id(), rect, uv_aspect_crop(cell_size), Color32::WHITE);
    }

    let a = (delta_pct.abs() / 2.4).clamp(0.0, 1.0);
    let alpha = ((0.55 + 0.45 * a) * 0.50 * 255.0).round() as u8;
    let (tint_fill, ring_col) = if delta_pct >= EPS_PCT {
        (Color32::from_rgba_unmultiplied(46, 230, 122, alpha), P_GEM)
    } else if delta_pct <= -EPS_PCT {
        (Color32::from_rgba_unmultiplied(236, 78, 66, alpha), P_DIRE)
    } else {
        (Color32::TRANSPARENT, P_LINE)
    };

    if tint_fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, 0.0, tint_fill);
    }

    // Bottom dark legibility shade
    let shade_top = rect.min.y + rect.height() * 0.62;
    let shade_rect = Rect::from_min_max(pos2(rect.min.x, shade_top), rect.max);
    painter.rect_filled(
        shade_rect,
        0.0,
        Color32::from_rgba_unmultiplied(10, 9, 24, 190),
    );

    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(ring_width, ring_col),
        StrokeKind::Inside,
    );

    painter.text(
        pos2(rect.center().x, rect.max.y - 2.0),
        Align2::CENTER_BOTTOM,
        fmt_val(delta_pct),
        FontId::proportional(font_size),
        P_GLINT,
    );
    resp
}

pub(super) fn paint_tinted_square_cell(
    ui: &mut egui::Ui,
    size: f32,
    tex: Option<&TextureHandle>,
    delta_pct: f32,
    font_size: f32,
    ring_width: f32,
) -> egui::Response {
    paint_tinted_cell(ui, vec2(size, size), tex, delta_pct, font_size, ring_width)
}

pub(super) fn paint_avg_square_cell(ui: &mut egui::Ui, size: Vec2, avg_pct: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, P_INDIGO_2);
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, P_LINE), StrokeKind::Inside);

    let val_font = (size.x * 0.25).clamp(7.5, 11.0);
    let y_off = (size.y * 0.13).clamp(3.0, 5.5);
    if let Some(pct) = avg_pct {
        let col = tone_for_user(pct, 1.0).color();
        painter.text(
            pos2(rect.center().x, rect.center().y - y_off),
            Align2::CENTER_CENTER,
            fmt_pct(pct),
            FontId::proportional(val_font),
            col,
        );
        // Centered zero-tick bar
        let bar_w = rect.width() * 0.72;
        let bar_h = (size.y * 0.09).clamp(2.5, 4.0);
        let bar_rect = Rect::from_center_size(
            pos2(rect.center().x, rect.center().y + y_off + 2.0),
            vec2(bar_w, bar_h),
        );
        painter.rect_filled(bar_rect, 0.0, P_INDIGO_3);
        let frac = (pct.abs() / 1.5).clamp(0.0, 1.0) * 0.5;
        let mid_x = bar_rect.center().x;
        let fill_rect = if pct >= 0.0 {
            Rect::from_min_max(
                pos2(mid_x, bar_rect.min.y),
                pos2(mid_x + bar_w * frac, bar_rect.max.y),
            )
        } else {
            Rect::from_min_max(
                pos2(mid_x - bar_w * frac, bar_rect.min.y),
                pos2(mid_x, bar_rect.max.y),
            )
        };
        painter.rect_filled(fill_rect, 0.0, col);
        painter.line_segment(
            [
                pos2(mid_x, bar_rect.min.y - 2.0),
                pos2(mid_x, bar_rect.max.y + 2.0),
            ],
            Stroke::new(1.0_f32, P_MUTED),
        );
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "...",
            FontId::proportional(val_font),
            P_MUTED,
        );
    }
}

pub(super) fn paint_total_corner_cell(ui: &mut egui::Ui, size: Vec2, edge_pct: f32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    let col = tone_for_user(edge_pct, 1.0).color();
    let bg = if edge_pct >= EPS_PCT {
        Color32::from_rgba_unmultiplied(46, 230, 122, 32)
    } else if edge_pct <= -EPS_PCT {
        Color32::from_rgba_unmultiplied(236, 78, 66, 32)
    } else {
        P_INDIGO_2
    };
    let val_font = (size.x * 0.25).clamp(7.5, 11.5);
    let sub_font = (size.x * 0.17).clamp(6.0, 8.5);
    let y_off = (size.y * 0.13).clamp(3.0, 5.5);
    painter.rect_filled(rect, 0.0, bg);
    painter.rect_stroke(rect, 0.0, Stroke::new(1.5_f32, col), StrokeKind::Inside);
    painter.text(
        pos2(rect.center().x, rect.center().y - y_off),
        Align2::CENTER_CENTER,
        fmt_pct(edge_pct),
        FontId::proportional(val_font),
        col,
    );
    painter.text(
        pos2(rect.center().x, rect.center().y + y_off + 2.5),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(sub_font),
        P_MUTED,
    );
}

pub(super) fn paint_matrix_scale_bar(ui: &mut egui::Ui, left_label: &str, right_label: &str) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, 14.0), Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(left_label).size(9.0).color(P_MUTED));
        let rem = (ui.available_width() - 62.0).max(40.0);
        let (bar_r, _) = ui.allocate_exact_size(vec2(rem, 4.0), Sense::hover());
        let mid = bar_r.center().x;
        ui.painter().rect_filled(
            Rect::from_min_max(bar_r.min, pos2(mid, bar_r.max.y)),
            0.0,
            P_DIRE,
        );
        ui.painter().rect_filled(
            Rect::from_min_max(pos2(mid, bar_r.min.y), bar_r.max),
            0.0,
            P_GEM,
        );
        ui.painter().line_segment(
            [pos2(mid, bar_r.min.y - 2.0), pos2(mid, bar_r.max.y + 2.0)],
            Stroke::new(1.0_f32, P_GLINT),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(right_label).size(9.0).color(P_MUTED));
        });
    });
}

pub(super) fn paint_mini_badge_scaled(
    ui: &mut egui::Ui,
    text: &str,
    text_color: Color32,
    border_color: Color32,
    font_size: f32,
    pad_x: i8,
) {
    egui::Frame::NONE
        .fill(P_INDIGO_1)
        .stroke(Stroke::new(1.0_f32, border_color))
        .inner_margin(Margin::symmetric(pad_x, 1))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .size(font_size)
                    .strong()
                    .color(text_color),
            );
        });
}

pub(super) fn paint_mini_badge(
    ui: &mut egui::Ui,
    text: &str,
    text_color: Color32,
    border_color: Color32,
) {
    paint_mini_badge_scaled(ui, text, text_color, border_color, 8.5, 4);
}

pub(super) fn paint_trend_badge_scaled(
    ui: &mut egui::Ui,
    trend_pct: f32,
    font_size: f32,
    pad_x: i8,
) {
    if trend_pct.abs() < EPS_PCT {
        return;
    }
    let up = trend_pct > 0.0;
    let col = if up { P_GEM } else { P_DIRE };
    let border = if up { P_GEM_DEEP } else { P_DIRE_DEEP };
    egui::Frame::NONE
        .fill(P_INDIGO_1)
        .stroke(Stroke::new(1.0_f32, border))
        .inner_margin(Margin::symmetric(pad_x, 1))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = if pad_x <= 2 { 1.5 } else { 2.0 };
                let tri_sz = if font_size < 8.0 { 5.0 } else { 6.0 };
                let (tri_rect, _) = ui.allocate_exact_size(vec2(tri_sz, tri_sz), Sense::hover());
                let pts = if up {
                    vec![
                        pos2(tri_rect.center().x, tri_rect.min.y),
                        pos2(tri_rect.max.x, tri_rect.max.y),
                        pos2(tri_rect.min.x, tri_rect.max.y),
                    ]
                } else {
                    vec![
                        pos2(tri_rect.min.x, tri_rect.min.y),
                        pos2(tri_rect.max.x, tri_rect.min.y),
                        pos2(tri_rect.center().x, tri_rect.max.y),
                    ]
                };
                ui.painter()
                    .add(Shape::convex_polygon(pts, col, Stroke::NONE));
                ui.label(
                    RichText::new(format!("{:.1}", trend_pct.abs()))
                        .size(font_size)
                        .strong()
                        .color(col),
                );
            });
        });
}

/// Builds a 64x64 RGBA window/taskbar icon centered on the embedded Gem of True Sight.
pub(crate) fn build_gem_window_icon() -> Option<egui::IconData> {
    let atlas = parse_hero_atlas()?;
    if atlas.gem_width == 0 || atlas.gem_height == 0 {
        return None;
    }
    let expected_len = atlas
        .gem_width
        .checked_mul(atlas.gem_height)?
        .checked_mul(3)?;
    if atlas.gem_rgb.len() < expected_len {
        return None;
    }

    const OUT_SIZE: usize = 64;
    const GEM_W: usize = 56;
    const GEM_H: usize = 42;
    const OFF_X: usize = (OUT_SIZE - GEM_W) / 2;
    const OFF_Y: usize = (OUT_SIZE - GEM_H) / 2;

    let mut rgba = vec![0u8; OUT_SIZE * OUT_SIZE * 4];
    for y in 0..OUT_SIZE {
        for x in 0..OUT_SIZE {
            let idx = (y * OUT_SIZE + x) * 4;
            let inner = 3..OUT_SIZE - 3;
            let in_corner_cut = !inner.contains(&x) && !inner.contains(&y);
            if in_corner_cut {
                continue;
            }
            let is_border = x == 0
                || y == 0
                || x == OUT_SIZE - 1
                || y == OUT_SIZE - 1
                || ((x == 1 || x == OUT_SIZE - 2) && inner.contains(&y))
                || ((y == 1 || y == OUT_SIZE - 2) && inner.contains(&x));
            let base = if is_border { P_GEM_DEEP } else { P_VOID };
            rgba[idx] = base.r();
            rgba[idx + 1] = base.g();
            rgba[idx + 2] = base.b();
            rgba[idx + 3] = 255;
        }
    }

    // Bilinear-sample the embedded 4:3 Gem RGB raster inside the dark TrueSight badge.
    for dy in 0..GEM_H {
        let sy = (dy as f32 + 0.5) * (atlas.gem_height as f32 / GEM_H as f32) - 0.5;
        let y0 = (sy.floor() as isize).clamp(0, atlas.gem_height as isize - 1) as usize;
        let y1 = (y0 + 1).min(atlas.gem_height - 1);
        let wy = (sy - y0 as f32).clamp(0.0, 1.0);

        for dx in 0..GEM_W {
            let sx = (dx as f32 + 0.5) * (atlas.gem_width as f32 / GEM_W as f32) - 0.5;
            let x0 = (sx.floor() as isize).clamp(0, atlas.gem_width as isize - 1) as usize;
            let x1 = (x0 + 1).min(atlas.gem_width - 1);
            let wx = (sx - x0 as f32).clamp(0.0, 1.0);

            let sample = |x: usize, y: usize, c: usize| -> f32 {
                atlas.gem_rgb[(y * atlas.gem_width + x) * 3 + c] as f32
            };
            let out_idx = ((OFF_Y + dy) * OUT_SIZE + (OFF_X + dx)) * 4;
            for c in 0..3 {
                let top = sample(x0, y0, c) * (1.0 - wx) + sample(x1, y0, c) * wx;
                let bot = sample(x0, y1, c) * (1.0 - wx) + sample(x1, y1, c) * wx;
                let val = (top * (1.0 - wy) + bot * wy).round().clamp(0.0, 255.0) as u8;
                rgba[out_idx + c] = val;
            }
            rgba[out_idx + 3] = 255;
        }
    }

    Some(egui::IconData {
        rgba,
        width: OUT_SIZE as u32,
        height: OUT_SIZE as u32,
    })
}
