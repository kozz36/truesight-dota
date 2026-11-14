use crate::AppSession;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Margin, Rect, RichText,
    Sense, Shape, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use std::collections::HashMap;
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_core::domain::{
    AttackType, Bracket, DraftPick, Faction, HeroId, Role, Side, TEAM_SIZE,
};
use truesight_core::math::{DraftAnalysis5v5, MatchupMatrix};
use truesight_core::ports::DraftEventSource;

const HERO_ATLAS_BYTES: &[u8] = include_bytes!("../assets/hero_atlas.bin");

// =============================================================================
// Gem of Truesight Color System (OKLCH tokens mapped to sRGB)
// =============================================================================
const P_VOID: Color32 = Color32::from_rgb(10, 9, 24);
const P_INDIGO_1: Color32 = Color32::from_rgb(17, 16, 38);
const P_INDIGO_2: Color32 = Color32::from_rgb(25, 24, 54);
const P_INDIGO_3: Color32 = Color32::from_rgb(38, 37, 74);
const P_LINE: Color32 = Color32::from_rgb(53, 52, 92);
const P_SILVER: Color32 = Color32::from_rgb(206, 210, 224);
const P_GLINT: Color32 = Color32::from_rgb(244, 250, 252);
const P_MUTED: Color32 = Color32::from_rgb(138, 142, 168);
const P_GEM: Color32 = Color32::from_rgb(46, 230, 122);
const P_GEM_DEEP: Color32 = Color32::from_rgb(18, 128, 72);
const P_DIRE: Color32 = Color32::from_rgb(236, 78, 66);
const P_DIRE_DEEP: Color32 = Color32::from_rgb(142, 42, 36);
const P_NEUTRAL: Color32 = Color32::from_rgb(224, 194, 96);
const P_CYAN: Color32 = Color32::from_rgb(76, 198, 232);

const EPS_PCT: f32 = 0.05;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixTab {
    Counter,
    Synergy,
    Comp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerTarget {
    Slot(Side, usize),
    Ban,
}

// =============================================================================
// Pure Telemetry & Diagnostic View-Models
// =============================================================================

#[derive(Debug, Clone, Copy)]
pub struct SlotBreakdown {
    pub meta: f32,
    pub cnt: f32,
    pub syn: f32,
    pub net: f32,
}

pub fn compute_slot_breakdown(
    session: &AppSession,
    analysis: &DraftAnalysis5v5,
    side: Side,
    slot: usize,
) -> Option<SlotBreakdown> {
    if slot >= TEAM_SIZE {
        return None;
    }
    let default_role = Role::ALL.get(slot).copied().unwrap_or(Role::Carry);
    match side {
        Side::Allies => {
            let pick = session.draft.allies[slot]?;
            let role = pick.role.unwrap_or(default_role);
            let base_wr = session
                .matrix
                .hero(pick.hero_id)
                .map(|h| h.meta_winrate(role))
                .unwrap_or(0.50);
            let meta = base_wr - 0.50;
            let cnt = analysis.ally_counter_avg[slot].unwrap_or(0.0);
            let syn = analysis.ally_synergy_avg[slot].unwrap_or(0.0);
            let net = analysis.ally_slot_net_delta[slot].unwrap_or(meta + cnt + syn);
            Some(SlotBreakdown {
                meta,
                cnt,
                syn,
                net,
            })
        }
        Side::Enemies => {
            let pick = session.draft.enemies[slot]?;
            let role = pick.role.unwrap_or(default_role);
            let base_wr = session
                .matrix
                .hero(pick.hero_id)
                .map(|h| h.meta_winrate(role))
                .unwrap_or(0.50);
            let meta = base_wr - 0.50;
            // Enemy perspective counter average is the negation of the column mean in counter_grid
            let cnt = enemy_counter_col_avg(analysis, slot)
                .map(|col_avg| -col_avg)
                .unwrap_or(0.0);
            let mut syn_sum = 0.0_f32;
            let mut syn_cnt = 0_u32;
            for (l, other_opt) in session.draft.enemies.iter().enumerate() {
                if l == slot {
                    continue;
                }
                if let Some(other) = other_opt {
                    syn_sum += session.matrix.cell(pick.hero_id, other.hero_id).synergy_delta;
                    syn_cnt += 1;
                }
            }
            let syn = if syn_cnt > 0 {
                syn_sum / syn_cnt as f32
            } else {
                0.0
            };
            let net = analysis.enemy_slot_net_delta[slot].unwrap_or(meta + cnt + syn);
            Some(SlotBreakdown {
                meta,
                cnt,
                syn,
                net,
            })
        }
    }
}

/// Arithmetic mean of column `enemy_slot` in `analysis.counter_grid` (from Ally perspective).
pub fn enemy_counter_col_avg(analysis: &DraftAnalysis5v5, enemy_slot: usize) -> Option<f32> {
    if enemy_slot >= TEAM_SIZE {
        return None;
    }
    let mut sum = 0.0_f32;
    let mut cnt = 0_u32;
    for row in &analysis.counter_grid {
        if let Some(d) = row[enemy_slot] {
            sum += d;
            cnt += 1;
        }
    }
    if cnt > 0 {
        Some(sum / cnt as f32)
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct CompDiagnostic {
    pub picked: u32,
    pub melee: Vec<HeroId>,
    pub ranged: Vec<HeroId>,
    pub disables: Vec<HeroId>,
    pub initiators: Vec<HeroId>,
    pub rule_active: [bool; 6],
    pub score: f32,
}

pub const COMP_RULES: [(&str, f32); 6] = [
    ("5 melee", -0.018),
    ("4 melee (>= 4 picked)", -0.007),
    ("5 ranged", -0.008),
    ("No disable (>= 4 picked)", -0.015),
    ("Only 1 disable (5 picked)", -0.005),
    ("Balanced: 1-3 melee, >= 2 dis, >= 1 init", 0.005),
];

pub fn compute_comp_diagnostic(
    picks: &[Option<DraftPick>; TEAM_SIZE],
    matrix: &MatchupMatrix,
) -> CompDiagnostic {
    let mut picked = 0_u32;
    let mut melee = Vec::with_capacity(TEAM_SIZE);
    let mut ranged = Vec::with_capacity(TEAM_SIZE);
    let mut disables = Vec::with_capacity(TEAM_SIZE);
    let mut initiators = Vec::with_capacity(TEAM_SIZE);

    for pick in picks.iter().flatten() {
        if let Some(h) = matrix.hero(pick.hero_id) {
            picked += 1;
            match h.attack_type {
                AttackType::Melee => melee.push(h.id),
                AttackType::Ranged => ranged.push(h.id),
            }
            if h.has_disable {
                disables.push(h.id);
            }
            if h.has_initiation {
                initiators.push(h.id);
            }
        }
    }

    let mut rule_active = [false; 6];
    let mut score = 0.0_f32;

    if picked >= 3 {
        let m = melee.len() as u32;
        let r = ranged.len() as u32;
        let d = disables.len() as u32;
        let ini = initiators.len() as u32;

        if m == 5 {
            rule_active[0] = true;
            score -= 0.018;
        } else if m == 4 && picked >= 4 {
            rule_active[1] = true;
            score -= 0.007;
        } else if r == 5 {
            rule_active[2] = true;
            score -= 0.008;
        }

        if picked >= 4 && d == 0 {
            rule_active[3] = true;
            score -= 0.015;
        } else if picked == 5 && d == 1 {
            rule_active[4] = true;
            score -= 0.005;
        }

        if picked >= 4 && (1..=3).contains(&m) && d >= 2 && ini >= 1 {
            rule_active[5] = true;
            score += 0.005;
        }
    }

    CompDiagnostic {
        picked,
        melee,
        ranged,
        disables,
        initiators,
        rule_active,
        score,
    }
}

// =============================================================================
// Embedded Hero Atlas Decoder & Texture Cache
// =============================================================================

pub struct DecodedHeroAtlas {
    pub hero_width: usize,
    pub hero_height: usize,
    pub gem_width: usize,
    pub gem_height: usize,
    pub gem_rgb: Vec<u8>,
    pub heroes: HashMap<HeroId, Vec<u8>>,
}

pub fn parse_hero_atlas() -> Option<DecodedHeroAtlas> {
    if HERO_ATLAS_BYTES.len() < 14 || &HERO_ATLAS_BYTES[0..4] != b"TSHA" {
        return None;
    }
    let count = u16::from_le_bytes([HERO_ATLAS_BYTES[4], HERO_ATLAS_BYTES[5]]) as usize;
    let hero_width = u16::from_le_bytes([HERO_ATLAS_BYTES[6], HERO_ATLAS_BYTES[7]]) as usize;
    let hero_height = u16::from_le_bytes([HERO_ATLAS_BYTES[8], HERO_ATLAS_BYTES[9]]) as usize;
    let gem_width = u16::from_le_bytes([HERO_ATLAS_BYTES[10], HERO_ATLAS_BYTES[11]]) as usize;
    let gem_height = u16::from_le_bytes([HERO_ATLAS_BYTES[12], HERO_ATLAS_BYTES[13]]) as usize;

    let gem_len = gem_width.checked_mul(gem_height)?.checked_mul(3)?;
    let mut offset = 14usize;
    let gem_end = offset.checked_add(gem_len)?;
    if gem_end > HERO_ATLAS_BYTES.len() {
        return None;
    }
    let gem_rgb = HERO_ATLAS_BYTES[offset..gem_end].to_vec();
    offset = gem_end;

    let hero_len = hero_width.checked_mul(hero_height)?.checked_mul(3)?;
    let mut heroes = HashMap::with_capacity(count);
    for _ in 0..count {
        let id_end = offset.checked_add(2)?;
        if id_end > HERO_ATLAS_BYTES.len() {
            return None;
        }
        let hid = u16::from_le_bytes([HERO_ATLAS_BYTES[offset], HERO_ATLAS_BYTES[offset + 1]]);
        offset = id_end;
        let frame_end = offset.checked_add(hero_len)?;
        if frame_end > HERO_ATLAS_BYTES.len() {
            return None;
        }
        heroes.insert(HeroId(hid), HERO_ATLAS_BYTES[offset..frame_end].to_vec());
        offset = frame_end;
    }

    Some(DecodedHeroAtlas {
        hero_width,
        hero_height,
        gem_width,
        gem_height,
        gem_rgb,
        heroes,
    })
}

struct HeroTextureSet {
    color: TextureHandle,
    toned: TextureHandle,
    banned: TextureHandle,
}

#[derive(Default)]
struct GuiTextureCache {
    initialized: bool,
    gem: Option<TextureHandle>,
    heroes: HashMap<HeroId, HeroTextureSet>,
}

impl GuiTextureCache {
    fn ensure_loaded(&mut self, ctx: &egui::Context) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        let Some(atlas) = parse_hero_atlas() else {
            return;
        };

        let gem_img = egui::ColorImage::from_rgb([atlas.gem_width, atlas.gem_height], &atlas.gem_rgb);
        self.gem = Some(ctx.load_texture("truesight_gem", gem_img, TextureOptions::LINEAR));

        let size = [atlas.hero_width, atlas.hero_height];
        for (hid, rgb) in atlas.heroes {
            let color_img = egui::ColorImage::from_rgb(size, &rgb);
            let toned_rgb = transform_rgb(&rgb, 0.60, 0.80);
            let toned_img = egui::ColorImage::from_rgb(size, &toned_rgb);
            let banned_rgb = transform_rgb(&rgb, 0.90, 0.55);
            let banned_img = egui::ColorImage::from_rgb(size, &banned_rgb);

            let color = ctx.load_texture(format!("h_col_{}", hid.0), color_img, TextureOptions::LINEAR);
            let toned = ctx.load_texture(format!("h_ton_{}", hid.0), toned_img, TextureOptions::LINEAR);
            let banned = ctx.load_texture(format!("h_ban_{}", hid.0), banned_img, TextureOptions::LINEAR);
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

fn uv_full() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

/// Center-crops a 16:9 (64x36) texture into a target rect aspect ratio without stretching.
fn uv_aspect_crop(target_size: Vec2) -> Rect {
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

fn uv_square_crop() -> Rect {
    uv_aspect_crop(vec2(1.0, 1.0))
}

fn hud_card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) -> Rect {
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

fn hud_card_fill(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) -> Rect {
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

fn paint_hud_corner_ticks(painter: &egui::Painter, rect: Rect, color: Color32) {
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

fn paint_portrait_16_9(
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

fn paint_tinted_cell(
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
        (
            Color32::from_rgba_unmultiplied(46, 230, 122, alpha),
            P_GEM,
        )
    } else if delta_pct <= -EPS_PCT {
        (
            Color32::from_rgba_unmultiplied(236, 78, 66, alpha),
            P_DIRE,
        )
    } else {
        (Color32::TRANSPARENT, P_LINE)
    };

    if tint_fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, 0.0, tint_fill);
    }

    // Bottom dark legibility shade
    let shade_top = rect.min.y + rect.height() * 0.52;
    let shade_rect = Rect::from_min_max(pos2(rect.min.x, shade_top), rect.max);
    painter.rect_filled(
        shade_rect,
        0.0,
        Color32::from_rgba_unmultiplied(10, 9, 24, 210),
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

fn paint_tinted_square_cell(
    ui: &mut egui::Ui,
    size: f32,
    tex: Option<&TextureHandle>,
    delta_pct: f32,
    font_size: f32,
    ring_width: f32,
) -> egui::Response {
    paint_tinted_cell(ui, vec2(size, size), tex, delta_pct, font_size, ring_width)
}

fn paint_avg_square_cell(ui: &mut egui::Ui, size: Vec2, avg_pct: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, P_INDIGO_2);
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, P_LINE), StrokeKind::Inside);

    if let Some(pct) = avg_pct {
        let col = tone_for_user(pct, 1.0).color();
        painter.text(
            pos2(rect.center().x, rect.center().y - 5.0),
            Align2::CENTER_CENTER,
            fmt_pct(pct),
            FontId::proportional(11.0),
            col,
        );
        // Centered zero-tick bar
        let bar_w = rect.width() * 0.72;
        let bar_h = 4.0;
        let bar_rect = Rect::from_center_size(
            pos2(rect.center().x, rect.center().y + 8.0),
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
            FontId::proportional(11.0),
            P_MUTED,
        );
    }
}

fn paint_total_corner_cell(ui: &mut egui::Ui, size: Vec2, edge_pct: f32, label: &str) {
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
    painter.rect_filled(rect, 0.0, bg);
    painter.rect_stroke(rect, 0.0, Stroke::new(1.5_f32, col), StrokeKind::Inside);
    painter.text(
        pos2(rect.center().x, rect.center().y - 5.0),
        Align2::CENTER_CENTER,
        fmt_pct(edge_pct),
        FontId::proportional(11.5),
        col,
    );
    painter.text(
        pos2(rect.center().x, rect.center().y + 8.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(8.5),
        P_MUTED,
    );
}

fn paint_matrix_scale_bar(ui: &mut egui::Ui, left_label: &str, right_label: &str) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(
        vec2(w, 14.0),
        Layout::left_to_right(Align::Center),
        |ui| {
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
        },
    );
}

fn paint_mini_badge(ui: &mut egui::Ui, text: &str, text_color: Color32, border_color: Color32) {
    egui::Frame::NONE
        .fill(P_INDIGO_1)
        .stroke(Stroke::new(1.0_f32, border_color))
        .inner_margin(Margin::symmetric(4, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(8.5).strong().color(text_color));
        });
}

fn paint_trend_badge(ui: &mut egui::Ui, trend_pct: f32) {
    if trend_pct.abs() < EPS_PCT {
        return;
    }
    let up = trend_pct > 0.0;
    let col = if up { P_GEM } else { P_DIRE };
    let border = if up { P_GEM_DEEP } else { P_DIRE_DEEP };
    egui::Frame::NONE
        .fill(P_INDIGO_1)
        .stroke(Stroke::new(1.0_f32, border))
        .inner_margin(Margin::symmetric(4, 1))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let (tri_rect, _) = ui.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
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
                ui.painter().add(Shape::convex_polygon(pts, col, Stroke::NONE));
                ui.label(
                    RichText::new(format!("{:.1}", trend_pct.abs()))
                        .size(8.5)
                        .strong()
                        .color(col),
                );
            });
        });
}

// =============================================================================
// Main Application State & eframe::App Implementation
// =============================================================================

pub struct TrueSightGuiApp {
    session: AppSession,
    gsi_listener: Option<BoundedGsiListener>,
    textures: GuiTextureCache,
    style_configured: bool,
    matrix_tab: MatrixTab,
    picker_target: Option<PickerTarget>,
    picker_search: String,
    focus_picker_input: bool,
    suggest_filter: String,
    status_banner: String,
}

impl TrueSightGuiApp {
    pub fn new(session: AppSession, gsi_listener: Option<BoundedGsiListener>) -> Self {
        Self {
            session,
            gsi_listener,
            textures: GuiTextureCache::default(),
            style_configured: false,
            matrix_tab: MatrixTab::Counter,
            picker_target: None,
            picker_search: String::new(),
            focus_picker_input: false,
            suggest_filter: String::new(),
            status_banner:
                "Click any Ally (P1..P5) or Enemy (P1..P5) slot card, type hero name, and press Enter."
                    .into(),
        }
    }

    fn configure_visuals(&mut self, ctx: &egui::Context) {
        if self.style_configured {
            return;
        }
        self.style_configured = true;
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = P_VOID;
        visuals.window_fill = P_INDIGO_1;
        visuals.extreme_bg_color = P_INDIGO_2;
        visuals.faint_bg_color = P_INDIGO_2;
        visuals.override_text_color = Some(P_SILVER);
        visuals.widgets.noninteractive.bg_fill = P_INDIGO_2;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, P_LINE);
        visuals.widgets.noninteractive.corner_radius = CornerRadius::same(2);
        visuals.widgets.inactive.bg_fill = P_INDIGO_2;
        visuals.widgets.inactive.weak_bg_fill = P_INDIGO_2;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, P_LINE);
        visuals.widgets.inactive.corner_radius = CornerRadius::same(2);
        visuals.widgets.hovered.bg_fill = P_INDIGO_3;
        visuals.widgets.hovered.weak_bg_fill = P_INDIGO_3;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, P_GEM);
        visuals.widgets.hovered.corner_radius = CornerRadius::same(2);
        visuals.widgets.active.bg_fill = P_GEM_DEEP;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, P_GEM);
        visuals.widgets.active.corner_radius = CornerRadius::same(2);
        visuals.selection.bg_fill = P_GEM;
        visuals.selection.stroke = Stroke::new(1.0_f32, P_VOID);
        ctx.set_visuals(visuals);
    }

    fn render_top_bar(&mut self, ui: &mut egui::Ui) -> Rect {
        hud_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if let Some(ref gem_tex) = self.textures.gem {
                    let (rect, _) = ui.allocate_exact_size(vec2(28.0, 21.0), Sense::hover());
                    ui.painter().image(gem_tex.id(), rect, uv_full(), Color32::WHITE);
                    ui.painter().rect_stroke(
                        rect,
                        0.0,
                        Stroke::new(1.0_f32, P_GEM_DEEP),
                        StrokeKind::Inside,
                    );
                }
                ui.label(
                    RichText::new("TRUE")
                        .size(14.0)
                        .strong()
                        .color(P_GLINT),
                );
                ui.add_space(-6.0);
                ui.label(
                    RichText::new("SIGHT")
                        .size(14.0)
                        .strong()
                        .color(P_GEM),
                );
                ui.label(
                    RichText::new("· DRAFT ORACLE")
                        .size(13.0)
                        .strong()
                        .color(P_GLINT),
                );

                // GSI pill
                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_LINE))
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot_r, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
                            ui.painter().rect_filled(dot_r, 0.0, P_GEM);
                            ui.label(
                                RichText::new(format!("GSI · {}", self.session.last_gsi_state))
                                    .size(10.5)
                                    .color(P_MUTED),
                            );
                        });
                    });

                // Interactive Faction Toggle Pill (YOU: RADIANT <-> / YOU: DIRE <->)
                let fac_col = match self.session.draft.ally_faction {
                    Faction::Radiant => P_GEM,
                    Faction::Dire => P_DIRE,
                };
                let fac_btn = egui::Button::new(
                    RichText::new(format!("YOU: {} <->", self.session.draft.ally_faction.label()))
                        .size(10.5)
                        .strong()
                        .color(fac_col),
                )
                .fill(P_INDIGO_2)
                .stroke(Stroke::new(1.0_f32, P_GEM_DEEP));
                if ui.add(fac_btn).clicked() {
                    self.session.draft.ally_faction = self.session.draft.ally_faction.opposite();
                }

                // OpenDota DB status pill
                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_LINE))
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&self.session.db_status)
                                .size(10.5)
                                .color(P_MUTED),
                        );
                    });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("RESET").size(10.5).strong().color(P_GLINT),
                            )
                            .fill(P_INDIGO_2),
                        )
                        .clicked()
                    {
                        self.session.draft.clear();
                        self.picker_target = None;
                        self.status_banner = "Draft cleared.".into();
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("SYNC DB").size(10.5).strong().color(P_GLINT),
                            )
                            .fill(P_INDIGO_2),
                        )
                        .clicked()
                    {
                        match self.session.sync_db(true) {
                            Ok(msg) => self.status_banner = msg,
                            Err(e) => {
                                self.status_banner =
                                    format!("Sync notice: {e} (using embedded snapshot)")
                            }
                        }
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("DEMO 5V5").size(10.5).strong().color(P_GEM),
                            )
                            .fill(P_INDIGO_2)
                            .stroke(Stroke::new(1.0_f32, P_GEM_DEEP)),
                        )
                        .clicked()
                    {
                        self.session.draft.clear();
                        let _ = crate::execute_command(&mut self.session, "ally 1 faceless void");
                        let _ = crate::execute_command(&mut self.session, "ally 3 magnus");
                        let _ = crate::execute_command(&mut self.session, "ally 4 shadow demon");
                        let _ = crate::execute_command(&mut self.session, "ally 5 crystal maiden");
                        let _ = crate::execute_command(&mut self.session, "enemy 1 medusa");
                        let _ = crate::execute_command(&mut self.session, "enemy 2 storm spirit");
                        let _ = crate::execute_command(&mut self.session, "enemy 3 axe");
                        let _ = crate::execute_command(&mut self.session, "enemy 4 tusk");
                        let _ = crate::execute_command(&mut self.session, "enemy 5 witch doctor");
                        self.session.draft.target_role = Role::Mid;
                        self.picker_target = Some(PickerTarget::Slot(Side::Allies, 1));
                        self.status_banner =
                            "Loaded Demo 5v5 (Pos 2 Mid picking · 9/10 picked).".into();
                    }

                    // 8-Medal Bracket Segmented Control
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        for bracket in Bracket::ALL.into_iter().rev() {
                            let is_on = self.session.draft.bracket == bracket;
                            let btn = egui::Button::new(
                                RichText::new(bracket.label())
                                    .size(10.5)
                                    .strong()
                                    .color(if is_on { P_VOID } else { P_MUTED }),
                            )
                            .fill(if is_on { P_GEM } else { P_INDIGO_2 })
                            .stroke(Stroke::new(1.0_f32, if is_on { P_GEM } else { P_LINE }));
                            if ui.add(btn).clicked() {
                                self.session.set_bracket(bracket);
                            }
                        }
                    });
                });
            });
        })
    }

    fn render_team_card(
        &mut self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
        side: Side,
    ) -> Rect {
        let is_ally = side == Side::Allies;
        let accent = if is_ally { P_GEM } else { P_DIRE };
        let title = if is_ally { "ALLIES" } else { "ENEMIES" };
        let faction = if is_ally {
            self.session.draft.ally_faction
        } else {
            self.session.draft.ally_faction.opposite()
        };
        let picks = match side {
            Side::Allies => &self.session.draft.allies,
            Side::Enemies => &self.session.draft.enemies,
        };
        let picked_count = picks.iter().flatten().count();
        let sign = if is_ally { 1.0_f32 } else { -1.0_f32 };
        let net_caption = if is_ally { "NET" } else { "THREAT" };
        let role_short = [
            "P1 · CARRY",
            "P2 · MID",
            "P3 · OFF",
            "P4 · SOFT",
            "P5 · HARD",
        ];

        hud_card_fill(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            // Card Header
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(11.0).strong().color(accent));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{picked_count} / {TEAM_SIZE}"))
                            .size(10.5)
                            .color(P_MUTED),
                    );
                    let fac_col = match faction {
                        Faction::Radiant => P_GEM,
                        Faction::Dire => P_DIRE,
                    };
                    paint_mini_badge(ui, faction.label(), fac_col, fac_col);
                });
            });
            ui.add_space(6.0);

            for slot in 0..TEAM_SIZE {
                let is_picking = self.picker_target == Some(PickerTarget::Slot(side, slot))
                    || (is_ally
                        && self.picker_target.is_none()
                        && self.session.draft.allies[slot].is_none()
                        && self.session.draft.target_role.index() == slot);
                let current_pick = match side {
                    Side::Allies => self.session.draft.allies[slot],
                    Side::Enemies => self.session.draft.enemies[slot],
                };
                let breakdown = compute_slot_breakdown(&self.session, analysis, side, slot);

                let bg_fill = if is_picking {
                    Color32::from_rgba_unmultiplied(46, 230, 122, 22)
                } else {
                    P_INDIGO_2
                };
                let border_col = if is_picking { P_GEM } else { P_INDIGO_3 };

                let row_frame = egui::Frame::NONE
                    .fill(bg_fill)
                    .stroke(Stroke::new(1.0_f32, border_col))
                    .inner_margin(Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.set_min_size(vec2(ui.available_width(), 44.0));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;

                            // 1. Portrait (58x33) or dashed '?'
                            if let Some(pick) = current_pick {
                                let tex = self
                                    .textures
                                    .heroes
                                    .get(&pick.hero_id)
                                    .map(|s| &s.color);
                                paint_portrait_16_9(ui, tex, vec2(58.0, 33.0), P_LINE, 1.0);
                            } else {
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(58.0, 33.0), Sense::hover());
                                ui.painter().rect_stroke(
                                    r,
                                    0.0,
                                    Stroke::new(1.0_f32, if is_picking { P_GEM } else { P_LINE }),
                                    StrokeKind::Inside,
                                );
                                ui.painter().text(
                                    r.center(),
                                    Align2::CENTER_CENTER,
                                    "?",
                                    FontId::proportional(15.0),
                                    if is_picking { P_GEM } else { P_MUTED },
                                );
                            }

                            // 2. Middle 3-line telemetry column
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                if let Some(pick) = current_pick {
                                    let profile = self.session.matrix.hero(pick.hero_id);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 3.0;
                                        ui.label(
                                            RichText::new(role_short[slot])
                                                .size(9.0)
                                                .color(P_MUTED),
                                        );
                                        if let Some(h) = profile {
                                            let atk_lbl = match h.attack_type {
                                                AttackType::Melee => "M",
                                                AttackType::Ranged => "R",
                                            };
                                            paint_mini_badge(ui, atk_lbl, P_SILVER, P_LINE);
                                            if h.has_disable {
                                                paint_mini_badge(ui, "DIS", P_CYAN, P_CYAN);
                                            }
                                            if h.has_initiation {
                                                paint_mini_badge(ui, "INIT", P_GEM, P_GEM_DEEP);
                                            }
                                            paint_trend_badge(ui, h.trend_delta * 100.0);
                                        }
                                    });

                                    let hero_name = profile
                                        .map(|h| h.display_name.as_str())
                                        .unwrap_or("Unknown");
                                    ui.label(
                                        RichText::new(hero_name)
                                            .size(12.5)
                                            .strong()
                                            .color(P_GLINT),
                                    );

                                    if let Some(b) = breakdown {
                                        let m_pct = b.meta * 100.0;
                                        let c_pct = b.cnt * 100.0;
                                        let s_pct = b.syn * 100.0;
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 3.0;
                                            ui.label(
                                                RichText::new("Meta").size(9.5).color(P_MUTED),
                                            );
                                            ui.label(
                                                RichText::new(fmt_val(m_pct))
                                                    .size(9.5)
                                                    .strong()
                                                    .color(tone_for_user(m_pct, sign).color()),
                                            );
                                            ui.label(RichText::new("·").size(9.5).color(P_MUTED));
                                            ui.label(RichText::new("Cnt").size(9.5).color(P_MUTED));
                                            ui.label(
                                                RichText::new(fmt_val(c_pct))
                                                    .size(9.5)
                                                    .strong()
                                                    .color(tone_for_user(c_pct, sign).color()),
                                            );
                                            ui.label(RichText::new("·").size(9.5).color(P_MUTED));
                                            ui.label(RichText::new("Syn").size(9.5).color(P_MUTED));
                                            ui.label(
                                                RichText::new(fmt_val(s_pct))
                                                    .size(9.5)
                                                    .strong()
                                                    .color(tone_for_user(s_pct, sign).color()),
                                            );
                                        });
                                    }
                                } else {
                                    ui.label(
                                        RichText::new(format!(
                                            "{} · {}",
                                            role_short[slot],
                                            if is_picking { "PICKING" } else { "EMPTY" }
                                        ))
                                        .size(9.0)
                                        .color(if is_picking { P_GEM } else { P_MUTED }),
                                    );
                                    ui.label(
                                        RichText::new("Click to pick hero...")
                                            .size(12.0)
                                            .color(P_MUTED),
                                    );
                                    ui.label(
                                        RichText::new(format!("Synced with Pos {}", slot + 1))
                                            .size(9.5)
                                            .color(P_MUTED),
                                    );
                                }
                            });

                            // 3. Right Net / Threat resultant + clear button
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if current_pick.is_some()
                                    && ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("×").size(11.0).color(P_MUTED),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                {
                                    match side {
                                        Side::Allies => {
                                            let _ = self.session.draft.set_ally(slot, None);
                                        }
                                        Side::Enemies => {
                                            let _ = self.session.draft.set_enemy(slot, None);
                                        }
                                    }
                                }
                                ui.vertical(|ui| {
                                    ui.with_layout(Layout::top_down(Align::Max), |ui| {
                                        if let Some(b) = breakdown {
                                            let net_pct = b.net * 100.0;
                                            ui.label(
                                                RichText::new(fmt_pct(net_pct))
                                                    .size(13.5)
                                                    .strong()
                                                    .color(tone_for_user(net_pct, sign).color()),
                                            );
                                            ui.label(
                                                RichText::new(net_caption)
                                                    .size(8.5)
                                                    .color(P_MUTED),
                                            );
                                        } else {
                                            ui.label(
                                                RichText::new("[Enter]")
                                                    .size(11.0)
                                                    .strong()
                                                    .color(if is_picking { P_GEM } else { P_MUTED }),
                                            );
                                        }
                                    });
                                });
                            });
                        });
                    });

                // Left 3px side accent bar
                let r = row_frame.response.rect;
                ui.painter().rect_filled(
                    Rect::from_min_max(r.min, pos2(r.min.x + 3.0, r.max.y)),
                    0.0,
                    accent,
                );

                // Click anywhere on the slot card to activate picker for that slot
                let click_resp = ui.interact(
                    r,
                    Id::new(("slot_card_click", is_ally, slot)),
                    Sense::click(),
                );
                if click_resp.clicked() {
                    if is_ally {
                        self.session.draft.target_role =
                            Role::ALL.get(slot).copied().unwrap_or(Role::Carry);
                    }
                    let target = PickerTarget::Slot(side, slot);
                    if self.picker_target == Some(target) {
                        self.picker_target = None;
                    } else {
                        self.picker_target = Some(target);
                        self.picker_search.clear();
                        self.focus_picker_input = true;
                    }
                }
            }
        })
    }

    fn render_oracle_card(&mut self, ui: &mut egui::Ui, analysis: &DraftAnalysis5v5) -> Rect {
        let ally_pct = analysis.ally_win_probability * 100.0;
        let enemy_pct = 100.0 - ally_pct;
        let lead = ally_pct - 50.0;
        let picked_total = self.session.draft.allies.iter().flatten().count()
            + self.session.draft.enemies.iter().flatten().count();

        hud_card_fill(ui, |ui| {
            // 1. Score Row
            ui.columns(3, |cols| {
                cols[0].label(
                    RichText::new("ALLIES WIN")
                        .size(10.0)
                        .color(P_MUTED),
                );
                cols[0].label(
                    RichText::new(format!("{ally_pct:4.1}%"))
                        .size(40.0)
                        .strong()
                        .color(P_GEM),
                );

                cols[1].vertical_centered(|ui| {
                    ui.add_space(8.0);
                    let tag_col = tone_for_user(lead, 1.0).color();
                    let tag_word = if lead >= EPS_PCT {
                        "FAVORED"
                    } else if lead <= -EPS_PCT {
                        "UNDERDOG"
                    } else {
                        "EVEN"
                    };
                    egui::Frame::NONE
                        .stroke(Stroke::new(1.0_f32, tag_col))
                        .inner_margin(Margin::symmetric(8, 3))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("{tag_word} {}", fmt_pct(lead)))
                                    .size(10.5)
                                    .strong()
                                    .color(tag_col),
                            );
                        });
                    ui.add_space(3.0);
                    ui.label(
                        RichText::new(format!(
                            "{} · {picked_total} / 10 picked",
                            self.session.draft.bracket.label()
                        ))
                        .size(10.0)
                        .color(P_MUTED),
                    );
                });

                cols[2].with_layout(Layout::top_down(Align::Max), |ui| {
                    ui.label(
                        RichText::new("ENEMIES WIN")
                            .size(10.0)
                            .color(P_MUTED),
                    );
                    ui.label(
                        RichText::new(format!("{enemy_pct:4.1}%"))
                            .size(40.0)
                            .strong()
                            .color(P_DIRE),
                    );
                });
            });

            ui.add_space(6.0);

            // 2. Tug-of-war bar with 50% center tick
            let bar_w = ui.available_width();
            let (tug_rect, _) = ui.allocate_exact_size(vec2(bar_w, 10.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(tug_rect, 0.0, P_DIRE);
            let fill_w = tug_rect.width() * analysis.ally_win_probability.clamp(0.0, 1.0);
            painter.rect_filled(
                Rect::from_min_max(tug_rect.min, pos2(tug_rect.min.x + fill_w, tug_rect.max.y)),
                0.0,
                P_GEM,
            );
            let mid_x = tug_rect.center().x;
            painter.line_segment(
                [
                    pos2(mid_x, tug_rect.min.y - 3.0),
                    pos2(mid_x, tug_rect.max.y + 3.0),
                ],
                Stroke::new(2.0_f32, P_GLINT),
            );

            ui.add_space(8.0);

            // 3. Four Edge Cards (Meta, Counter, Synergy, Composition)
            let pair_cnt = analysis
                .counter_grid
                .iter()
                .flatten()
                .flatten()
                .count();
            let ally_comp =
                compute_comp_diagnostic(&self.session.draft.allies, &self.session.matrix).score
                    * 100.0;
            let enemy_comp =
                compute_comp_diagnostic(&self.session.draft.enemies, &self.session.matrix).score
                    * 100.0;

            let edges = [
                (
                    "Meta edge",
                    analysis.total_meta_edge * 100.0,
                    format!("Role baseline vs 50%"),
                ),
                (
                    "Counter edge",
                    analysis.total_counter_edge * 100.0,
                    format!("{pair_cnt} / 25 pairs"),
                ),
                (
                    "Synergy edge",
                    analysis.total_synergy_edge * 100.0,
                    format!(
                        "A {} · E {}",
                        fmt_val(analysis.ally_synergy_norm * 100.0),
                        fmt_val(analysis.enemy_synergy_norm * 100.0)
                    ),
                ),
                (
                    "Composition",
                    analysis.total_composition_edge * 100.0,
                    format!("A {} · E {}", fmt_val(ally_comp), fmt_val(enemy_comp)),
                ),
            ];

            ui.columns(4, |cols| {
                for (idx, (label, val_pct, sub)) in edges.iter().enumerate() {
                    let top_col = tone_for_user(*val_pct, 1.0).color();
                    let fr = egui::Frame::NONE
                        .fill(P_INDIGO_2)
                        .inner_margin(Margin::symmetric(8, 7))
                        .show(&mut cols[idx], |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.label(RichText::new(*label).size(10.0).color(P_MUTED));
                            ui.label(
                                RichText::new(fmt_pct(*val_pct))
                                    .size(18.0)
                                    .strong()
                                    .color(top_col),
                            );
                            ui.label(RichText::new(sub).size(9.5).color(P_MUTED));
                        });
                    let r = fr.response.rect;
                    cols[idx].painter().rect_filled(
                        Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + 2.0)),
                        0.0,
                        top_col,
                    );
                }
            });

            ui.add_space(8.0);

            // 4. Insight Strip (Strongest Pair + Hardest Counter)
            self.render_insight_strip(ui, analysis);

            ui.add_space(8.0);

            // 5. Draft Order (10 slots) & Bans (16 slots) side-by-side
            ui.columns(2, |cols| {
                // Draft Order Box
                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_INDIGO_3))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(&mut cols[0], |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("DRAFT ORDER").size(9.5).color(P_MUTED));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "TARGET · {}",
                                        self.session.draft.target_role.label()
                                    ))
                                    .size(9.5)
                                    .color(P_MUTED),
                                );
                            });
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            let w = ((ui.available_width() - 9.0 * 3.0) / 10.0).max(24.0);
                            let h = (w * 9.0 / 16.0).round();
                            for i in 0..TEAM_SIZE {
                                if let Some(p) = self.session.draft.allies[i] {
                                    let tex = self
                                        .textures
                                        .heroes
                                        .get(&p.hero_id)
                                        .map(|s| &s.color);
                                    paint_portrait_16_9(ui, tex, vec2(w, h), P_GEM, 1.5);
                                } else {
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(w, h), Sense::hover());
                                    ui.painter().rect_stroke(
                                        r,
                                        0.0,
                                        Stroke::new(1.0_f32, P_GEM),
                                        StrokeKind::Inside,
                                    );
                                    ui.painter().text(
                                        r.center(),
                                        Align2::CENTER_CENTER,
                                        format!("P{}", i + 1),
                                        FontId::proportional(8.5),
                                        P_GEM,
                                    );
                                }
                                if let Some(p) = self.session.draft.enemies[i] {
                                    let tex = self
                                        .textures
                                        .heroes
                                        .get(&p.hero_id)
                                        .map(|s| &s.color);
                                    paint_portrait_16_9(ui, tex, vec2(w, h), P_DIRE, 1.5);
                                } else {
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(w, h), Sense::hover());
                                    ui.painter().rect_stroke(
                                        r,
                                        0.0,
                                        Stroke::new(1.0_f32, P_LINE),
                                        StrokeKind::Inside,
                                    );
                                    ui.painter().text(
                                        r.center(),
                                        Align2::CENTER_CENTER,
                                        format!("E{}", i + 1),
                                        FontId::proportional(8.5),
                                        P_MUTED,
                                    );
                                }
                            }
                        });
                    });

                // Bans Box
                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_INDIGO_3))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(&mut cols[1], |ui| {
                        let banned_ids: Vec<HeroId> =
                            self.session.draft.bans.iter().copied().flatten().collect();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("BANS").size(9.5).color(P_MUTED));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let ban_active = self.picker_target == Some(PickerTarget::Ban);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(if ban_active {
                                                "CLOSE BAN"
                                            } else {
                                                "+ BAN"
                                            })
                                            .size(9.0)
                                            .strong()
                                            .color(if ban_active { P_VOID } else { P_DIRE }),
                                        )
                                        .fill(if ban_active { P_DIRE } else { P_INDIGO_1 })
                                        .stroke(Stroke::new(1.0_f32, P_DIRE_DEEP)),
                                    )
                                    .clicked()
                                {
                                    if ban_active {
                                        self.picker_target = None;
                                    } else {
                                        self.picker_target = Some(PickerTarget::Ban);
                                        self.picker_search.clear();
                                        self.focus_picker_input = true;
                                    }
                                }
                                ui.label(
                                    RichText::new(format!("{} / 16", banned_ids.len()))
                                        .size(9.5)
                                        .color(P_MUTED),
                                );
                            });
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            let slots_shown = 8usize;
                            let w = ((ui.available_width() - 7.0 * 3.0) / 8.0).max(24.0);
                            let h = (w * 9.0 / 16.0).round();
                            for idx in 0..slots_shown {
                                if let Some(&hid) = banned_ids.get(idx) {
                                    let (r, resp) =
                                        ui.allocate_exact_size(vec2(w, h), Sense::click());
                                    if let Some(t) =
                                        self.textures.heroes.get(&hid).map(|s| &s.banned)
                                    {
                                        ui.painter().image(
                                            t.id(),
                                            r,
                                            uv_full(),
                                            Color32::WHITE,
                                        );
                                    }
                                    ui.painter().rect_stroke(
                                        r,
                                        0.0,
                                        Stroke::new(1.0_f32, P_DIRE_DEEP),
                                        StrokeKind::Inside,
                                    );
                                    ui.painter().line_segment(
                                        [r.min, r.max],
                                        Stroke::new(1.5_f32, P_DIRE),
                                    );
                                    if resp.clicked() {
                                        let _ = self.session.draft.toggle_ban(hid);
                                    }
                                } else {
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(w, h), Sense::hover());
                                    ui.painter().rect_stroke(
                                        r,
                                        0.0,
                                        Stroke::new(1.0_f32, P_LINE),
                                        StrokeKind::Inside,
                                    );
                                }
                            }
                        });
                    });
            });

            // 6. Active Slot / Ban Search Bar (inline inside Oracle when active)
            if let Some(target) = self.picker_target {
                ui.add_space(6.0);
                self.render_inline_picker(ui, target);
            }
        })
    }

    fn render_insight_strip(&self, ui: &mut egui::Ui, analysis: &DraftAnalysis5v5) {
        let mut best_syn: Option<(HeroId, HeroId, f32)> = None;
        for i in 0..TEAM_SIZE {
            let Some(a) = self.session.draft.allies[i] else {
                continue;
            };
            for k in (i + 1)..TEAM_SIZE {
                let Some(b) = self.session.draft.allies[k] else {
                    continue;
                };
                if let Some(s) = analysis.ally_synergy_grid[i][k] {
                    if best_syn.map_or(true, |cur| s > cur.2) {
                        best_syn = Some((a.hero_id, b.hero_id, s));
                    }
                }
            }
        }

        let mut worst_cnt: Option<(HeroId, HeroId, usize, f32)> = None;
        for i in 0..TEAM_SIZE {
            let Some(a) = self.session.draft.allies[i] else {
                continue;
            };
            for j in 0..TEAM_SIZE {
                let Some(e) = self.session.draft.enemies[j] else {
                    continue;
                };
                if let Some(c) = analysis.counter_grid[i][j] {
                    if worst_cnt.map_or(true, |cur| c < cur.3) {
                        worst_cnt = Some((a.hero_id, e.hero_id, i, c));
                    }
                }
            }
        }

        let fr = egui::Frame::NONE
            .fill(P_INDIGO_2)
            .inner_margin(Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if let Some((ha, hb, syn)) = best_syn {
                        let ta = self.textures.heroes.get(&ha).map(|s| &s.color);
                        let tb = self.textures.heroes.get(&hb).map(|s| &s.color);
                        paint_portrait_16_9(ui, ta, vec2(40.0, 22.0), P_LINE, 1.0);
                        paint_portrait_16_9(ui, tb, vec2(40.0, 22.0), P_LINE, 1.0);
                        let na = self
                            .session
                            .matrix
                            .hero(ha)
                            .map(|h| h.display_name.as_str())
                            .unwrap_or("Ally");
                        let nb = self
                            .session
                            .matrix
                            .hero(hb)
                            .map(|h| h.display_name.as_str())
                            .unwrap_or("Ally");
                        ui.label(
                            RichText::new(format!("{na} + {nb}"))
                                .size(11.5)
                                .strong()
                                .color(P_GLINT),
                        );
                        ui.label(
                            RichText::new(format!(
                                "is your best pair ({} synergy).",
                                fmt_pct(syn * 100.0)
                            ))
                            .size(11.5)
                            .color(P_MUTED),
                        );
                    } else {
                        ui.label(
                            RichText::new(&self.status_banner)
                                .size(11.5)
                                .color(P_MUTED),
                        );
                    }

                    if let Some((_ally_id, enemy_id, slot_idx, cnt)) = worst_cnt {
                        if cnt * 100.0 <= -EPS_PCT {
                            let ne = self
                                .session
                                .matrix
                                .hero(enemy_id)
                                .map(|h| h.display_name.as_str())
                                .unwrap_or("Enemy");
                            ui.label(
                                RichText::new(ne).size(11.5).strong().color(P_GLINT),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "counters your Pos {} ({}).",
                                    slot_idx + 1,
                                    fmt_pct(cnt * 100.0)
                                ))
                                .size(11.5)
                                .color(P_MUTED),
                            );
                        }
                    }
                });
            });
        let r = fr.response.rect;
        ui.painter().rect_filled(
            Rect::from_min_max(r.min, pos2(r.min.x + 3.0, r.max.y)),
            0.0,
            P_GEM,
        );
    }

    fn render_inline_picker(&mut self, ui: &mut egui::Ui, target: PickerTarget) {
        let label = match target {
            PickerTarget::Slot(Side::Allies, i) => format!("PICKING ALLY P{}", i + 1),
            PickerTarget::Slot(Side::Enemies, j) => format!("PICKING ENEMY P{}", j + 1),
            PickerTarget::Ban => "TOGGLE BAN HERO".into(),
        };
        let matches = match target {
            PickerTarget::Slot(side, slot) => {
                self.session
                    .filter_heroes_for_slot(side, slot, &self.picker_search)
            }
            PickerTarget::Ban => {
                self.session
                    .filter_heroes_for_slot(Side::Allies, 0, &self.picker_search)
            }
        };

        egui::Frame::NONE
            .fill(P_INDIGO_2)
            .stroke(Stroke::new(1.0_f32, P_GEM))
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&label).size(10.5).strong().color(P_GEM));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.picker_search)
                            .desired_width(190.0)
                            .hint_text("Type hero name & press Enter..."),
                    );
                    if self.focus_picker_input {
                        resp.request_focus();
                        self.focus_picker_input = false;
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if let Some(&(hid, ref hname)) = matches.first() {
                            self.apply_picker_choice(target, hid, hname);
                            return;
                        }
                    }
                    if ui.small_button("Close ×").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        self.picker_target = None;
                        return;
                    }
                    ui.horizontal_wrapped(|ui| {
                        for (idx, &(hid, ref hname)) in matches.iter().take(6).enumerate() {
                            let txt = if idx == 0 && !self.picker_search.trim().is_empty() {
                                RichText::new(format!("[Enter] {hname}"))
                                    .size(10.5)
                                    .strong()
                                    .color(P_GEM)
                            } else {
                                RichText::new(hname.as_str()).size(10.5).color(P_GLINT)
                            };
                            if ui.small_button(txt).clicked() {
                                self.apply_picker_choice(target, hid, hname);
                                break;
                            }
                        }
                    });
                });
            });
    }

    fn apply_picker_choice(&mut self, target: PickerTarget, hid: HeroId, hname: &str) {
        match target {
            PickerTarget::Slot(side, slot) => {
                let pick = Some(DraftPick {
                    hero_id: hid,
                    role: Role::ALL.get(slot).copied(),
                });
                match side {
                    Side::Allies => {
                        let _ = self.session.draft.set_ally(slot, pick);
                        self.status_banner = format!("Picked {hname} into Ally P{}.", slot + 1);
                    }
                    Side::Enemies => {
                        let _ = self.session.draft.set_enemy(slot, pick);
                        self.status_banner = format!("Picked {hname} into Enemy P{}.", slot + 1);
                    }
                }
                self.picker_target = None;
            }
            PickerTarget::Ban => {
                let _ = self.session.draft.toggle_ban(hid);
                self.status_banner = format!("Toggled ban for {hname}.");
                self.picker_search.clear();
            }
        }
    }

    fn render_suggested_picks_card(&mut self, ui: &mut egui::Ui) -> Rect {
        let target_role = self.session.draft.target_role;
        let target_slot_idx = target_role.index();

        hud_card_fill(ui, |ui| {
            // Header
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "SUGGESTED PICKS · {}",
                        target_role.label().to_ascii_uppercase()
                    ))
                    .size(11.0)
                    .strong()
                    .color(P_GLINT),
                );

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if !self.suggest_filter.is_empty() && ui.small_button("×").clicked() {
                        self.suggest_filter.clear();
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.suggest_filter)
                            .desired_width(135.0)
                            .hint_text("Filter hero..."),
                    );

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        for (idx, role) in Role::ALL.into_iter().enumerate().rev() {
                            let is_on = self.session.draft.target_role == role;
                            let btn = egui::Button::new(
                                RichText::new(format!("POS {}", idx + 1))
                                    .size(10.0)
                                    .strong()
                                    .color(if is_on { P_VOID } else { P_MUTED }),
                            )
                            .fill(if is_on { P_GEM } else { P_INDIGO_2 })
                            .stroke(Stroke::new(1.0_f32, if is_on { P_GEM } else { P_LINE }));
                            if ui.add(btn).clicked() {
                                self.session.draft.target_role = role;
                            }
                        }
                    });
                });
            });

            ui.add_space(6.0);

            let recs_all = truesight_core::math::recommend_heroes(
                &self.session.draft,
                &self.session.matrix,
                &self.session.weights,
                40,
            );
            let filter_lower = self.suggest_filter.to_ascii_lowercase();

            let table_w = ui.available_width();
            let col_gap = ((table_w - 760.0) / 10.0).clamp(8.0, 22.0);
            let hero_col_w = (table_w - 720.0 - 10.0 * col_gap).max(165.0);

            egui::ScrollArea::vertical()
                .id_salt("v4_suggested_picks_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("v4_suggested_picks_grid")
                        .striped(true)
                        .spacing([col_gap, 6.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("#").size(9.5).strong().color(P_MUTED));
                            ui.allocate_ui_with_layout(
                                vec2(hero_col_w, 16.0),
                                Layout::left_to_right(Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new("HERO").size(9.5).strong().color(P_MUTED),
                                    );
                                },
                            );
                            ui.label(
                                RichText::new("PROJECTED WIN")
                                    .size(9.5)
                                    .strong()
                                    .color(P_MUTED),
                            );
                            ui.label(RichText::new("GAIN").size(9.5).strong().color(P_MUTED));
                            ui.label(RichText::new("META WR").size(9.5).strong().color(P_MUTED));
                            ui.label(RichText::new("7D").size(9.5).strong().color(P_MUTED));
                            ui.label(
                                RichText::new("VS ENEMIES")
                                    .size(9.5)
                                    .strong()
                                    .color(P_MUTED),
                            );
                            ui.label(RichText::new("AVG CNT").size(9.5).strong().color(P_MUTED));
                            ui.label(RichText::new("AVG SYN").size(9.5).strong().color(P_MUTED));
                            ui.label(RichText::new("COMFORT").size(9.5).strong().color(P_MUTED));
                            ui.label(RichText::new("").size(9.5));
                            ui.end_row();

                            for (idx, rec) in recs_all.iter().enumerate() {
                                let profile = self.session.matrix.hero(rec.hero_id);
                                let name = profile
                                    .map(|h| h.display_name.as_str())
                                    .unwrap_or("Unknown");
                                if !filter_lower.is_empty()
                                    && !name.to_ascii_lowercase().contains(&filter_lower)
                                {
                                    continue;
                                }

                                // 1. Rank
                                ui.label(
                                    RichText::new(format!("{}", idx + 1))
                                        .size(11.0)
                                        .strong()
                                        .color(P_MUTED),
                                );

                                // 2. Hero Portrait + Name
                                ui.allocate_ui_with_layout(
                                    vec2(hero_col_w, 27.0),
                                    Layout::left_to_right(Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = 7.0;
                                        let tex = self
                                            .textures
                                            .heroes
                                            .get(&rec.hero_id)
                                            .map(|s| &s.color);
                                        paint_portrait_16_9(ui, tex, vec2(48.0, 27.0), P_LINE, 1.0);
                                        ui.label(
                                            RichText::new(name)
                                                .size(12.0)
                                                .strong()
                                                .color(P_GLINT),
                                        );
                                    },
                                );

                                // 3. Projected Win% + bar
                                let proj = rec.projected_win_probability * 100.0;
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    ui.label(
                                        RichText::new(format!("{proj:4.1}%"))
                                            .size(12.0)
                                            .strong()
                                            .color(P_GLINT),
                                    );
                                    let (bar_r, _) =
                                        ui.allocate_exact_size(vec2(52.0, 4.0), Sense::hover());
                                    ui.painter().rect_filled(bar_r, 0.0, P_INDIGO_3);
                                    let fw = bar_r.width() * (proj / 100.0).clamp(0.0, 1.0);
                                    ui.painter().rect_filled(
                                        Rect::from_min_max(
                                            bar_r.min,
                                            pos2(bar_r.min.x + fw, bar_r.max.y),
                                        ),
                                        0.0,
                                        P_GEM,
                                    );
                                });

                                // 4. Draft Gain
                                let gain = rec.net_probability_delta * 100.0;
                                ui.label(
                                    RichText::new(fmt_pct(gain))
                                        .size(12.0)
                                        .strong()
                                        .color(tone_for_user(gain, 1.0).color()),
                                );

                                // 5. Meta WR
                                ui.label(
                                    RichText::new(format!("{:4.1}%", rec.base_meta_wr * 100.0))
                                        .size(11.5)
                                        .color(P_SILVER),
                                );

                                // 6. 7d Trend
                                let trend_pct = profile.map(|h| h.trend_delta * 100.0).unwrap_or(0.0);
                                if trend_pct.abs() >= EPS_PCT {
                                    paint_trend_badge(ui, trend_pct);
                                } else {
                                    ui.label(RichText::new("·").size(11.0).color(P_MUTED));
                                }

                                // 7. VS Enemies (5 square tinted portraits)
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    for j in 0..TEAM_SIZE {
                                        if let Some(ep) = self.session.draft.enemies[j] {
                                            let d = self
                                                .session
                                                .matrix
                                                .cell(rec.hero_id, ep.hero_id)
                                                .counter_delta
                                                * 100.0;
                                            let tex = self
                                                .textures
                                                .heroes
                                                .get(&ep.hero_id)
                                                .map(|s| &s.toned);
                                            paint_tinted_square_cell(ui, 26.0, tex, d, 8.5, 1.5);
                                        } else {
                                            let (r, _) = ui.allocate_exact_size(
                                                vec2(26.0, 26.0),
                                                Sense::hover(),
                                            );
                                            ui.painter().rect_stroke(
                                                r,
                                                0.0,
                                                Stroke::new(1.0_f32, P_LINE),
                                                StrokeKind::Inside,
                                            );
                                        }
                                    }
                                });

                                // 8. Avg Cnt
                                let cnt = rec.counter_delta_sum * 100.0;
                                ui.label(
                                    RichText::new(fmt_pct(cnt))
                                        .size(11.5)
                                        .strong()
                                        .color(tone_for_user(cnt, 1.0).color()),
                                );

                                // 9. Avg Syn
                                let syn = rec.synergy_delta_sum * 100.0;
                                ui.label(
                                    RichText::new(fmt_pct(syn))
                                        .size(11.5)
                                        .strong()
                                        .color(tone_for_user(syn, 1.0).color()),
                                );

                                // 10. Comfort (% + 5-pip cyan meter)
                                let com = rec.comfort_delta * 100.0;
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 5.0;
                                    ui.label(
                                        RichText::new(fmt_pct(com))
                                            .size(11.5)
                                            .strong()
                                            .color(tone_for_user(com, 1.0).color()),
                                    );
                                    let pips =
                                        ((com.max(0.0) / 0.5).round() as usize).clamp(0, 5);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 2.0;
                                        for k in 0..5 {
                                            let (pr, _) = ui.allocate_exact_size(
                                                vec2(4.0, 10.0),
                                                Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                pr,
                                                0.0,
                                                if k < pips { P_CYAN } else { P_INDIGO_3 },
                                            );
                                        }
                                    });
                                });

                                // 11. PICK button
                                let pick_btn = egui::Button::new(
                                    RichText::new("PICK")
                                        .size(10.0)
                                        .strong()
                                        .color(P_GEM),
                                )
                                .fill(P_INDIGO_1)
                                .stroke(Stroke::new(1.0_f32, P_GEM));
                                if ui.add(pick_btn).clicked() {
                                    let _ = self.session.draft.set_ally(
                                        target_slot_idx,
                                        Some(DraftPick {
                                            hero_id: rec.hero_id,
                                            role: Some(target_role),
                                        }),
                                    );
                                    self.status_banner = format!(
                                        "Picked {} into Ally P{}.",
                                        name,
                                        target_slot_idx + 1
                                    );
                                }
                                ui.end_row();
                            }
                        });
                });
        })
    }

    fn render_matrix_card(&mut self, ui: &mut egui::Ui, analysis: &DraftAnalysis5v5) -> Rect {
        let title = match self.matrix_tab {
            MatrixTab::Counter => "COUNTER MATRIX",
            MatrixTab::Synergy => "ALLY SYNERGY",
            MatrixTab::Comp => "COMPOSITION",
        };

        hud_card_fill(ui, |ui| {
            // Header with COUNTER | SYNERGY | COMP tabs
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(11.0).strong().color(P_GLINT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    for (tab, lbl) in [
                        (MatrixTab::Comp, "COMP"),
                        (MatrixTab::Synergy, "SYNERGY"),
                        (MatrixTab::Counter, "COUNTER"),
                    ] {
                        let is_on = self.matrix_tab == tab;
                        let btn = egui::Button::new(
                            RichText::new(lbl)
                                .size(10.0)
                                .strong()
                                .color(if is_on { P_VOID } else { P_MUTED }),
                        )
                        .fill(if is_on { P_GEM } else { P_INDIGO_2 })
                        .stroke(Stroke::new(1.0_f32, if is_on { P_GEM } else { P_LINE }));
                        if ui.add(btn).clicked() {
                            self.matrix_tab = tab;
                        }
                    }
                });
            });

            ui.add_space(8.0);

            match self.matrix_tab {
                MatrixTab::Counter => self.render_counter_tab(ui, analysis),
                MatrixTab::Synergy => self.render_synergy_tab(ui, analysis),
                MatrixTab::Comp => self.render_comp_tab(ui, analysis),
            }
        })
    }

    fn matrix_grid_metrics(ui: &egui::Ui) -> (Vec2, f32, f32) {
        let gap = 4.0_f32;
        let avail_w = ui.available_width().max(260.0);
        let avail_h = ui.available_height().max(280.0);
        // Reserve ~44px at bottom for the gradient scale bar + 1-2 line legend
        let cell_w = ((avail_w - 6.0 * gap) / 7.0).floor().clamp(34.0, 62.0);
        let cell_h = ((avail_h - 46.0 - 6.0 * gap) / 7.0)
            .floor()
            .clamp(32.0, cell_w)
            .min(58.0);
        let h_gap = ((avail_w - 7.0 * cell_w) / 6.0).clamp(4.0, 10.0);
        (vec2(cell_w, cell_h), h_gap, gap)
    }

    fn render_counter_tab(&self, ui: &mut egui::Ui, analysis: &DraftAnalysis5v5) {
        let (cell_sz, h_gap, v_gap) = Self::matrix_grid_metrics(ui);

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = v_gap;

            // Header Row: Blank + 5 Enemy portraits + AVG
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = h_gap;
                ui.allocate_exact_size(cell_sz, Sense::hover());
                for j in 0..TEAM_SIZE {
                    let (r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                    if let Some(ep) = self.session.draft.enemies[j] {
                        if let Some(t) = self.textures.heroes.get(&ep.hero_id).map(|s| &s.color) {
                            ui.painter()
                                .image(t.id(), r, uv_aspect_crop(cell_sz), Color32::WHITE);
                        }
                    } else {
                        ui.painter().rect_stroke(
                            r,
                            0.0,
                            Stroke::new(1.0_f32, P_LINE),
                            StrokeKind::Inside,
                        );
                        ui.painter().text(
                            r.center(),
                            Align2::CENTER_CENTER,
                            format!("E{}", j + 1),
                            FontId::proportional(10.0),
                            P_MUTED,
                        );
                    }
                    ui.painter().line_segment(
                        [pos2(r.min.x, r.max.y), r.max],
                        Stroke::new(2.0_f32, P_DIRE),
                    );
                }
                let (avg_r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                ui.painter().rect_filled(avg_r, 0.0, P_INDIGO_2);
                ui.painter().rect_stroke(
                    avg_r,
                    0.0,
                    Stroke::new(1.0_f32, P_LINE),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    avg_r.center(),
                    Align2::CENTER_CENTER,
                    "AVG",
                    FontId::proportional(10.0),
                    P_GLINT,
                );
            });

            // 5 Ally Rows
            for i in 0..TEAM_SIZE {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = h_gap;
                    // Row Header
                    let (rh_r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                    if let Some(ap) = self.session.draft.allies[i] {
                        if let Some(t) = self.textures.heroes.get(&ap.hero_id).map(|s| &s.color) {
                            ui.painter()
                                .image(t.id(), rh_r, uv_aspect_crop(cell_sz), Color32::WHITE);
                        }
                        ui.painter().line_segment(
                            [pos2(rh_r.max.x, rh_r.min.y), rh_r.max],
                            Stroke::new(2.0_f32, P_GEM),
                        );
                    } else {
                        ui.painter().rect_stroke(
                            rh_r,
                            0.0,
                            Stroke::new(1.0_f32, P_GEM),
                            StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rh_r.center(),
                            Align2::CENTER_CENTER,
                            format!("P{}", i + 1),
                            FontId::proportional(10.0),
                            P_GEM,
                        );
                    }

                    // 5 Matchup Cells
                    for j in 0..TEAM_SIZE {
                        if let (Some(ap), Some(ep), Some(delta)) = (
                            self.session.draft.allies[i],
                            self.session.draft.enemies[j],
                            analysis.counter_grid[i][j],
                        ) {
                            let pct = delta * 100.0;
                            let winner_id = if pct <= -EPS_PCT {
                                ep.hero_id
                            } else {
                                ap.hero_id
                            };
                            let tex = self.textures.heroes.get(&winner_id).map(|s| &s.toned);
                            paint_tinted_cell(ui, cell_sz, tex, pct, 11.0, 2.5);
                        } else {
                            let (er, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                            ui.painter().rect_stroke(
                                er,
                                0.0,
                                Stroke::new(1.0_f32, P_LINE),
                                StrokeKind::Inside,
                            );
                        }
                    }

                    // Row AVG
                    paint_avg_square_cell(
                        ui,
                        cell_sz,
                        analysis.ally_counter_avg[i].map(|v| v * 100.0),
                    );
                });
            }

            // Bottom AVG Row + Total Counter Edge corner
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = h_gap;
                let (lbl_r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                ui.painter().rect_filled(lbl_r, 0.0, P_INDIGO_2);
                ui.painter().rect_stroke(
                    lbl_r,
                    0.0,
                    Stroke::new(1.0_f32, P_LINE),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    lbl_r.center(),
                    Align2::CENTER_CENTER,
                    "AVG",
                    FontId::proportional(10.0),
                    P_GLINT,
                );
                for j in 0..TEAM_SIZE {
                    let col_avg = enemy_counter_col_avg(analysis, j).map(|v| v * 100.0);
                    paint_avg_square_cell(ui, cell_sz, col_avg);
                }
                paint_total_corner_cell(ui, cell_sz, analysis.total_counter_edge * 100.0, "EDGE");
            });
        });

        ui.add_space(6.0);
        paint_matrix_scale_bar(ui, "Enemy wins", "Ally wins");
        ui.add_space(2.0);
        ui.label(
            RichText::new("Cell portrait = matchup winner. Right col = ally avg; bottom row = enemy avg vs team.")
                .size(9.0)
                .color(P_MUTED),
        );
    }

    fn render_synergy_tab(&self, ui: &mut egui::Ui, analysis: &DraftAnalysis5v5) {
        let (cell_sz, h_gap, v_gap) = Self::matrix_grid_metrics(ui);

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = v_gap;

            // Header Row: Blank + 5 Ally portraits + AVG
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = h_gap;
                ui.allocate_exact_size(cell_sz, Sense::hover());
                for k in 0..TEAM_SIZE {
                    let (r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                    if let Some(ap) = self.session.draft.allies[k] {
                        if let Some(t) = self.textures.heroes.get(&ap.hero_id).map(|s| &s.color) {
                            ui.painter()
                                .image(t.id(), r, uv_aspect_crop(cell_sz), Color32::WHITE);
                        }
                    } else {
                        ui.painter().rect_stroke(
                            r,
                            0.0,
                            Stroke::new(1.0_f32, P_GEM),
                            StrokeKind::Inside,
                        );
                        ui.painter().text(
                            r.center(),
                            Align2::CENTER_CENTER,
                            format!("P{}", k + 1),
                            FontId::proportional(10.0),
                            P_GEM,
                        );
                    }
                    ui.painter().line_segment(
                        [pos2(r.min.x, r.max.y), r.max],
                        Stroke::new(2.0_f32, P_GEM),
                    );
                }
                let (avg_r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                ui.painter().rect_filled(avg_r, 0.0, P_INDIGO_2);
                ui.painter().rect_stroke(
                    avg_r,
                    0.0,
                    Stroke::new(1.0_f32, P_LINE),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    avg_r.center(),
                    Align2::CENTER_CENTER,
                    "AVG",
                    FontId::proportional(10.0),
                    P_GLINT,
                );
            });

            // 5 Ally Rows
            for i in 0..TEAM_SIZE {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = h_gap;
                    let (rh_r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                    if let Some(ap) = self.session.draft.allies[i] {
                        if let Some(t) = self.textures.heroes.get(&ap.hero_id).map(|s| &s.color) {
                            ui.painter()
                                .image(t.id(), rh_r, uv_aspect_crop(cell_sz), Color32::WHITE);
                        }
                        ui.painter().line_segment(
                            [pos2(rh_r.max.x, rh_r.min.y), rh_r.max],
                            Stroke::new(2.0_f32, P_GEM),
                        );
                    } else {
                        ui.painter().rect_stroke(
                            rh_r,
                            0.0,
                            Stroke::new(1.0_f32, P_GEM),
                            StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rh_r.center(),
                            Align2::CENTER_CENTER,
                            format!("P{}", i + 1),
                            FontId::proportional(10.0),
                            P_GEM,
                        );
                    }

                    for k in 0..TEAM_SIZE {
                        if i == k {
                            let (dr, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                            ui.painter().rect_filled(dr, 0.0, P_INDIGO_2);
                            ui.painter().rect_stroke(
                                dr,
                                0.0,
                                Stroke::new(1.0_f32, P_LINE),
                                StrokeKind::Inside,
                            );
                            ui.painter().text(
                                dr.center(),
                                Align2::CENTER_CENTER,
                                "—",
                                FontId::proportional(11.0),
                                P_LINE,
                            );
                        } else if let (Some(partner), Some(syn)) = (
                            self.session.draft.allies[k],
                            analysis.ally_synergy_grid[i][k],
                        ) {
                            let pct = syn * 100.0;
                            let tex = self
                                .textures
                                .heroes
                                .get(&partner.hero_id)
                                .map(|s| &s.toned);
                            paint_tinted_cell(ui, cell_sz, tex, pct, 11.0, 2.5);
                        } else {
                            let (er, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                            ui.painter().rect_stroke(
                                er,
                                0.0,
                                Stroke::new(1.0_f32, P_LINE),
                                StrokeKind::Inside,
                            );
                        }
                    }

                    paint_avg_square_cell(
                        ui,
                        cell_sz,
                        analysis.ally_synergy_avg[i].map(|v| v * 100.0),
                    );
                });
            }

            // Bottom summary row
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = h_gap;
                for _ in 0..6 {
                    let (er, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                    ui.painter().rect_stroke(
                        er,
                        0.0,
                        Stroke::new(1.0_f32, P_LINE),
                        StrokeKind::Inside,
                    );
                }
                paint_total_corner_cell(
                    ui,
                    cell_sz,
                    analysis.ally_synergy_norm * 100.0,
                    "ALLY SYN",
                );
            });
        });

        ui.add_space(6.0);
        paint_matrix_scale_bar(ui, "Anti-synergy", "Synergy");
        ui.add_space(2.0);
        ui.label(
            RichText::new("Symmetric ally pairs. Right col = avg synergy with allies; corner = normalized Ally Synergy.")
                .size(9.0)
                .color(P_MUTED),
        );
    }

    fn render_comp_tab(&self, ui: &mut egui::Ui, analysis: &DraftAnalysis5v5) {
        let ally_diag =
            compute_comp_diagnostic(&self.session.draft.allies, &self.session.matrix);
        let enemy_diag =
            compute_comp_diagnostic(&self.session.draft.enemies, &self.session.matrix);
        let ally_pct = ally_diag.score * 100.0;
        let enemy_pct = enemy_diag.score * 100.0;
        let comp_edge = analysis.total_composition_edge * 100.0;

        let avail_w = ui.available_width().max(280.0);

        // 1. Header: Allies | Comp Edge | Enemies (fixed height so egui never centers into remaining height)
        let hdr_mid_w = 92.0_f32;
        let hdr_side_w = ((avail_w - hdr_mid_w - 8.0) * 0.5).max(80.0);
        ui.allocate_ui_with_layout(
            vec2(avail_w, 38.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.allocate_ui_with_layout(
                    vec2(hdr_side_w, 38.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.label(
                            RichText::new(format!("ALLIES · {} / 5", ally_diag.picked))
                                .size(9.5)
                                .color(P_MUTED),
                        );
                        ui.label(
                            RichText::new(fmt_pct(ally_pct))
                                .size(18.0)
                                .strong()
                                .color(tone_for_user(ally_pct, 1.0).color()),
                        );
                    },
                );
                ui.allocate_ui_with_layout(
                    vec2(hdr_mid_w, 38.0),
                    Layout::top_down(Align::Center),
                    |ui| {
                        ui.label(
                            RichText::new(fmt_pct(comp_edge))
                                .size(18.0)
                                .strong()
                                .color(tone_for_user(comp_edge, 1.0).color()),
                        );
                        ui.label(RichText::new("COMP EDGE").size(9.0).color(P_MUTED));
                    },
                );
                ui.allocate_ui_with_layout(
                    vec2(hdr_side_w, 38.0),
                    Layout::top_down(Align::Max),
                    |ui| {
                        ui.label(
                            RichText::new(format!("ENEMIES · {} / 5", enemy_diag.picked))
                                .size(9.5)
                                .color(P_MUTED),
                        );
                        ui.label(
                            RichText::new(fmt_pct(enemy_pct))
                                .size(18.0)
                                .strong()
                                .color(tone_for_user(enemy_pct, -1.0).color()),
                        );
                    },
                );
            },
        );

        ui.add_space(6.0);

        // 2. Four Mirror Rows (Melee, Ranged, Disables, Initiators)
        let mirror_rows: [(&str, &str, &[HeroId], &[HeroId]); 4] = [
            ("MELEE", "1-3 ideal", &ally_diag.melee, &enemy_diag.melee),
            ("RANGED", "avoid 5", &ally_diag.ranged, &enemy_diag.ranged),
            (
                "DISABLES",
                ">= 2 ideal",
                &ally_diag.disables,
                &enemy_diag.disables,
            ),
            (
                "INITIATORS",
                ">= 1 ideal",
                &ally_diag.initiators,
                &enemy_diag.initiators,
            ),
        ];

        let mir_mid_w = 82.0_f32;
        let mir_side_w = ((avail_w - mir_mid_w - 8.0) * 0.5).max(80.0);

        for (title, sub, a_ids, e_ids) in mirror_rows {
            ui.allocate_ui_with_layout(
                vec2(avail_w, 26.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    ui.allocate_ui_with_layout(
                        vec2(mir_side_w, 26.0),
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.label(
                                RichText::new(format!("{}", a_ids.len()))
                                    .size(13.0)
                                    .strong()
                                    .color(P_GEM),
                            );
                            for hid in a_ids {
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                                if let Some(t) =
                                    self.textures.heroes.get(hid).map(|s| &s.color)
                                {
                                    ui.painter().image(
                                        t.id(),
                                        r,
                                        uv_square_crop(),
                                        Color32::WHITE,
                                    );
                                }
                                ui.painter().rect_stroke(
                                    r,
                                    0.0,
                                    Stroke::new(1.0_f32, P_GEM_DEEP),
                                    StrokeKind::Inside,
                                );
                            }
                        },
                    );

                    ui.allocate_ui_with_layout(
                        vec2(mir_mid_w, 26.0),
                        Layout::top_down(Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(RichText::new(title).size(9.5).strong().color(P_SILVER));
                            ui.label(RichText::new(sub).size(8.5).color(P_MUTED));
                        },
                    );

                    ui.allocate_ui_with_layout(
                        vec2(mir_side_w, 26.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.label(
                                RichText::new(format!("{}", e_ids.len()))
                                    .size(13.0)
                                    .strong()
                                    .color(P_DIRE),
                            );
                            for hid in e_ids {
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                                if let Some(t) =
                                    self.textures.heroes.get(hid).map(|s| &s.color)
                                {
                                    ui.painter().image(
                                        t.id(),
                                        r,
                                        uv_square_crop(),
                                        Color32::WHITE,
                                    );
                                }
                                ui.painter().rect_stroke(
                                    r,
                                    0.0,
                                    Stroke::new(1.0_f32, P_DIRE_DEEP),
                                    StrokeKind::Inside,
                                );
                            }
                        },
                    );
                },
            );
            ui.add_space(3.0);
        }

        ui.add_space(8.0);

        // 3. Six-Rule Table filling 100% width
        let rule_col_w = (avail_w - 204.0).max(165.0);
        egui::Grid::new("v4_comp_rules_grid")
            .striped(true)
            .spacing([12.0, 5.0])
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    vec2(rule_col_w, 16.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(RichText::new("RULE").size(9.0).strong().color(P_MUTED));
                    },
                );
                ui.label(RichText::new("IMPACT").size(9.0).strong().color(P_MUTED));
                ui.label(RichText::new("ALLIES").size(9.0).strong().color(P_MUTED));
                ui.label(RichText::new("ENEMIES").size(9.0).strong().color(P_MUTED));
                ui.end_row();

                for (idx, &(rule_name, impact)) in COMP_RULES.iter().enumerate() {
                    let imp_pct = impact * 100.0;
                    ui.allocate_ui_with_layout(
                        vec2(rule_col_w, 18.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.label(RichText::new(rule_name).size(10.0).color(P_SILVER));
                        },
                    );
                    ui.label(
                        RichText::new(fmt_pct(imp_pct))
                            .size(10.0)
                            .color(tone_for_user(imp_pct, 1.0).color()),
                    );
                    if ally_diag.rule_active[idx] {
                        let c = tone_for_user(imp_pct, 1.0).color();
                        paint_mini_badge(ui, &fmt_pct(imp_pct), c, c);
                    } else {
                        ui.label(RichText::new("·").size(10.0).color(P_LINE));
                    }
                    if enemy_diag.rule_active[idx] {
                        let c = tone_for_user(imp_pct, -1.0).color();
                        paint_mini_badge(ui, &fmt_pct(imp_pct), c, c);
                    } else {
                        ui.label(RichText::new("·").size(10.0).color(P_LINE));
                    }
                    ui.end_row();
                }
            });

        ui.add_space(10.0);

        // 4. Next Pick Hint Box
        let target_idx = self.session.draft.target_role.index();
        let m_cnt = ally_diag.melee.len();
        let r_cnt = ally_diag.ranged.len();
        let hint_fr = egui::Frame::NONE
            .fill(P_INDIGO_2)
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new(format!(
                        "Next pick (Pos {}): melee -> {}M / {}R · ranged -> {}M / {}R.",
                        target_idx + 1,
                        m_cnt + 1,
                        r_cnt,
                        m_cnt,
                        r_cnt + 1
                    ))
                    .size(10.0)
                    .color(P_SILVER),
                );
            });
        let hr = hint_fr.response.rect;
        ui.painter().rect_filled(
            Rect::from_min_max(hr.min, pos2(hr.min.x + 3.0, hr.max.y)),
            0.0,
            P_GEM,
        );
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LayoutRects {
    pub top_bar: Rect,
    pub allies: Rect,
    pub oracle: Rect,
    pub enemies: Rect,
    pub suggested: Rect,
    pub matrix: Rect,
}

impl TrueSightGuiApp {
    pub fn render_frame(&mut self, ctx: &egui::Context) -> LayoutRects {
        self.configure_visuals(ctx);
        self.textures.ensure_loaded(ctx);

        if let Some(ref mut listener) = self.gsi_listener {
            if let Ok(events) = listener.poll_events() {
                for ev in events {
                    self.session.apply_event(ev);
                }
            }
        }

        let (analysis, _) = self.session.evaluate();
        let mut rects = LayoutRects {
            top_bar: Rect::NOTHING,
            allies: Rect::NOTHING,
            oracle: Rect::NOTHING,
            enemies: Rect::NOTHING,
            suggested: Rect::NOTHING,
            matrix: Rect::NOTHING,
        };

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(P_VOID).inner_margin(Margin::same(10)))
            .show(ctx, |ui| {
                // 1. Top Bar
                rects.top_bar = self.render_top_bar(ui);
                ui.add_space(10.0);

                // 2. Top Row: Allies (348px) | Oracle (flex) | Enemies (348px)
                let total_w = ui.available_width();
                let side_w = 348.0_f32.min(total_w * 0.27);
                let gap = 10.0;
                let center_w = (total_w - 2.0 * side_w - 2.0 * gap).max(360.0);
                let top_row_h = 384.0_f32;

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.allocate_ui_with_layout(
                        vec2(side_w, top_row_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            rects.allies = self.render_team_card(ui, &analysis, Side::Allies);
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(center_w, top_row_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            rects.oracle = self.render_oracle_card(ui, &analysis);
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(side_w, top_row_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            rects.enemies = self.render_team_card(ui, &analysis, Side::Enemies);
                        },
                    );
                });

                ui.add_space(10.0);

                // 3. Bottom Row: Suggested Picks (flex) | 5v5 Matrix / Comp (434px)
                let matrix_w = 434.0_f32.min(total_w * 0.34);
                let recs_w = (total_w - matrix_w - gap).max(480.0);
                let bottom_h = ui.available_height().max(320.0);

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.allocate_ui_with_layout(
                        vec2(recs_w, bottom_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            rects.suggested = self.render_suggested_picks_card(ui);
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(matrix_w, bottom_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            rects.matrix = self.render_matrix_card(ui, &analysis);
                        },
                    );
                });
            });

        rects
    }
}

impl eframe::App for TrueSightGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _ = self.render_frame(ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

pub fn run_gui(session: AppSession, gsi_listener: Option<BoundedGsiListener>) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1468.0, 960.0])
            .with_title("TrueSight Dota — 5v5 Live Draft Oracle"),
        ..Default::default()
    };
    eframe::run_native(
        "TrueSight Dota",
        options,
        Box::new(move |_cc| Ok(Box::new(TrueSightGuiApp::new(session, gsi_listener)))),
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
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

        let mut app = TrueSightGuiApp::new(session, None);
        app.picker_target = Some(PickerTarget::Slot(Side::Allies, 1));

        let ctx = egui::Context::default();
        let raw_input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1468.0, 960.0))),
            ..Default::default()
        };

        for tab in [MatrixTab::Counter, MatrixTab::Synergy, MatrixTab::Comp] {
            app.matrix_tab = tab;
            let mut layout = None;
            for _ in 0..2 {
                let _ = ctx.run(raw_input.clone(), |ctx| {
                    layout = Some(app.render_frame(ctx));
                });
            }
            let r = layout.expect("layout rects captured");

            // Top bar must stay compact and never consume the whole window
            assert!(
                r.top_bar.height() > 24.0 && r.top_bar.height() < 65.0,
                "tab={tab:?} top_bar height out of range: {:?}",
                r.top_bar
            );

            // Top row cards must start below top_bar and share identical top Y & bottom Y
            assert!(r.allies.min.y > r.top_bar.max.y);
            assert!((r.allies.min.y - r.oracle.min.y).abs() <= 1.5, "tab={tab:?} allies={:?} oracle={:?}", r.allies, r.oracle);
            assert!((r.allies.min.y - r.enemies.min.y).abs() <= 1.5, "tab={tab:?} allies={:?} enemies={:?}", r.allies, r.enemies);
            assert!((r.allies.max.y - r.oracle.max.y).abs() <= 2.0, "tab={tab:?} allies={:?} oracle={:?}", r.allies, r.oracle);
            assert!((r.allies.max.y - r.enemies.max.y).abs() <= 2.0, "tab={tab:?} allies={:?} enemies={:?}", r.allies, r.enemies);
            assert!(r.enemies.max.x <= 1468.0, "tab={tab:?} enemies overflowed right: {:?}", r.enemies);

            // Bottom row cards must start below top row, align top & bottom, and stay inside viewport
            assert!(r.suggested.min.y > r.allies.max.y);
            assert!((r.suggested.min.y - r.matrix.min.y).abs() <= 1.5, "tab={tab:?} suggested={:?} matrix={:?}", r.suggested, r.matrix);
            assert!((r.suggested.max.y - r.matrix.max.y).abs() <= 2.0, "tab={tab:?} suggested={:?} matrix={:?}", r.suggested, r.matrix);
            assert!(r.matrix.max.x <= 1468.0 && r.matrix.max.y <= 960.0, "tab={tab:?} bottom overflow: {:?}", r.matrix);
        }
    }
}
