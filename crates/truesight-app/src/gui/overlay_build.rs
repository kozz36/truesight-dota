use super::TrueSightGuiApp;
use super::i18n::UiStrings;
use super::surfaces::{
    estimate_overlay_build_row_width, format_build_clock as format_clock,
    overlay_build_phase_short as phase_short, paint_overlay_corner_pill as paint_corner_pill,
};
#[cfg(test)]
pub(super) use super::surfaces::{max_overlay_variant_rows, select_overlay_late_grid_2x2};
#[cfg(not(test))]
use super::surfaces::{max_overlay_variant_rows, select_overlay_late_grid_2x2};
use super::theme::*;
use super::viewmodel::OverlaySummary;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense,
    Stroke, vec2,
};
use truesight_core::domain::{BuildStep, HeroId, ItemId, ItemPhase, OptionalBuildItem, TEAM_SIZE};
use truesight_core::ports::ItemMetaRepository;

pub(super) struct OverlayBuildOutput {
    pub(super) clicked_variant: Option<(ItemId, ItemId)>,
    pub(super) clicked_ally_hero: Option<HeroId>,
    pub(super) ally_role_buttons: [Rect; TEAM_SIZE],
    pub(super) phases_rect: Option<Rect>,
}

impl TrueSightGuiApp {
    /// Renders the compact icon-only item build overlay organized into three framed phase
    /// boxes (`EARLY > CORE > LATE`), `P1`–`P5` ally selector pills, inline starting items,
    /// and a prominent framed `OPT` situational counter strip underneath.
    pub(super) fn render_overlay_build_panel(
        &self,
        ui: &mut egui::Ui,
        summary: &OverlaySummary,
        textures: &GuiTextureCache,
    ) -> OverlayBuildOutput {
        let tr = super::i18n::strings(self.session.ui_language);
        let mut clicked_variant: Option<(ItemId, ItemId)> = None;
        let mut clicked_ally_hero: Option<HeroId> = None;
        let mut ally_role_buttons = [Rect::NOTHING; TEAM_SIZE];
        let mut phases_rect: Option<Rect> = None;
        let active_build = self.session.active_build_hero();
        let start_items: &[ItemId] = summary
            .build
            .as_ref()
            .map(|b| b.starting_items.as_slice())
            .unwrap_or(&[]);

        let hdr_avail_w = ui.available_width();
        let inline_starting = !start_items.is_empty() && hdr_avail_w >= 330.0;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            super::surfaces::overlay_text_scrim(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(
                        RichText::new(format!("T = {}", format_clock(summary.game_time_secs)))
                            .size(9.0)
                            .strong()
                            .color(P_CYAN),
                    );
                    if let Some((hero_id, role)) = active_build {
                        let hero_name = self
                            .session
                            .matrix
                            .hero(hero_id)
                            .map(|h| h.display_name.as_str())
                            .unwrap_or("Hero");
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{hero_name} · P{}", role.index() + 1))
                                    .size(9.0)
                                    .strong()
                                    .color(P_SILVER),
                            )
                            .truncate(),
                        );
                    } else {
                        ui.label(
                            RichText::new(tr.overlay_item_build_label)
                                .size(9.0)
                                .strong()
                                .color(P_SILVER),
                        );
                    }
                });
            });

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for idx in 0..TEAM_SIZE {
                    let picked_hero = self.session.draft.allies[idx].map(|p| p.hero_id);
                    let is_on =
                        active_build.is_some_and(|(active_id, _)| picked_hero == Some(active_id));
                    let txt_col = if picked_hero.is_none() {
                        P_MUTED
                    } else if is_on {
                        P_VOID
                    } else {
                        P_SILVER
                    };
                    let fill_col = if is_on {
                        P_GEM
                    } else {
                        super::surfaces::translucent(P_INDIGO_2, 76)
                    };
                    let stroke_col = if is_on {
                        P_GEM
                    } else {
                        super::surfaces::translucent(P_LINE, 130)
                    };
                    let hover = match picked_hero
                        .and_then(|id| self.session.matrix.hero(id))
                        .map(|h| h.display_name.as_str())
                    {
                        Some(name) => format!("P{} · {name}", idx + 1),
                        None => format!("P{} · —", idx + 1),
                    };
                    let btn = egui::Button::new(
                        RichText::new(format!("P{}", idx + 1))
                            .size(8.0)
                            .strong()
                            .color(txt_col),
                    )
                    .fill(fill_col)
                    .stroke(Stroke::new(1.0_f32, stroke_col))
                    .corner_radius(CornerRadius::same(2))
                    .min_size(vec2(17.0, 14.0));
                    let resp = ui
                        .add_enabled(picked_hero.is_some(), btn)
                        .on_disabled_hover_text(&hover)
                        .on_hover_text(hover);
                    ally_role_buttons[idx] = resp.rect;
                    if resp.clicked()
                        && let Some(hid) = picked_hero
                    {
                        clicked_ally_hero = Some(hid);
                    }
                }
            });

            if inline_starting {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.render_overlay_starting_icons(ui, start_items, true, textures);
                });
            }
        });

        let Some(build) = &summary.build else {
            ui.add_space(4.0);
            super::surfaces::overlay_scrim_label(
                ui,
                tr.overlay_pick_ally_hint,
                9.5,
                false,
                P_SILVER,
            );
            return OverlayBuildOutput {
                clicked_variant: None,
                clicked_ally_hero,
                ally_role_buttons,
                phases_rect: None,
            };
        };

        let avail_w = ui.available_width();
        let rem_h_after_hdr = (ui.available_height() - 4.0).max(32.0);
        if !inline_starting && !start_items.is_empty() && rem_h_after_hdr >= 80.0 {
            ui.add_space(1.0);
            ui.horizontal(|ui| {
                self.render_overlay_starting_icons(ui, start_items, false, textures);
            });
        }
        ui.add_space(3.0);

        let avail_h = (ui.available_height() - 2.0).max(30.0);
        let show_opt_row = !build.optional_items.is_empty() && avail_h >= 60.0;
        let opt_reserve_h = if show_opt_row {
            (avail_h * 0.34).clamp(28.0, 38.0)
        } else {
            0.0
        };
        let opt_gap = if show_opt_row { 2.0 } else { 0.0 };
        let main_h = (avail_h - opt_reserve_h - opt_gap).max(28.0);

        let show_variants = main_h >= 54.0;
        let early_steps: Vec<(usize, &BuildStep)> = build
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.phase == ItemPhase::Early)
            .collect();
        let core_steps: Vec<(usize, &BuildStep)> = build
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.phase == ItemPhase::Core)
            .collect();
        let late_steps: Vec<(usize, &BuildStep)> = build
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.phase == ItemPhase::Late)
            .collect();

        let late_is_2x2 = select_overlay_late_grid_2x2(avail_w, main_h, late_steps.len());
        let late_cols = if late_is_2x2 {
            2
        } else {
            late_steps.len().max(1)
        };
        let step_cols = (early_steps.len().max(1) + core_steps.len().max(1) + late_cols) as f32;

        let single_row_budget = (main_h - 22.0).max(20.0);
        let late_row_budget = if late_is_2x2 {
            ((main_h - 25.0) * 0.5).max(20.0)
        } else {
            single_row_budget
        };
        let eff_row_budget = if late_is_2x2 {
            late_row_budget
        } else {
            single_row_budget
        };

        let width_tile = ((avail_w - 48.0 - step_cols * 6.0) / step_cols).floor();
        let height_tile = if show_variants {
            ((eff_row_budget - 18.0) / 0.73).floor()
        } else {
            (eff_row_budget / 0.73).floor()
        };
        let tile_w = width_tile.min(height_tile).clamp(24.0, 44.0);
        let tile_h = (tile_w * 0.73).round().clamp(18.0, 32.0);
        let variant_w = (tile_w * 0.50).round().clamp(15.0, 22.0);
        let variant_h = (variant_w * 0.72).round().clamp(11.0, 15.0);

        let single_var_rows = if show_variants {
            max_overlay_variant_rows(single_row_budget, tile_h, variant_h)
        } else {
            0
        };
        let late_var_rows = if show_variants {
            max_overlay_variant_rows(late_row_budget, tile_h, variant_h)
        } else {
            0
        };

        struct OverlayPhaseSpec<'a> {
            label: &'a str,
            steps: &'a [(usize, &'a BuildStep)],
            grid_2x2: bool,
            var_rows: usize,
        }

        let phase_label_w = |lbl: &str| {
            ui.painter()
                .layout_no_wrap(lbl.to_string(), FontId::proportional(7.5), P_CYAN)
                .size()
                .x
        };
        let row_w = estimate_overlay_build_row_width(
            phase_label_w(tr.phase_early),
            &early_steps,
            phase_label_w(tr.phase_core),
            &core_steps,
            phase_label_w(tr.phase_late),
            &late_steps,
            late_is_2x2,
            tile_w,
            variant_w,
            single_var_rows,
            late_var_rows,
        );
        let phase_lead = super::overlay::centered_leading_space(avail_w, row_w);

        ui.allocate_ui_with_layout(vec2(avail_w, main_h), Layout::top_down(Align::Min), |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("truesight_overlay_build_scroll")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .auto_shrink([false, false])
                .animated(false)
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if phase_lead > 0.0 {
                            ui.add_space(phase_lead);
                        }
                        let strip_resp = ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            let phases = [
                                OverlayPhaseSpec {
                                    label: tr.phase_early,
                                    steps: &early_steps,
                                    grid_2x2: false,
                                    var_rows: single_var_rows,
                                },
                                OverlayPhaseSpec {
                                    label: tr.phase_core,
                                    steps: &core_steps,
                                    grid_2x2: false,
                                    var_rows: single_var_rows,
                                },
                                OverlayPhaseSpec {
                                    label: tr.phase_late,
                                    steps: &late_steps,
                                    grid_2x2: late_is_2x2,
                                    var_rows: late_var_rows,
                                },
                            ];
                            for (p_idx, spec) in phases.into_iter().enumerate() {
                                if p_idx > 0 {
                                    ui.vertical(|ui| {
                                        ui.add_space((10.0 + tile_h * 0.40).max(12.0));
                                        let (cr, _) =
                                            ui.allocate_exact_size(vec2(8.0, 12.0), Sense::hover());
                                        paint_chevron_right(ui.painter(), cr, P_GEM);
                                    });
                                }
                                self.render_overlay_phase_box(
                                    ui,
                                    spec.label,
                                    spec.steps,
                                    build.next_unowned_index,
                                    tile_w,
                                    tile_h,
                                    variant_w,
                                    variant_h,
                                    spec.var_rows,
                                    spec.grid_2x2,
                                    textures,
                                    &mut clicked_variant,
                                );
                            }
                        });
                        phases_rect = Some(strip_resp.response.rect);
                    });
                });
        });

        if show_opt_row {
            ui.add_space(opt_gap);
            self.render_overlay_optional_row(
                ui,
                tr,
                &build.optional_items,
                avail_w,
                tile_w,
                opt_reserve_h,
                textures,
            );
        }

        OverlayBuildOutput {
            clicked_variant,
            clicked_ally_hero,
            ally_role_buttons,
            phases_rect,
        }
    }

    fn render_overlay_starting_icons(
        &self,
        ui: &mut egui::Ui,
        start_items: &[ItemId],
        right_to_left: bool,
        textures: &GuiTextureCache,
    ) {
        let paint_chip = |ui: &mut egui::Ui, item_id: ItemId| {
            let owned = self.session.active_build_owned_items().contains(&item_id);
            let (name, cost) = self
                .session
                .item_repo
                .item_catalog(item_id)
                .map(|e| (e.display_name, e.cost))
                .unwrap_or_else(|| (format!("#{}", item_id.0), 0));
            let (r, resp) = ui.allocate_exact_size(vec2(19.0, 14.0), Sense::hover());
            let border = if owned { P_GEM_DEEP } else { P_LINE };
            paint_item_icon_with_fallback(
                ui.painter(),
                r,
                textures.items.get(&item_id),
                Some(name.as_str()),
                border,
                owned,
            );
            resp.on_hover_text(format!("{name} ({cost}g) · START"));
        };

        ui.spacing_mut().item_spacing.x = 2.0;
        if right_to_left {
            for &id in start_items.iter().take(5).rev() {
                paint_chip(ui, id);
            }
            ui.add_space(2.0);
            super::surfaces::overlay_scrim_label(ui, "START", 7.5, true, P_SILVER);
        } else {
            super::surfaces::overlay_scrim_label(ui, "START", 7.5, true, P_SILVER);
            for &id in start_items.iter().take(5) {
                paint_chip(ui, id);
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn render_overlay_step_tile(
        &self,
        ui: &mut egui::Ui,
        idx: usize,
        step: &BuildStep,
        next_unowned_index: Option<usize>,
        tile_w: f32,
        tile_h: f32,
        variant_w: f32,
        variant_h: f32,
        var_rows: usize,
        textures: &GuiTextureCache,
        clicked_variant: &mut Option<(ItemId, ItemId)>,
    ) {
        let is_next = next_unowned_index == Some(idx);
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
        let step_tex = textures.items.get(&step.item_id);

        let step_resp = ui
            .vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let (rect, resp) = ui.allocate_exact_size(vec2(tile_w, tile_h), Sense::hover());
                paint_item_icon_with_fallback(
                    ui.painter(),
                    rect,
                    step_tex,
                    Some(name.as_str()),
                    Color32::TRANSPARENT,
                    step.owned,
                );
                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::same(3),
                    Stroke::new(if is_next { 1.5_f32 } else { 1.0_f32 }, border_col),
                    egui::StrokeKind::Inside,
                );
                if step.owned {
                    paint_corner_pill(
                        ui.painter(),
                        rect.min + vec2(1.0, 1.0),
                        Align2::LEFT_TOP,
                        "OK",
                        P_VOID,
                        P_GEM,
                        6.5,
                    );
                } else if is_next {
                    paint_corner_pill(
                        ui.painter(),
                        rect.min + vec2(1.0, 1.0),
                        Align2::LEFT_TOP,
                        "NEXT",
                        P_VOID,
                        P_GEM,
                        6.5,
                    );
                } else if step.owned_components > 0 {
                    paint_corner_pill(
                        ui.painter(),
                        rect.min + vec2(1.0, 1.0),
                        Align2::LEFT_TOP,
                        &format!("{}/{}", step.owned_components, step.total_components),
                        P_VOID,
                        P_CYAN,
                        6.5,
                    );
                }

                let timing_txt = format!("{}m", step.avg_time_mins.round().max(1.0) as u32);
                paint_corner_pill(
                    ui.painter(),
                    rect.max - vec2(1.0, 1.0),
                    Align2::RIGHT_BOTTOM,
                    &timing_txt,
                    P_GLINT,
                    P_VOID.gamma_multiply(0.84),
                    7.0,
                );

                let trait_note = step
                    .counter_trait
                    .map(|t| format!(" · CNT: {}", t.label()))
                    .unwrap_or_default();
                let owned_note = if step.owned { " · OWNED" } else { "" };
                resp.on_hover_text(format!(
                    "{name} ({cost}g) · ~{:.0}m · {}{}{}",
                    step.avg_time_mins,
                    phase_short(step.phase),
                    trait_note,
                    owned_note
                ));

                if var_rows > 0 && !step.variants.is_empty() {
                    let owned_items = self.session.active_build_owned_items();
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for var_chunk in step.variants.chunks(2).take(var_rows) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                for var in var_chunk {
                                    let var_entry =
                                        self.session.item_repo.item_catalog(var.item_id);
                                    let is_direct_upgrade = var_entry
                                        .as_ref()
                                        .is_some_and(|e| e.components.contains(&step.item_id));
                                    let (vname, vcost) = var_entry
                                        .map(|e| (e.display_name, e.cost))
                                        .unwrap_or_else(|| (format!("#{}", var.item_id.0), 0));
                                    let var_owned = owned_items.contains(&var.item_id);
                                    let upgrade_ready =
                                        is_direct_upgrade && step.owned && !var_owned;
                                    let sense = if is_direct_upgrade {
                                        Sense::hover()
                                    } else {
                                        Sense::click()
                                    };
                                    let (vr, vresp) =
                                        ui.allocate_exact_size(vec2(variant_w, variant_h), sense);
                                    let vborder = if upgrade_ready {
                                        P_CYAN
                                    } else if !is_direct_upgrade && vresp.hovered() {
                                        P_GEM
                                    } else {
                                        P_LINE
                                    };
                                    paint_item_icon_with_fallback(
                                        ui.painter(),
                                        vr,
                                        textures.items.get(&var.item_id),
                                        Some(vname.as_str()),
                                        vborder,
                                        var_owned,
                                    );
                                    if is_direct_upgrade && variant_w >= 18.0 {
                                        let (fg, bg) = if upgrade_ready {
                                            (P_VOID, P_CYAN)
                                        } else {
                                            (P_GLINT, P_VOID.gamma_multiply(0.84))
                                        };
                                        paint_corner_pill(
                                            ui.painter(),
                                            vr.min + vec2(1.0, 1.0),
                                            Align2::LEFT_TOP,
                                            "UPG",
                                            fg,
                                            bg,
                                            5.5,
                                        );
                                    }
                                    let trait_note = var
                                        .counter_trait
                                        .map(|t| format!(" · CNT: {}", t.label()))
                                        .unwrap_or_default();
                                    let upg_note = if upgrade_ready {
                                        " · UPGRADE AVAILABLE"
                                    } else if is_direct_upgrade {
                                        " · UPGRADE"
                                    } else {
                                        ""
                                    };
                                    let vresp = vresp.on_hover_text(format!(
                                        "{vname} ({vcost}g) · ~{:.0}m{trait_note}{upg_note}",
                                        var.avg_time_mins
                                    ));
                                    if !is_direct_upgrade && vresp.clicked() {
                                        *clicked_variant = Some((step.item_id, var.item_id));
                                    }
                                }
                            });
                        }
                    });
                }
            })
            .response;

        if is_next {
            step_resp.scroll_to_me(Some(Align::Center));
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn render_overlay_phase_box(
        &self,
        ui: &mut egui::Ui,
        phase_label: &str,
        steps: &[(usize, &BuildStep)],
        next_unowned_index: Option<usize>,
        tile_w: f32,
        tile_h: f32,
        variant_w: f32,
        variant_h: f32,
        var_rows: usize,
        grid_2x2: bool,
        textures: &GuiTextureCache,
        clicked_variant: &mut Option<(ItemId, ItemId)>,
    ) {
        egui::Frame::NONE
            .fill(Color32::from_rgba_unmultiplied(10, 9, 24, 38))
            .stroke(Stroke::NONE)
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(Margin::symmetric(4, 3))
            .show(ui, |ui| {
                // Explicit vertical (top_down Min) layout avoids inheriting `horizontal_top`
                // while keeping parent available width instead of collapsing `max_rect.width` to 0.
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(
                        egui::Label::new(
                            RichText::new(phase_label).size(7.5).strong().color(P_CYAN),
                        )
                        .extend(),
                    );
                    if steps.is_empty() {
                        let (r, _) =
                            ui.allocate_exact_size(vec2(tile_w, tile_h + 2.0), Sense::hover());
                        ui.painter().text(
                            r.center(),
                            Align2::CENTER_CENTER,
                            "—",
                            FontId::proportional(9.5),
                            P_MUTED,
                        );
                        return;
                    }
                    if grid_2x2 && steps.len() >= 3 {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            for row_steps in steps.chunks(2) {
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 3.0;
                                    for &(idx, step) in row_steps {
                                        self.render_overlay_step_tile(
                                            ui,
                                            idx,
                                            step,
                                            next_unowned_index,
                                            tile_w,
                                            tile_h,
                                            variant_w,
                                            variant_h,
                                            var_rows,
                                            textures,
                                            clicked_variant,
                                        );
                                    }
                                });
                            }
                        });
                    } else {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            for &(idx, step) in steps {
                                self.render_overlay_step_tile(
                                    ui,
                                    idx,
                                    step,
                                    next_unowned_index,
                                    tile_w,
                                    tile_h,
                                    variant_w,
                                    variant_h,
                                    var_rows,
                                    textures,
                                    clicked_variant,
                                );
                            }
                        });
                    }
                });
            });
    }

    #[expect(clippy::too_many_arguments)]
    fn render_overlay_optional_row(
        &self,
        ui: &mut egui::Ui,
        _tr: &UiStrings,
        optional_items: &[OptionalBuildItem],
        avail_w: f32,
        tile_w: f32,
        reserve_h: f32,
        textures: &GuiTextureCache,
    ) {
        let opt_h = (reserve_h - 6.0).clamp(20.0, 31.0);
        let opt_w = (opt_h / 0.73)
            .round()
            .max(tile_w.min(42.0))
            .clamp(28.0, 42.0);
        let opt_h = (opt_w * 0.73).round().min(opt_h).clamp(20.0, 31.0);
        let badge_label = if avail_w >= 360.0 {
            "OPT · CNT"
        } else {
            "OPT"
        };
        let inner_w = (avail_w - 12.0).max(80.0);

        egui::Frame::NONE
            .fill(Color32::from_rgba_unmultiplied(10, 9, 24, 58))
            .stroke(Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(224, 194, 96, 130),
            ))
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(Margin::symmetric(5, 2))
            .show(ui, |ui| {
                ui.set_min_width(inner_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.add(
                        egui::Label::new(
                            RichText::new(badge_label)
                                .size(8.5)
                                .strong()
                                .color(P_NEUTRAL),
                        )
                        .extend(),
                    );
                    for opt in optional_items.iter().take(6) {
                        let (name, cost) = self
                            .session
                            .item_repo
                            .item_catalog(opt.item_id)
                            .map(|e| (e.display_name, e.cost))
                            .unwrap_or_else(|| (format!("#{}", opt.item_id.0), 0));
                        let (r, resp) = ui.allocate_exact_size(vec2(opt_w, opt_h), Sense::hover());
                        let border = if opt.owned {
                            P_GEM_DEEP
                        } else if resp.hovered() {
                            P_GEM
                        } else {
                            P_NEUTRAL
                        };
                        paint_item_icon_with_fallback(
                            ui.painter(),
                            r,
                            textures.items.get(&opt.item_id),
                            Some(name.as_str()),
                            Color32::TRANSPARENT,
                            opt.owned,
                        );
                        ui.painter().rect_stroke(
                            r,
                            egui::CornerRadius::same(3),
                            Stroke::new(1.5_f32, border),
                            egui::StrokeKind::Inside,
                        );
                        if opt.owned {
                            paint_corner_pill(
                                ui.painter(),
                                r.min + vec2(1.0, 1.0),
                                Align2::LEFT_TOP,
                                "OK",
                                P_VOID,
                                P_GEM,
                                6.5,
                            );
                        }
                        let timing_txt = format!("{}m", opt.avg_time_mins.round().max(1.0) as u32);
                        paint_corner_pill(
                            ui.painter(),
                            r.max - vec2(1.0, 1.0),
                            Align2::RIGHT_BOTTOM,
                            &timing_txt,
                            P_GLINT,
                            P_VOID.gamma_multiply(0.84),
                            7.0,
                        );
                        let reason = opt
                            .counter_trait
                            .map(|t| format!(" · CNT: {}", t.label()))
                            .unwrap_or_default();
                        let owned_note = if opt.owned { " · OWNED" } else { "" };
                        resp.on_hover_text(format!(
                            "{name} ({cost}g) · ~{:.0}m{reason}{owned_note}",
                            opt.avg_time_mins
                        ));
                    }
                });
            });
    }
}
