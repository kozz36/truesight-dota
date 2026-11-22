use super::TrueSightGuiApp;
use super::theme::*;
use super::viewmodel::OverlaySummary;
use eframe::egui::{self, Align, Align2, Color32, FontId, Margin, RichText, Sense, Stroke, vec2};
use truesight_core::domain::{ItemId, ItemPhase, OptionalBuildItem};
use truesight_core::ports::ItemMetaRepository;

fn phase_short(phase: ItemPhase) -> &'static str {
    match phase {
        ItemPhase::Starting => "START",
        ItemPhase::Early => "EARLY",
        ItemPhase::Core => "CORE",
        ItemPhase::Late => "LATE",
        ItemPhase::Situational => "OPT",
    }
}

fn format_clock(secs: i32) -> String {
    let abs = secs.unsigned_abs();
    let mins = abs / 60;
    let rem = abs % 60;
    if secs < 0 {
        format!("-{mins:02}:{rem:02}")
    } else {
        format!("{mins:02}:{rem:02}")
    }
}

fn paint_corner_pill(
    painter: &egui::Painter,
    anchor: egui::Pos2,
    align: Align2,
    text: &str,
    text_color: Color32,
    bg_color: Color32,
    font_size: f32,
) {
    let galley = painter.layout_no_wrap(
        text.to_string(),
        FontId::proportional(font_size),
        text_color,
    );
    let pad = vec2(2.5, 0.5);
    let box_sz = galley.size() + pad * 2.0;
    let rect = align.anchor_size(anchor, box_sz);
    painter.rect_filled(rect, 1.5, bg_color);
    painter.galley(rect.min + pad, galley, text_color);
}

impl TrueSightGuiApp {
    /// Renders the ultra-compact icon-only responsive item build view inside the overlay,
    /// returning `Some((primary_item_id, variant_item_id))` when the user clicks a mini variant icon.
    pub(super) fn render_overlay_build_panel(
        &self,
        ui: &mut egui::Ui,
        summary: &OverlaySummary,
    ) -> Option<(ItemId, ItemId)> {
        let tr = super::i18n::strings(self.session.ui_language);
        let mut clicked_variant: Option<(ItemId, ItemId)> = None;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                RichText::new(format!("T = {}", format_clock(summary.game_time_secs)))
                    .size(9.0)
                    .strong()
                    .color(P_CYAN),
            );
            if let Some((hero_id, role)) = self.session.active_build_hero() {
                let hero_name = self
                    .session
                    .matrix
                    .hero(hero_id)
                    .map(|h| h.display_name.clone())
                    .unwrap_or_else(|| format!("Hero #{}", hero_id.0));
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{hero_name} · P{}", role.index() + 1))
                            .size(9.0)
                            .strong()
                            .color(P_MUTED),
                    )
                    .truncate(),
                );
            } else {
                ui.label(
                    RichText::new(tr.overlay_item_build_label)
                        .size(9.0)
                        .strong()
                        .color(P_MUTED),
                );
            }
        });
        ui.add_space(3.0);

        let Some(build) = &summary.build else {
            ui.label(
                RichText::new(tr.overlay_pick_ally_hint)
                    .size(9.5)
                    .color(P_MUTED),
            );
            return None;
        };

        let avail_w = ui.available_width();
        let avail_h = (ui.available_height() - 6.0).max(36.0);
        let show_opt_row = !build.optional_items.is_empty() && avail_h >= 54.0;
        let opt_reserve_h = if show_opt_row { 22.0 } else { 0.0 };
        let main_h = (avail_h - opt_reserve_h).max(28.0);

        let step_count = build.steps.len().max(1) as f32;
        let chevron_budget = (step_count - 1.0).max(0.0) * 10.0;
        let width_tile = ((avail_w - chevron_budget - step_count * 6.0) / step_count).floor();
        let height_tile = (main_h * 0.70 / 0.73).floor();
        let tile_w = width_tile.min(height_tile).clamp(32.0, 50.0);
        let tile_h = (tile_w * 0.73).round().clamp(24.0, 36.0);
        let variant_w = (tile_w * 0.56).round().clamp(18.0, 25.0);
        let variant_h = (variant_w * 0.72).round().clamp(13.0, 18.0);
        let show_variants = main_h >= tile_h + variant_h + 10.0;

        ui.allocate_ui_with_layout(
            vec2(avail_w, main_h),
            egui::Layout::top_down(Align::Min),
            |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("truesight_overlay_build_scroll")
                    .auto_shrink([false, false])
                    .animated(false)
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            for (idx, step) in build.steps.iter().enumerate() {
                                if idx > 0 {
                                    ui.vertical(|ui| {
                                        ui.add_space((tile_h * 0.35).max(4.0));
                                        let (cr, _) =
                                            ui.allocate_exact_size(vec2(8.0, 12.0), Sense::hover());
                                        paint_chevron_right(ui.painter(), cr, P_GEM);
                                    });
                                }

                                let is_next = build.next_unowned_index == Some(idx);
                                let border_col = if step.owned {
                                    P_GEM_DEEP
                                } else if is_next {
                                    P_GEM
                                } else {
                                    P_LINE
                                };
                                let (name, cost) = self
                                    .session
                                    .item_repo
                                    .item_catalog(step.item_id)
                                    .map(|e| (e.display_name, e.cost))
                                    .unwrap_or_else(|| (format!("#{}", step.item_id.0), 0));
                                let step_tex = self.textures.items.get(&step.item_id);

                                let step_resp = ui
                                    .vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        let icon_frame = egui::Frame::NONE
                                            .fill(P_INDIGO_2)
                                            .stroke(Stroke::new(
                                                if is_next { 1.5_f32 } else { 1.0_f32 },
                                                border_col,
                                            ))
                                            .inner_margin(Margin::same(1))
                                            .show(ui, |ui| {
                                                let (rect, resp) = ui.allocate_exact_size(
                                                    vec2(tile_w, tile_h),
                                                    Sense::hover(),
                                                );
                                                paint_item_icon_on(
                                                    ui.painter(),
                                                    rect,
                                                    step_tex,
                                                    border_col,
                                                    step.owned,
                                                );

                                                // Overlaid status pill (top-left) — zero wasted space.
                                                if step.owned {
                                                    paint_corner_pill(
                                                        ui.painter(),
                                                        rect.min + vec2(1.0, 1.0),
                                                        Align2::LEFT_TOP,
                                                        "OK",
                                                        P_VOID,
                                                        P_GEM,
                                                        7.0,
                                                    );
                                                } else if is_next {
                                                    paint_corner_pill(
                                                        ui.painter(),
                                                        rect.min + vec2(1.0, 1.0),
                                                        Align2::LEFT_TOP,
                                                        "NEXT",
                                                        P_VOID,
                                                        P_GEM,
                                                        7.0,
                                                    );
                                                } else if step.owned_components > 0 {
                                                    paint_corner_pill(
                                                        ui.painter(),
                                                        rect.min + vec2(1.0, 1.0),
                                                        Align2::LEFT_TOP,
                                                        &format!(
                                                            "{}/{}",
                                                            step.owned_components,
                                                            step.total_components
                                                        ),
                                                        P_VOID,
                                                        P_CYAN,
                                                        7.0,
                                                    );
                                                }

                                                // Overlaid timing pill (bottom-right) — preserves timing compactly.
                                                let timing_txt = format!(
                                                    "{}m",
                                                    step.avg_time_mins.round().max(1.0) as u32
                                                );
                                                paint_corner_pill(
                                                    ui.painter(),
                                                    rect.max - vec2(1.0, 1.0),
                                                    Align2::RIGHT_BOTTOM,
                                                    &timing_txt,
                                                    P_GLINT,
                                                    P_VOID.gamma_multiply(0.84),
                                                    7.5,
                                                );

                                                let trait_note = step
                                                    .counter_trait
                                                    .map(|t| format!(" · CNT: {}", t.label()))
                                                    .unwrap_or_default();
                                                let owned_note =
                                                    if step.owned { " · OWNED" } else { "" };
                                                resp.on_hover_text(format!(
                                                    "{name} ({cost}g) · ~{:.0}m · {}{}{}",
                                                    step.avg_time_mins,
                                                    phase_short(step.phase),
                                                    trait_note,
                                                    owned_note
                                                ));
                                            })
                                            .response;
                                        let _ = icon_frame;

                                        // Icon-only clickable mini variant row beneath primary icon
                                        if show_variants && !step.variants.is_empty() {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 2.0;
                                                for var in step.variants.iter().take(2) {
                                                    let (vname, vcost) = self
                                                        .session
                                                        .item_repo
                                                        .item_catalog(var.item_id)
                                                        .map(|e| (e.display_name, e.cost))
                                                        .unwrap_or_else(|| {
                                                            (format!("#{}", var.item_id.0), 0)
                                                        });
                                                    let (vr, vresp) = ui.allocate_exact_size(
                                                        vec2(variant_w, variant_h),
                                                        Sense::click(),
                                                    );
                                                    let vborder = if vresp.hovered() {
                                                        P_GEM
                                                    } else {
                                                        P_LINE
                                                    };
                                                    paint_item_icon_on(
                                                        ui.painter(),
                                                        vr,
                                                        self.textures.items.get(&var.item_id),
                                                        vborder,
                                                        false,
                                                    );
                                                    let trait_note = var
                                                        .counter_trait
                                                        .map(|t| format!(" · CNT: {}", t.label()))
                                                        .unwrap_or_default();
                                                    if vresp
                                                        .on_hover_text(format!(
                                                            "{vname} ({vcost}g) · ~{:.0}m{trait_note}",
                                                            var.avg_time_mins
                                                        ))
                                                        .clicked()
                                                    {
                                                        clicked_variant =
                                                            Some((step.item_id, var.item_id));
                                                    }
                                                }
                                            });
                                        }
                                    })
                                    .response;

                                if is_next {
                                    step_resp.scroll_to_me(Some(Align::Center));
                                }
                            }
                        });
                    });
            },
        );

        if show_opt_row {
            ui.add_space(2.0);
            self.render_overlay_optional_row(ui, &build.optional_items);
        }

        clicked_variant
    }

    fn render_overlay_optional_row(&self, ui: &mut egui::Ui, optional_items: &[OptionalBuildItem]) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(RichText::new("OPT").size(7.5).strong().color(P_NEUTRAL));
            for opt in optional_items.iter().take(6) {
                let (name, cost) = self
                    .session
                    .item_repo
                    .item_catalog(opt.item_id)
                    .map(|e| (e.display_name, e.cost))
                    .unwrap_or_else(|| (format!("#{}", opt.item_id.0), 0));
                let (r, resp) = ui.allocate_exact_size(vec2(26.0, 19.0), Sense::hover());
                let border = if opt.owned {
                    P_GEM_DEEP
                } else if resp.hovered() {
                    P_GEM
                } else {
                    P_NEUTRAL
                };
                paint_item_icon_on(
                    ui.painter(),
                    r,
                    self.textures.items.get(&opt.item_id),
                    border,
                    opt.owned,
                );
                let timing_txt = format!("{}m", opt.avg_time_mins.round().max(1.0) as u32);
                paint_corner_pill(
                    ui.painter(),
                    r.max - vec2(1.0, 1.0),
                    Align2::RIGHT_BOTTOM,
                    &timing_txt,
                    P_GLINT,
                    P_VOID.gamma_multiply(0.84),
                    6.5,
                );
                let reason = opt
                    .counter_trait
                    .map(|t| format!(" · CNT: {}", t.label()))
                    .unwrap_or_default();
                resp.on_hover_text(format!(
                    "{name} ({cost}g) · ~{:.0}m{reason}",
                    opt.avg_time_mins
                ));
            }
        });
    }
}
