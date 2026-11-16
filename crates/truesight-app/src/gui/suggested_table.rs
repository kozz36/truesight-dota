use super::TrueSightGuiApp;
use super::theme::*;
use eframe::egui::{self, Align, Layout, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use truesight_core::domain::{Role, Side, TEAM_SIZE};

impl TrueSightGuiApp {
    pub(super) fn render_suggested_picks_card(&mut self, ui: &mut egui::Ui) -> (Rect, f32) {
        let target_role = self.session.draft.target_role;
        let target_slot_idx = target_role.index();
        let mut last_col_max_x = 0.0_f32;

        let card_rect = hud_card_fill(ui, |ui| {
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
            let usable_w = (table_w - 16.0).max(580.0);
            let hero_col_w = (usable_w * 0.19).clamp(172.0, 340.0);
            let col_gap = ((usable_w - 578.0 - hero_col_w) / 10.0).max(8.0);

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
                                    ui.set_min_width(hero_col_w);
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
                                        ui.set_min_width(hero_col_w);
                                        ui.set_max_width(hero_col_w);
                                        ui.spacing_mut().item_spacing.x = 7.0;
                                        let tex = self
                                            .textures
                                            .heroes
                                            .get(&rec.hero_id)
                                            .map(|s| &s.color);
                                        paint_portrait_16_9(ui, tex, vec2(48.0, 27.0), P_LINE, 1.0);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(name)
                                                    .size(12.0)
                                                    .strong()
                                                    .color(P_GLINT),
                                            )
                                            .truncate(),
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
                                let trend_pct =
                                    profile.map(|h| h.trend_delta * 100.0).unwrap_or(0.0);
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
                                            paint_tinted_square_cell(ui, 28.0, tex, d, 8.5, 1.5);
                                        } else {
                                            let (r, _) = ui.allocate_exact_size(
                                                vec2(28.0, 28.0),
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
                                    let pips = ((com.max(0.0) / 0.5).round() as usize).clamp(0, 5);
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
                                    RichText::new("PICK").size(10.0).strong().color(P_GEM),
                                )
                                .fill(P_INDIGO_1)
                                .stroke(Stroke::new(1.0_f32, P_GEM));
                                let pick_resp = ui.add(pick_btn);
                                last_col_max_x = last_col_max_x.max(pick_resp.rect.max.x);
                                if pick_resp.clicked() {
                                    let picked_name = name.to_owned();
                                    self.session.set_slot_manual(
                                        Side::Allies,
                                        target_slot_idx,
                                        rec.hero_id,
                                    );
                                    self.status_banner = format!(
                                        "Picked {} into Ally P{}.",
                                        picked_name,
                                        target_slot_idx + 1
                                    );
                                }
                                ui.end_row();
                            }
                        });
                });
        });
        (card_rect, last_col_max_x)
    }
}
