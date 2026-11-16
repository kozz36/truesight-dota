use super::theme::*;
use super::viewmodel::*;
use super::{MatrixTab, TrueSightGuiApp};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, Margin, Rect, RichText, Sense, Stroke,
    StrokeKind, Vec2, pos2, vec2,
};
use truesight_core::domain::{HeroId, TEAM_SIZE};
use truesight_core::math::DraftAnalysis5v5;

impl TrueSightGuiApp {
    pub(super) fn render_matrix_card(
        &mut self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
    ) -> (Rect, Vec2, f32, f32, f32) {
        let title = match self.matrix_tab {
            MatrixTab::Counter => "COUNTER MATRIX",
            MatrixTab::Synergy => "ALLY SYNERGY",
            MatrixTab::Comp => "COMPOSITION",
        };
        let mut metrics = (Vec2::ZERO, 0.0_f32, 0.0_f32, 0.0_f32);

        let card_rect = hud_card_fill(ui, |ui| {
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
                            RichText::new(lbl).size(10.0).strong().color(if is_on {
                                P_VOID
                            } else {
                                P_MUTED
                            }),
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

            metrics = match self.matrix_tab {
                MatrixTab::Counter => self.render_counter_tab(ui, analysis),
                MatrixTab::Synergy => self.render_synergy_tab(ui, analysis),
                MatrixTab::Comp => {
                    self.render_comp_tab(ui, analysis);
                    let (cell_sz, h_gap, v_gap) = Self::matrix_grid_metrics(ui);
                    (cell_sz, h_gap, v_gap, ui.min_rect().max.y)
                }
            }
        });
        (card_rect, metrics.0, metrics.1, metrics.2, metrics.3)
    }

    fn matrix_grid_metrics(ui: &egui::Ui) -> (Vec2, f32, f32) {
        let v_gap = 4.0_f32;
        let avail_w = ui.available_width().max(260.0);
        let avail_h = ui.available_height().max(280.0);
        // Reserve ~58px at bottom for the gradient scale bar + wrapped caption + clearance
        let max_w = ((avail_w - 6.0 * v_gap) / 7.0).floor();
        let max_h = ((avail_h - 58.0 - 6.0 * v_gap) / 7.0).floor();
        let side = max_w.min(max_h).clamp(32.0, 56.0);
        let h_gap = ((avail_w - 7.0 * side) / 6.0).clamp(4.0, 6.0);
        (vec2(side, side), h_gap, v_gap)
    }

    fn render_counter_tab(
        &self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
    ) -> (Vec2, f32, f32, f32) {
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
                            ui.painter().image(
                                t.id(),
                                rh_r,
                                uv_aspect_crop(cell_sz),
                                Color32::WHITE,
                            );
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
        let note_resp = ui.label(
            RichText::new("Cell portrait = matchup winner. Right col = ally avg; bottom row = enemy avg vs team.")
                .size(9.0)
                .color(P_MUTED),
        );
        (cell_sz, h_gap, v_gap, note_resp.rect.max.y)
    }

    fn render_synergy_tab(
        &self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
    ) -> (Vec2, f32, f32, f32) {
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
                    ui.painter()
                        .line_segment([pos2(r.min.x, r.max.y), r.max], Stroke::new(2.0_f32, P_GEM));
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
                            ui.painter().image(
                                t.id(),
                                rh_r,
                                uv_aspect_crop(cell_sz),
                                Color32::WHITE,
                            );
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
                            let tex = self.textures.heroes.get(&partner.hero_id).map(|s| &s.toned);
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
        let note_resp = ui.label(
            RichText::new("Symmetric ally pairs. Right col = avg synergy with allies; corner = normalized Ally Synergy.")
                .size(9.0)
                .color(P_MUTED),
        );
        (cell_sz, h_gap, v_gap, note_resp.rect.max.y)
    }

    fn render_comp_tab(&self, ui: &mut egui::Ui, analysis: &DraftAnalysis5v5) {
        let ally_diag = compute_comp_diagnostic(&self.session.draft.allies, &self.session.matrix);
        let enemy_diag = compute_comp_diagnostic(&self.session.draft.enemies, &self.session.matrix);
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
                                if let Some(t) = self.textures.heroes.get(hid).map(|s| &s.color) {
                                    ui.painter()
                                        .image(t.id(), r, uv_square_crop(), Color32::WHITE);
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
                                if let Some(t) = self.textures.heroes.get(hid).map(|s| &s.color) {
                                    ui.painter()
                                        .image(t.id(), r, uv_square_crop(), Color32::WHITE);
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
