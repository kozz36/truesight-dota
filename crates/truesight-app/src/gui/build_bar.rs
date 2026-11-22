use super::TrueSightGuiApp;
use super::theme::*;
use eframe::egui::{self, Align, Layout, Margin, Rect, RichText, Sense, Stroke, vec2};
use truesight_core::domain::{ItemPhase, counter_trait_table_patch};
use truesight_core::ports::ItemMetaRepository;

pub(super) struct BuildBarRenderOutput {
    pub(super) card: Rect,
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

fn phase_label(phase: ItemPhase) -> &'static str {
    match phase {
        ItemPhase::Starting => "START",
        ItemPhase::Early => "EARLY",
        ItemPhase::Core => "CORE",
        ItemPhase::Late => "LATE",
        ItemPhase::Situational => "SIT",
    }
}

impl TrueSightGuiApp {
    pub(super) fn render_item_build_card(&mut self, ui: &mut egui::Ui) -> BuildBarRenderOutput {
        let tr = super::i18n::strings(self.session.ui_language);
        let mut clicked_variant: Option<(
            truesight_core::domain::ItemId,
            truesight_core::domain::ItemId,
        )> = None;
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

            // Starting Items Row
            if !build.starting_items.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
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
                                    paint_item_icon(ui, tex, vec2(22.0, 16.0), P_LINE, false);
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

            // Sequential Build Bar: Step cards joined by crisp vector chevrons
            egui::ScrollArea::horizontal()
                .id_salt("truesight_item_build_bar_scroll")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (idx, step) in build.steps.iter().enumerate() {
                            if idx > 0 {
                                ui.vertical(|ui| {
                                    ui.add_space(28.0);
                                    let (chev_rect, _) =
                                        ui.allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
                                    paint_chevron_right(ui.painter(), chev_rect, P_GEM);
                                });
                            }

                            let is_next = build.next_unowned_index == Some(idx);
                            let border_color = if step.owned {
                                P_GEM_DEEP
                            } else if is_next {
                                P_GEM
                            } else {
                                P_LINE
                            };
                            let fill_color = if step.owned {
                                P_INDIGO_1
                            } else {
                                P_INDIGO_2
                            };

                            let (name, cost) = self
                                .session
                                .item_repo
                                .item_catalog(step.item_id)
                                .map(|e| (e.display_name, e.cost))
                                .unwrap_or_else(|| (format!("Item #{}", step.item_id.0), 0));
                            let step_tex = self.textures.items.get(&step.item_id);

                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                egui::Frame::NONE
                                    .fill(fill_color)
                                    .stroke(Stroke::new(
                                        if is_next { 1.5_f32 } else { 1.0_f32 },
                                        border_color,
                                    ))
                                    .inner_margin(Margin::symmetric(8, 6))
                                    .show(ui, |ui| {
                                        ui.set_min_width(142.0);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(phase_label(step.phase))
                                                    .size(9.0)
                                                    .strong()
                                                    .color(P_MUTED),
                                            );
                                            ui.label(
                                                RichText::new(format!("~{:.0}m", step.avg_time_mins))
                                                    .size(9.0)
                                                    .color(P_MUTED),
                                            );
                                            if step.owned {
                                                paint_mini_badge_scaled(
                                                    ui, "OK", P_GEM, P_GEM_DEEP, 8.0, 3,
                                                );
                                            } else if is_next {
                                                paint_mini_badge_scaled(
                                                    ui, "NEXT", P_GEM, P_GEM, 8.0, 3,
                                                );
                                            } else if step.owned_components > 0 {
                                                paint_mini_badge_scaled(
                                                    ui,
                                                    &format!(
                                                        "{}/{}",
                                                        step.owned_components, step.total_components
                                                    ),
                                                    P_CYAN,
                                                    P_LINE,
                                                    8.0,
                                                    3,
                                                );
                                            }
                                        });

                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 6.0;
                                            paint_item_icon(
                                                ui,
                                                step_tex,
                                                vec2(44.0, 32.0),
                                                border_color,
                                                step.owned,
                                            );
                                            ui.vertical(|ui| {
                                                ui.spacing_mut().item_spacing.y = 1.0;
                                                ui.label(
                                                    RichText::new(name)
                                                        .size(10.5)
                                                        .strong()
                                                        .color(if step.owned {
                                                            P_MUTED
                                                        } else {
                                                            P_GLINT
                                                        }),
                                                );
                                                ui.label(
                                                    RichText::new(format!("{cost}g"))
                                                        .size(9.5)
                                                        .color(P_MUTED),
                                                );
                                            });
                                        });

                                        if let Some(tr) = step.counter_trait {
                                            ui.label(
                                                RichText::new(format!("CNT: {}", tr.label()))
                                                    .size(9.0)
                                                    .strong()
                                                    .color(P_NEUTRAL),
                                            )
                                            .on_hover_text(format!(
                                                "Counter-promoted item for enemy {}",
                                                tr.label()
                                            ));
                                        }
                                    });

                                // Variants sub-row underneath step box
                                if !step.variants.is_empty() {
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        for var in &step.variants {
                                            let var_name = self
                                                .session
                                                .item_repo
                                                .item_catalog(var.item_id)
                                                .map(|e| e.display_name)
                                                .unwrap_or_else(|| format!("Item #{}", var.item_id.0));
                                            let var_img = self
                                                .textures
                                                .items
                                                .get(&var.item_id)
                                                .map(|t| egui::Image::new((t.id(), vec2(20.0, 15.0))));
                                            let var_btn = egui::Button::opt_image_and_text(
                                                var_img,
                                                Some(
                                                    RichText::new(var_name)
                                                        .size(9.5)
                                                        .color(P_SILVER)
                                                        .into(),
                                                ),
                                            )
                                            .fill(P_INDIGO_1)
                                            .stroke(Stroke::new(1.0_f32, P_LINE));
                                            let trait_note = var
                                                .counter_trait
                                                .map(|t| format!(" | CNT: {}", t.label()))
                                                .unwrap_or_default();
                                            if ui
                                                .add(var_btn)
                                                .on_hover_text(format!(
                                                    "Swap into primary slot (score {:.2}, ~{:.0}m{})",
                                                    var.score, var.avg_time_mins, trait_note
                                                ))
                                                .clicked()
                                            {
                                                clicked_variant =
                                                    Some((step.item_id, var.item_id));
                                            }
                                        }
                                    });
                                }
                            });
                        }
                    });
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
