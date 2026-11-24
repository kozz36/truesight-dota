use super::TrueSightGuiApp;
use super::i18n::UiStrings;
use super::theme::*;
use eframe::egui::{self, Align, Layout, Margin, Rect, RichText, Sense, Stroke, vec2};
use truesight_core::domain::{
    BuildStep, ItemId, ItemPhase, OptionalBuildItem, counter_trait_table_patch,
};
use truesight_core::ports::ItemMetaRepository;

pub(super) struct BuildBarRenderOutput {
    pub(super) card: Rect,
}

pub(super) fn variant_chip_label_and_highlight(
    var_name: &str,
    is_direct_upgrade: bool,
    parent_owned: bool,
    var_owned: bool,
) -> (String, bool) {
    if !is_direct_upgrade {
        (var_name.to_string(), false)
    } else if parent_owned && !var_owned {
        (format!("UPG READY · {var_name}"), true)
    } else {
        (format!("UPG · {var_name}"), false)
    }
}

fn format_game_clock(secs: i32) -> String {
    let abs = secs.unsigned_abs();
    let mins = abs / 60;
    let rem = abs % 60;
    if secs < 0 {
        format!("-{mins:02}:{rem:02}")
    } else {
        format!("{mins:02}:{rem:02}")
    }
}

fn format_phase_timing_window(steps: &[(usize, &BuildStep)]) -> String {
    if steps.is_empty() {
        return "—".to_string();
    }
    let mut min_t = f32::INFINITY;
    let mut max_t = f32::NEG_INFINITY;
    for &(_, step) in steps {
        let t = step.avg_time_mins.round().max(1.0);
        min_t = min_t.min(t);
        max_t = max_t.max(t);
    }
    let lo = min_t as u32;
    let hi = max_t as u32;
    if lo == hi {
        format!("~{lo}m")
    } else {
        format!("~{lo}-{hi}m")
    }
}

impl TrueSightGuiApp {
    fn render_build_step_card(
        &self,
        ui: &mut egui::Ui,
        step: &BuildStep,
        is_next: bool,
        clicked_variant: &mut Option<(ItemId, ItemId)>,
    ) {
        let border_color = if step.owned {
            P_GEM_DEEP
        } else if is_next {
            P_GEM
        } else {
            P_LINE
        };
        let fill_color = if step.owned { P_INDIGO_1 } else { P_INDIGO_2 };
        let (name, cost) = self
            .session
            .item_repo
            .item_catalog(step.item_id)
            .map(|e| (e.display_name, e.cost))
            .unwrap_or_else(|| (format!("Item #{}", step.item_id.0), 0));
        let step_tex = self.textures.items.get(&step.item_id);

        egui::Frame::NONE
            .fill(fill_color)
            .stroke(Stroke::new(
                if is_next { 1.5_f32 } else { 1.0_f32 },
                border_color,
            ))
            .inner_margin(Margin::symmetric(5, 3))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("~{:.0}m", step.avg_time_mins))
                            .size(8.5)
                            .color(P_MUTED),
                    );
                    if step.owned {
                        paint_mini_badge_scaled(ui, "OK", P_GEM, P_GEM_DEEP, 8.0, 3);
                    } else if is_next {
                        paint_mini_badge_scaled(ui, "NEXT", P_GEM, P_GEM, 8.0, 3);
                    } else if step.owned_components > 0 {
                        paint_mini_badge_scaled(
                            ui,
                            &format!("{}/{}", step.owned_components, step.total_components),
                            P_CYAN,
                            P_LINE,
                            8.0,
                            3,
                        );
                    }
                    if let Some(tr) = step.counter_trait {
                        ui.label(
                            RichText::new(format!("CNT: {}", tr.label()))
                                .size(8.0)
                                .strong()
                                .color(P_NEUTRAL),
                        );
                    }
                });

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    paint_item_icon_named(
                        ui,
                        step_tex,
                        Some(name.as_str()),
                        vec2(32.0, 24.0),
                        border_color,
                        step.owned,
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.add(
                            egui::Label::new(
                                RichText::new(&name)
                                    .size(10.0)
                                    .strong()
                                    .color(if step.owned { P_MUTED } else { P_GLINT }),
                            )
                            .truncate(),
                        );
                        ui.label(RichText::new(format!("{cost}g")).size(9.0).color(P_MUTED));
                    });
                });
            });

        if !step.variants.is_empty() {
            let owned_items = self.session.active_build_owned_items();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 3.0);
                for var in &step.variants {
                    let var_entry = self.session.item_repo.item_catalog(var.item_id);
                    let is_direct_upgrade = var_entry
                        .as_ref()
                        .is_some_and(|e| e.components.contains(&step.item_id));
                    let var_name = var_entry
                        .map(|e| e.display_name)
                        .unwrap_or_else(|| format!("Item #{}", var.item_id.0));
                    let var_owned = owned_items.contains(&var.item_id);
                    let (chip_label, highlight) = variant_chip_label_and_highlight(
                        &var_name,
                        is_direct_upgrade,
                        step.owned,
                        var_owned,
                    );
                    let var_img = self
                        .textures
                        .items
                        .get(&var.item_id)
                        .map(|t| egui::Image::new((t.id(), vec2(18.0, 13.0))));
                    let chip_stroke = if highlight {
                        Stroke::new(1.5_f32, P_CYAN)
                    } else {
                        Stroke::new(1.0_f32, P_LINE)
                    };
                    let chip_text_col = if highlight { P_GLINT } else { P_SILVER };
                    let var_btn = egui::Button::opt_image_and_text(
                        var_img,
                        Some(
                            RichText::new(chip_label)
                                .size(9.0)
                                .color(chip_text_col)
                                .into(),
                        ),
                    )
                    .fill(P_INDIGO_2)
                    .stroke(chip_stroke)
                    .sense(if is_direct_upgrade {
                        Sense::hover()
                    } else {
                        Sense::click()
                    });
                    let trait_note = var
                        .counter_trait
                        .map(|t| format!(" | CNT: {}", t.label()))
                        .unwrap_or_default();
                    let hover = if highlight {
                        format!(
                            "Upgrade available ({var_name}) · ~{:.0}m{trait_note}",
                            var.avg_time_mins
                        )
                    } else if is_direct_upgrade {
                        format!(
                            "Upgrade ({var_name}, score {:.2}, ~{:.0}m{trait_note})",
                            var.score, var.avg_time_mins
                        )
                    } else {
                        format!(
                            "Swap into primary slot (score {:.2}, ~{:.0}m{trait_note})",
                            var.score, var.avg_time_mins
                        )
                    };
                    let var_resp = ui.add(var_btn).on_hover_text(hover);
                    if !is_direct_upgrade && var_resp.clicked() {
                        *clicked_variant = Some((step.item_id, var.item_id));
                    }
                }
            });
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn render_phase_column(
        &self,
        ui: &mut egui::Ui,
        phase_title: &str,
        col_w: f32,
        steps: &[(usize, &BuildStep)],
        next_unowned_index: Option<usize>,
        two_col_grid: bool,
        clicked_variant: &mut Option<(ItemId, ItemId)>,
    ) {
        let timing_summary = format_phase_timing_window(steps);
        let inner_w = (col_w - 10.0).max(80.0);
        egui::Frame::NONE
            .fill(P_INDIGO_1)
            .stroke(Stroke::new(1.0_f32, P_LINE))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    vec2(inner_w, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_min_width(inner_w);
                        ui.set_max_width(inner_w);

                        ui.horizontal(|ui| {
                            ui.label(RichText::new(phase_title).size(9.5).strong().color(P_CYAN));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new(timing_summary).size(9.0).color(P_MUTED));
                            });
                        });
                        ui.add_space(2.0);

                        if steps.is_empty() {
                            ui.label(RichText::new("—").size(9.5).color(P_MUTED));
                            return;
                        }

                        if two_col_grid && steps.len() >= 3 && inner_w >= 210.0 {
                            let tile_gap = 6.0_f32;
                            let cell_w = ((inner_w - tile_gap) / 2.0).floor().max(95.0);
                            for row in steps.chunks(2) {
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = tile_gap;
                                    for &(idx, step) in row {
                                        ui.allocate_ui_with_layout(
                                            vec2(cell_w, 0.0),
                                            Layout::top_down(Align::Min),
                                            |ui| {
                                                ui.set_min_width(cell_w);
                                                ui.set_max_width(cell_w);
                                                self.render_build_step_card(
                                                    ui,
                                                    step,
                                                    next_unowned_index == Some(idx),
                                                    clicked_variant,
                                                );
                                            },
                                        );
                                    }
                                });
                                ui.add_space(4.0);
                            }
                        } else {
                            for &(idx, step) in steps {
                                self.render_build_step_card(
                                    ui,
                                    step,
                                    next_unowned_index == Some(idx),
                                    clicked_variant,
                                );
                                ui.add_space(4.0);
                            }
                        }
                    },
                );
            });
    }

    fn render_optional_counters_section(
        &self,
        ui: &mut egui::Ui,
        tr: &UiStrings,
        optional_items: &[OptionalBuildItem],
        section_w: f32,
    ) {
        let inner_w = (section_w - 14.0).max(120.0);
        egui::Frame::NONE
            .fill(P_INDIGO_1)
            .stroke(Stroke::new(1.0_f32, P_LINE))
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    vec2(inner_w, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_min_width(inner_w);
                        ui.set_max_width(inner_w);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(tr.optional_items_label)
                                    .size(9.5)
                                    .strong()
                                    .color(P_NEUTRAL),
                            );
                        });
                        ui.add_space(4.0);

                        if optional_items.is_empty() {
                            ui.label(
                                RichText::new(tr.optional_items_empty)
                                    .size(9.5)
                                    .color(P_MUTED),
                            );
                            return;
                        }

                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
                            for opt in optional_items {
                                let (name, cost) = self
                                    .session
                                    .item_repo
                                    .item_catalog(opt.item_id)
                                    .map(|e| (e.display_name, e.cost))
                                    .unwrap_or_else(|| (format!("Item #{}", opt.item_id.0), 0));
                                let opt_tex = self.textures.items.get(&opt.item_id);
                                let border = if opt.owned { P_GEM_DEEP } else { P_NEUTRAL };
                                let chip_resp = egui::Frame::NONE
                                    .fill(P_INDIGO_2)
                                    .stroke(Stroke::new(1.0_f32, border))
                                    .inner_margin(Margin::symmetric(6, 4))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 5.0;
                                            paint_item_icon_named(
                                                ui,
                                                opt_tex,
                                                Some(name.as_str()),
                                                vec2(28.0, 20.0),
                                                border,
                                                opt.owned,
                                            );
                                            ui.vertical(|ui| {
                                                ui.spacing_mut().item_spacing.y = 0.0;
                                                ui.horizontal(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 4.0;
                                                    ui.label(
                                                        RichText::new(&name)
                                                            .size(9.5)
                                                            .strong()
                                                            .color(if opt.owned {
                                                                P_MUTED
                                                            } else {
                                                                P_GLINT
                                                            }),
                                                    );
                                                    if opt.owned {
                                                        paint_mini_badge_scaled(
                                                            ui, "OK", P_GEM, P_GEM_DEEP, 7.5, 2,
                                                        );
                                                    }
                                                });
                                                let reason = opt
                                                    .counter_trait
                                                    .map(|t| format!("CNT: {}", t.label()))
                                                    .unwrap_or_default();
                                                let sub = if reason.is_empty() {
                                                    format!("{cost}g · ~{:.0}m", opt.avg_time_mins)
                                                } else {
                                                    format!("{reason} · ~{:.0}m", opt.avg_time_mins)
                                                };
                                                ui.label(RichText::new(sub).size(8.5).color(
                                                    if opt.owned { P_MUTED } else { P_NEUTRAL },
                                                ));
                                            });
                                        });
                                    })
                                    .response;
                                let hover = match opt.counter_trait {
                                    Some(ct) => format!(
                                        "{name} ({cost}g) · ~{:.0}m · Counter vs enemy {}",
                                        opt.avg_time_mins,
                                        ct.label()
                                    ),
                                    None => {
                                        format!("{name} ({cost}g) · ~{:.0}m", opt.avg_time_mins)
                                    }
                                };
                                let _ = ui
                                    .interact(
                                        chip_resp.rect,
                                        ui.id().with(("opt_chip", opt.item_id.0)),
                                        Sense::hover(),
                                    )
                                    .on_hover_text(hover);
                            }
                        });
                    },
                );
            });
    }

    /// Renders the post-draft item build card with `INICIALES`, three responsive timed phase
    /// columns (`EARLY`, `CORE`, `LATE`), and a full-width role-filtered `OPTIONAL / COUNTERS` section.
    pub(super) fn render_item_build_card(&mut self, ui: &mut egui::Ui) -> BuildBarRenderOutput {
        let tr = super::i18n::strings(self.session.ui_language);
        let mut clicked_variant: Option<(ItemId, ItemId)> = None;
        let mut switch_to_picks = false;

        let card_rect = hud_card_fill(ui, |ui| {
            let build_opt = self.session.compute_recommended_build();
            let hero_header = if let Some((hero_id, role)) = self.session.active_build_hero() {
                let hero_name = self
                    .session
                    .matrix
                    .hero(hero_id)
                    .map(|h| h.display_name.to_ascii_uppercase())
                    .unwrap_or_else(|| format!("HERO #{}", hero_id.0));
                format!(
                    "{} · {hero_name} ({})",
                    tr.item_build_header,
                    role.label().to_ascii_uppercase()
                )
            } else {
                tr.item_build_post_draft_fallback.to_string()
            };

            // Header
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(
                    RichText::new(hero_header)
                        .size(11.0)
                        .strong()
                        .color(P_GLINT),
                );

                ui.label(
                    RichText::new(format!(
                        "T = {}",
                        format_game_clock(self.session.game_time_secs)
                    ))
                    .size(10.0)
                    .strong()
                    .color(P_CYAN),
                );

                if self.session.is_counter_table_patch_stale() {
                    ui.label(
                        RichText::new(format!(
                            "PATCH {} TABLE (ACTIVE: {})",
                            counter_trait_table_patch(),
                            self.session.active_game_patch
                        ))
                        .size(9.5)
                        .strong()
                        .color(P_NEUTRAL),
                    );
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let toggle_btn = egui::Button::new(
                        RichText::new(tr.picks_view_btn)
                            .size(10.0)
                            .strong()
                            .color(P_GLINT),
                    )
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_LINE));
                    if ui
                        .add(toggle_btn)
                        .on_hover_text(tr.picks_view_hover)
                        .clicked()
                    {
                        switch_to_picks = true;
                    }
                });
            });

            ui.add_space(6.0);

            let Some(build) = build_opt else {
                ui.add_space(24.0);
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new(tr.item_build_empty_hint)
                            .size(11.0)
                            .color(P_MUTED),
                    );
                });
                return;
            };

            egui::ScrollArea::vertical()
                .id_salt("truesight_item_build_body_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let avail_w = ui.available_width();

                    // 1. Starting items strip
                    if !build.starting_items.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                            ui.label(
                                RichText::new(tr.starting_items_label)
                                    .size(10.0)
                                    .strong()
                                    .color(P_MUTED),
                            );
                            for &item_id in &build.starting_items {
                                let (name, cost) = self
                                    .session
                                    .item_repo
                                    .item_catalog(item_id)
                                    .map(|e| (e.display_name, e.cost))
                                    .unwrap_or_else(|| (format!("Item #{}", item_id.0), 0));
                                let tex = self.textures.items.get(&item_id);
                                egui::Frame::NONE
                                    .fill(P_INDIGO_2)
                                    .stroke(Stroke::new(1.0_f32, P_LINE))
                                    .inner_margin(Margin::symmetric(5, 2))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            paint_item_icon_named(
                                                ui,
                                                tex,
                                                Some(name.as_str()),
                                                vec2(22.0, 16.0),
                                                P_LINE,
                                                false,
                                            );
                                            ui.label(
                                                RichText::new(format!("{name} · {cost}g"))
                                                    .size(9.5)
                                                    .color(P_SILVER),
                                            );
                                        });
                                    });
                            }
                        });
                        ui.add_space(6.0);
                    }

                    // 2. Three responsive timed phase columns: EARLY | CORE | LATE
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

                    let col_gap = 6.0_f32;
                    let use_late_2x2 = late_steps.len() >= 3;
                    if avail_w >= 520.0 {
                        let usable_w = (avail_w - 2.0 * col_gap - 4.0).max(360.0);
                        let (early_w, core_w, late_w) = if use_late_2x2 && avail_w >= 560.0 {
                            let ew = (usable_w * 0.24).floor().max(110.0);
                            let cw = (usable_w * 0.26).floor().max(115.0);
                            let lw = (usable_w - ew - cw).floor().max(220.0);
                            (ew, cw, lw)
                        } else {
                            let eq = (usable_w / 3.0).floor().max(120.0);
                            (eq, eq, eq)
                        };
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = col_gap;
                            self.render_phase_column(
                                ui,
                                tr.phase_early,
                                early_w,
                                &early_steps,
                                build.next_unowned_index,
                                false,
                                &mut clicked_variant,
                            );
                            self.render_phase_column(
                                ui,
                                tr.phase_core,
                                core_w,
                                &core_steps,
                                build.next_unowned_index,
                                false,
                                &mut clicked_variant,
                            );
                            self.render_phase_column(
                                ui,
                                tr.phase_late,
                                late_w,
                                &late_steps,
                                build.next_unowned_index,
                                use_late_2x2,
                                &mut clicked_variant,
                            );
                        });
                    } else if avail_w >= 340.0 {
                        let two_w = ((avail_w - col_gap - 4.0) / 2.0).floor().max(120.0);
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = col_gap;
                            self.render_phase_column(
                                ui,
                                tr.phase_early,
                                two_w,
                                &early_steps,
                                build.next_unowned_index,
                                false,
                                &mut clicked_variant,
                            );
                            self.render_phase_column(
                                ui,
                                tr.phase_core,
                                two_w,
                                &core_steps,
                                build.next_unowned_index,
                                false,
                                &mut clicked_variant,
                            );
                        });
                        ui.add_space(6.0);
                        self.render_phase_column(
                            ui,
                            tr.phase_late,
                            avail_w,
                            &late_steps,
                            build.next_unowned_index,
                            use_late_2x2,
                            &mut clicked_variant,
                        );
                    } else {
                        self.render_phase_column(
                            ui,
                            tr.phase_early,
                            avail_w,
                            &early_steps,
                            build.next_unowned_index,
                            false,
                            &mut clicked_variant,
                        );
                        ui.add_space(4.0);
                        self.render_phase_column(
                            ui,
                            tr.phase_core,
                            avail_w,
                            &core_steps,
                            build.next_unowned_index,
                            false,
                            &mut clicked_variant,
                        );
                        ui.add_space(4.0);
                        self.render_phase_column(
                            ui,
                            tr.phase_late,
                            avail_w,
                            &late_steps,
                            build.next_unowned_index,
                            use_late_2x2,
                            &mut clicked_variant,
                        );
                    }

                    ui.add_space(6.0);

                    // 3. Full-width bottom Optional / Counters section
                    self.render_optional_counters_section(ui, tr, &build.optional_items, avail_w);
                });
        });

        if switch_to_picks {
            self.session.toggle_post_draft_build_view();
        }
        if let Some((primary_id, var_id)) = clicked_variant {
            self.session
                .select_build_variant_for_item(primary_id, var_id);
        }

        BuildBarRenderOutput { card: card_rect }
    }
}
