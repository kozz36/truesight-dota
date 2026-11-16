use super::theme::*;
use super::viewmodel::*;
use super::{MatrixTab, TrueSightGuiApp};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, Margin, Rect, RichText, Sense, Stroke,
    StrokeKind, Vec2, pos2, vec2,
};
use truesight_core::domain::{HeroId, TEAM_SIZE};
use truesight_core::math::DraftAnalysis5v5;

pub(super) struct MatrixRenderOutput {
    pub(super) card: Rect,
    pub(super) grid: Rect,
    pub(super) cell_size: Vec2,
    pub(super) h_gap: f32,
    pub(super) v_gap: f32,
    pub(super) bottom_y: f32,
    pub(super) comp_enemies_hdr: Rect,
}

impl TrueSightGuiApp {
    pub(super) fn render_matrix_card(
        &mut self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
        docked_comp_mode: bool,
    ) -> MatrixRenderOutput {
        let active_tab = if docked_comp_mode && self.matrix_tab == MatrixTab::Comp {
            MatrixTab::Counter
        } else {
            self.matrix_tab
        };
        let title = match active_tab {
            MatrixTab::Counter => "COUNTER MATRIX",
            MatrixTab::Synergy => "ALLY SYNERGY",
            MatrixTab::Comp => "COMPOSITION",
        };
        let mut grid_rect = Rect::NOTHING;
        let mut cell_sz = Vec2::ZERO;
        let mut h_gap = 0.0_f32;
        let mut v_gap = 0.0_f32;
        let mut bottom_y = 0.0_f32;
        let mut comp_enemies_hdr = Rect::NOTHING;

        let card_rect = hud_card_fill(ui, |ui| {
            let compact_hdr = ui.available_width() < 325.0;
            let short_title = match active_tab {
                MatrixTab::Counter => "MATRIX",
                MatrixTab::Synergy => "SYNERGY",
                MatrixTab::Comp => "COMP",
            };
            // Header with COUNTER | SYNERGY | COMP tabs (COMP hidden when docked below)
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if compact_hdr { short_title } else { title })
                        .size(if compact_hdr { 10.5 } else { 11.0 })
                        .strong()
                        .color(P_GLINT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    let tabs: &[(MatrixTab, &str)] = if docked_comp_mode {
                        &[
                            (MatrixTab::Synergy, "SYNERGY"),
                            (MatrixTab::Counter, "COUNTER"),
                        ]
                    } else {
                        &[
                            (MatrixTab::Comp, "COMP"),
                            (MatrixTab::Synergy, "SYNERGY"),
                            (MatrixTab::Counter, "COUNTER"),
                        ]
                    };
                    for &(tab, lbl) in tabs {
                        let is_on = active_tab == tab;
                        let btn = egui::Button::new(
                            RichText::new(lbl)
                                .size(if compact_hdr { 9.5 } else { 10.0 })
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

            ui.add_space(if ui.available_height() < 245.0 {
                4.0
            } else {
                8.0
            });

            match active_tab {
                MatrixTab::Counter => {
                    (grid_rect, cell_sz, h_gap, v_gap, bottom_y) =
                        self.render_counter_tab(ui, analysis);
                }
                MatrixTab::Synergy => {
                    (grid_rect, cell_sz, h_gap, v_gap, bottom_y) =
                        self.render_synergy_tab(ui, analysis);
                }
                MatrixTab::Comp => {
                    egui::ScrollArea::vertical()
                        .id_salt("v4_comp_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            comp_enemies_hdr = self.render_comp_tab(ui, analysis);
                        });
                    (cell_sz, h_gap, v_gap) = Self::matrix_grid_metrics(ui);
                    bottom_y = ui.min_rect().max.y;
                }
            }
        });
        MatrixRenderOutput {
            card: card_rect,
            grid: grid_rect,
            cell_size: cell_sz,
            h_gap,
            v_gap,
            bottom_y,
            comp_enemies_hdr,
        }
    }

    /// Renders a standalone Composition card below the active matrix card on tall windows.
    pub(super) fn render_comp_card(
        &self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
    ) -> (Rect, Rect) {
        let mut comp_enemies_hdr = Rect::NOTHING;
        let card_rect = hud_card_fill(ui, |ui| {
            ui.label(
                RichText::new("COMPOSITION")
                    .size(11.0)
                    .strong()
                    .color(P_GLINT),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("v4_docked_comp_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    comp_enemies_hdr = self.render_comp_tab(ui, analysis);
                });
        });
        (card_rect, comp_enemies_hdr)
    }

    fn matrix_grid_metrics(ui: &egui::Ui) -> (Vec2, f32, f32) {
        let avail_w = ui.available_width().max(180.0);
        let avail_h = ui.available_height().max(180.0);
        let v_gap = if avail_h < 250.0 { 3.0 } else { 4.0 };
        let reserve_bottom = if avail_h < 250.0 { 52.0 } else { 58.0 };
        let max_w = ((avail_w - 6.0 * v_gap) / 7.0).floor();
        let max_h = ((avail_h - reserve_bottom - 6.0 * v_gap) / 7.0).floor();
        let side = max_w.min(max_h).clamp(20.0, 56.0);
        let h_gap = ((avail_w - 7.0 * side) / 6.0).clamp(v_gap, 6.0);
        (vec2(side, side), h_gap, v_gap)
    }

    fn render_counter_tab(
        &self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
    ) -> (Rect, Vec2, f32, f32, f32) {
        let avail_w = ui.available_width();
        let (cell_sz, h_gap, v_gap) = Self::matrix_grid_metrics(ui);
        let grid_w = 7.0 * cell_sz.x + 6.0 * h_gap;
        let left_pad = ((avail_w - grid_w) * 0.5).max(0.0);
        let cell_font = (cell_sz.x * 0.24).clamp(7.5, 11.0);
        let hdr_font = (cell_sz.x * 0.22).clamp(8.0, 10.0);

        let grid_rect = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if left_pad > 0.0 {
                    ui.add_space(left_pad);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = v_gap;

                    // Header Row: Blank + 5 Enemy portraits + AVG
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = h_gap;
                        ui.allocate_exact_size(cell_sz, Sense::hover());
                        for j in 0..TEAM_SIZE {
                            let (r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                            if let Some(ep) = self.session.draft.enemies[j] {
                                if let Some(t) =
                                    self.textures.heroes.get(&ep.hero_id).map(|s| &s.color)
                                {
                                    ui.painter().image(
                                        t.id(),
                                        r,
                                        uv_aspect_crop(cell_sz),
                                        Color32::WHITE,
                                    );
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
                                    FontId::proportional(hdr_font),
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
                            FontId::proportional(hdr_font),
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
                                if let Some(t) =
                                    self.textures.heroes.get(&ap.hero_id).map(|s| &s.color)
                                {
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
                                    FontId::proportional(hdr_font),
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
                                    let tex =
                                        self.textures.heroes.get(&winner_id).map(|s| &s.toned);
                                    paint_tinted_cell(ui, cell_sz, tex, pct, cell_font, 2.0);
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
                            FontId::proportional(hdr_font),
                            P_GLINT,
                        );
                        for j in 0..TEAM_SIZE {
                            let col_avg = enemy_counter_col_avg(analysis, j).map(|v| v * 100.0);
                            paint_avg_square_cell(ui, cell_sz, col_avg);
                        }
                        paint_total_corner_cell(
                            ui,
                            cell_sz,
                            analysis.total_counter_edge * 100.0,
                            "EDGE",
                        );
                    });
                })
                .response
                .rect
            })
            .inner;

        ui.add_space(6.0);
        paint_matrix_scale_bar(ui, "Enemy wins", "Ally wins");
        ui.add_space(2.0);
        let note_resp = ui.label(
            RichText::new("Cell portrait = matchup winner. Right col = ally avg; bottom row = enemy avg vs team.")
                .size(9.0)
                .color(P_MUTED),
        );
        (grid_rect, cell_sz, h_gap, v_gap, note_resp.rect.max.y)
    }

    fn render_synergy_tab(
        &self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
    ) -> (Rect, Vec2, f32, f32, f32) {
        let avail_w = ui.available_width();
        let (cell_sz, h_gap, v_gap) = Self::matrix_grid_metrics(ui);
        let grid_w = 7.0 * cell_sz.x + 6.0 * h_gap;
        let left_pad = ((avail_w - grid_w) * 0.5).max(0.0);
        let cell_font = (cell_sz.x * 0.24).clamp(7.5, 11.0);
        let hdr_font = (cell_sz.x * 0.22).clamp(8.0, 10.0);

        let grid_rect = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if left_pad > 0.0 {
                    ui.add_space(left_pad);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = v_gap;

                    // Header Row: Blank + 5 Ally portraits + AVG
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = h_gap;
                        ui.allocate_exact_size(cell_sz, Sense::hover());
                        for k in 0..TEAM_SIZE {
                            let (r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                            if let Some(ap) = self.session.draft.allies[k] {
                                if let Some(t) =
                                    self.textures.heroes.get(&ap.hero_id).map(|s| &s.color)
                                {
                                    ui.painter().image(
                                        t.id(),
                                        r,
                                        uv_aspect_crop(cell_sz),
                                        Color32::WHITE,
                                    );
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
                                    FontId::proportional(hdr_font),
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
                            FontId::proportional(hdr_font),
                            P_GLINT,
                        );
                    });

                    // 5 Ally Rows
                    for i in 0..TEAM_SIZE {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = h_gap;
                            let (rh_r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                            if let Some(ap) = self.session.draft.allies[i] {
                                if let Some(t) =
                                    self.textures.heroes.get(&ap.hero_id).map(|s| &s.color)
                                {
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
                                    FontId::proportional(hdr_font),
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
                                        FontId::proportional(hdr_font),
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
                                    paint_tinted_cell(ui, cell_sz, tex, pct, cell_font, 2.0);
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
                })
                .response
                .rect
            })
            .inner;

        ui.add_space(6.0);
        paint_matrix_scale_bar(ui, "Anti-synergy", "Synergy");
        ui.add_space(2.0);
        let note_resp = ui.label(
            RichText::new("Symmetric ally pairs. Right col = avg synergy with allies; corner = normalized Ally Synergy.")
                .size(9.0)
                .color(P_MUTED),
        );
        (grid_rect, cell_sz, h_gap, v_gap, note_resp.rect.max.y)
    }

    fn render_comp_tab(&self, ui: &mut egui::Ui, analysis: &DraftAnalysis5v5) -> Rect {
        let ally_diag = compute_comp_diagnostic(&self.session.draft.allies, &self.session.matrix);
        let enemy_diag = compute_comp_diagnostic(&self.session.draft.enemies, &self.session.matrix);
        let ally_pct = ally_diag.score * 100.0;
        let enemy_pct = enemy_diag.score * 100.0;
        let comp_edge = analysis.total_composition_edge * 100.0;

        let avail_w = ui.available_width().max(220.0);
        ui.set_min_width(avail_w);
        let narrow_comp = avail_w < 300.0;
        let mut enemies_hdr_rect = Rect::NOTHING;

        // 1. Symmetrical Header: Allies (left) | Comp Edge (center) | Enemies (flush right)
        ui.columns(3, |cols| {
            cols[0].vertical(|ui| {
                ui.label(
                    RichText::new(format!("ALLIES · {} / 5", ally_diag.picked))
                        .size(9.5)
                        .color(P_MUTED),
                );
                ui.label(
                    RichText::new(fmt_pct(ally_pct))
                        .size(if narrow_comp { 15.0 } else { 18.0 })
                        .strong()
                        .color(tone_for_user(ally_pct, 1.0).color()),
                );
            });
            cols[1].vertical_centered(|ui| {
                ui.label(
                    RichText::new(fmt_pct(comp_edge))
                        .size(if narrow_comp { 15.0 } else { 18.0 })
                        .strong()
                        .color(tone_for_user(comp_edge, 1.0).color()),
                );
                ui.label(RichText::new("COMP EDGE").size(9.0).color(P_MUTED));
            });
            cols[2].with_layout(Layout::top_down(Align::Max), |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new(format!("ENEMIES · {} / 5", enemy_diag.picked))
                        .size(9.5)
                        .color(P_MUTED),
                );
                ui.label(
                    RichText::new(fmt_pct(enemy_pct))
                        .size(if narrow_comp { 15.0 } else { 18.0 })
                        .strong()
                        .color(tone_for_user(enemy_pct, -1.0).color()),
                );
            });
            enemies_hdr_rect = cols[2].min_rect();
        });

        ui.add_space(6.0);

        // 2. Four Symmetrical Mirror Rows (Melee, Ranged, Disables, Initiators)
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

        let mir_mid_w = if narrow_comp { 68.0 } else { 82.0 };
        let mir_side_w = ((avail_w - mir_mid_w - 8.0) * 0.5).max(68.0);
        let icon_gap = if narrow_comp { 2.0 } else { 3.0 };
        let mir_icon_sz = ((mir_side_w - 20.0 - 4.0 * icon_gap) / 5.0).clamp(12.0, 20.0);

        for (title, sub, a_ids, e_ids) in mirror_rows {
            ui.allocate_ui_with_layout(
                vec2(avail_w, 24.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    ui.allocate_ui_with_layout(
                        vec2(mir_side_w, 24.0),
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            ui.set_min_width(mir_side_w);
                            ui.spacing_mut().item_spacing.x = icon_gap;
                            ui.label(
                                RichText::new(format!("{}", a_ids.len()))
                                    .size(if narrow_comp { 11.5 } else { 13.0 })
                                    .strong()
                                    .color(P_GEM),
                            );
                            for hid in a_ids {
                                let (r, _) = ui.allocate_exact_size(
                                    vec2(mir_icon_sz, mir_icon_sz),
                                    Sense::hover(),
                                );
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
                        vec2(mir_mid_w, 24.0),
                        Layout::top_down(Align::Center),
                        |ui| {
                            ui.set_min_width(mir_mid_w);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(RichText::new(title).size(9.5).strong().color(P_SILVER));
                            ui.label(RichText::new(sub).size(8.5).color(P_MUTED));
                        },
                    );

                    ui.allocate_ui_with_layout(
                        vec2(mir_side_w, 24.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_min_width(mir_side_w);
                            ui.spacing_mut().item_spacing.x = icon_gap;
                            ui.label(
                                RichText::new(format!("{}", e_ids.len()))
                                    .size(if narrow_comp { 11.5 } else { 13.0 })
                                    .strong()
                                    .color(P_DIRE),
                            );
                            for hid in e_ids {
                                let (r, _) = ui.allocate_exact_size(
                                    vec2(mir_icon_sz, mir_icon_sz),
                                    Sense::hover(),
                                );
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

        // 3. Six-Rule Table filling 100% width symmetrically
        let stat_col_w = if narrow_comp { 42.0 } else { 58.0 };
        let col_gap = if narrow_comp { 4.0 } else { 8.0 };
        let rule_col_w = (avail_w - 3.0 * stat_col_w - 3.0 * col_gap).max(80.0);
        egui::Grid::new("v4_comp_rules_grid")
            .striped(true)
            .spacing([col_gap, 5.0])
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    vec2(rule_col_w, 16.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_width(rule_col_w);
                        ui.label(RichText::new("RULE").size(9.0).strong().color(P_MUTED));
                    },
                );
                for h in ["IMP", "ALLIES", "ENEMIES"] {
                    ui.allocate_ui_with_layout(
                        vec2(stat_col_w, 16.0),
                        Layout::centered_and_justified(egui::Direction::LeftToRight),
                        |ui| {
                            ui.set_min_width(stat_col_w);
                            ui.label(RichText::new(h).size(9.0).strong().color(P_MUTED));
                        },
                    );
                }
                ui.end_row();

                for (idx, &(rule_name, impact)) in COMP_RULES.iter().enumerate() {
                    let imp_pct = impact * 100.0;
                    ui.allocate_ui_with_layout(
                        vec2(rule_col_w, 18.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_min_width(rule_col_w);
                            ui.set_max_width(rule_col_w);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(rule_name)
                                        .size(if narrow_comp { 9.0 } else { 10.0 })
                                        .color(P_SILVER),
                                )
                                .truncate(),
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(stat_col_w, 18.0),
                        Layout::centered_and_justified(egui::Direction::LeftToRight),
                        |ui| {
                            ui.label(
                                RichText::new(fmt_pct(imp_pct))
                                    .size(10.0)
                                    .color(tone_for_user(imp_pct, 1.0).color()),
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(stat_col_w, 18.0),
                        Layout::centered_and_justified(egui::Direction::LeftToRight),
                        |ui| {
                            if ally_diag.rule_active[idx] {
                                let c = tone_for_user(imp_pct, 1.0).color();
                                paint_mini_badge(ui, &fmt_pct(imp_pct), c, c);
                            } else {
                                ui.label(RichText::new("·").size(10.0).color(P_LINE));
                            }
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(stat_col_w, 18.0),
                        Layout::centered_and_justified(egui::Direction::LeftToRight),
                        |ui| {
                            if enemy_diag.rule_active[idx] {
                                let c = tone_for_user(imp_pct, -1.0).color();
                                paint_mini_badge(ui, &fmt_pct(imp_pct), c, c);
                            } else {
                                ui.label(RichText::new("·").size(10.0).color(P_LINE));
                            }
                        },
                    );
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
                ui.add(
                    egui::Label::new(
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
                    )
                    .wrap(),
                );
            });
        let hr = hint_fr.response.rect;
        ui.painter().rect_filled(
            Rect::from_min_max(hr.min, pos2(hr.min.x + 3.0, hr.max.y)),
            0.0,
            P_GEM,
        );
        enemies_hdr_rect
    }
}
