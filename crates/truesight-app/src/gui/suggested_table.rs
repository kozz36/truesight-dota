use super::TrueSightGuiApp;
use super::theme::*;
use eframe::egui::{
    self, Align, Align2, CornerRadius, FontId, Layout, Rect, RichText, Sense, Stroke, StrokeKind,
    pos2, vec2,
};
use truesight_core::domain::{Role, Side, TEAM_SIZE};

pub(super) struct SuggestedRenderOutput {
    pub(super) card: Rect,
    pub(super) last_col_max_x: f32,
    pub(super) comfort_hdr_max_x: f32,
    pub(super) row_rects: Vec<Rect>,
    pub(super) vs_row_rects: Vec<Rect>,
    pub(super) pick_rects: Vec<Rect>,
    pub(super) pick_stripe_rects: Vec<Option<Rect>>,
}

impl TrueSightGuiApp {
    pub(super) fn render_suggested_picks_card(
        &mut self,
        ui: &mut egui::Ui,
    ) -> SuggestedRenderOutput {
        let target_role = self.session.draft.target_role;
        let target_slot_idx = target_role.index();
        let mut last_col_max_x = 0.0_f32;
        let mut comfort_hdr_max_x = 0.0_f32;
        let mut row_rects = Vec::new();
        let mut vs_row_rects = Vec::new();
        let mut pick_rects = Vec::new();
        let mut pick_stripe_rects = Vec::new();

        let card_rect = hud_card_fill(ui, |ui| {
            let card_w = ui.available_width();
            let compact_hdr = card_w < 680.0;
            // Header
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = if compact_hdr { 4.0 } else { 8.0 };
                ui.label(
                    RichText::new(format!(
                        "SUGGESTED PICKS · {}",
                        target_role.label().to_ascii_uppercase()
                    ))
                    .size(if compact_hdr { 10.0 } else { 11.0 })
                    .strong()
                    .color(P_GLINT),
                );

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = if compact_hdr { 4.0 } else { 6.0 };
                    if !self.suggest_filter.is_empty() && ui.small_button("×").clicked() {
                        self.suggest_filter.clear();
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.suggest_filter)
                            .desired_width(if compact_hdr { 88.0 } else { 135.0 })
                            .hint_text("Filter..."),
                    );

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        for (idx, role) in Role::ALL.into_iter().enumerate().rev() {
                            let is_on = self.session.draft.target_role == role;
                            let lbl = if compact_hdr {
                                format!("P{}", idx + 1)
                            } else {
                                format!("POS {}", idx + 1)
                            };
                            let btn = egui::Button::new(
                                RichText::new(lbl)
                                    .size(if compact_hdr { 9.5 } else { 10.0 })
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
            let compact_tbl = table_w < 740.0;
            let pick_col_w = if compact_tbl { 40.0 } else { 52.0 };
            let pick_btn_sz = if compact_tbl {
                vec2(34.0, 17.0)
            } else {
                vec2(44.0, 20.0)
            };
            let pick_font = if compact_tbl { 9.0 } else { 10.0 };
            let split_gap = if compact_tbl { 4.0 } else { 6.0 };
            let left_w = (table_w - pick_col_w - split_gap).max(200.0);
            let usable_left_w = (left_w - 14.0).max(200.0);

            let row_h = if compact_tbl { 22.0 } else { 28.0 };
            let row_gap = if compact_tbl { 4.5 } else { 6.0 };
            let portrait_sz = if compact_tbl {
                vec2(32.0, 18.0)
            } else {
                vec2(48.0, 27.0)
            };
            let hero_col_w = if compact_tbl {
                (usable_left_w * 0.175).clamp(96.0, 128.0)
            } else {
                (usable_left_w * 0.20).clamp(160.0, 340.0)
            };
            let vs_cell_sz = if compact_tbl { 19.0 } else { 28.0 };
            let vs_gap = if compact_tbl { 1.5 } else { 2.0 };
            let vs_font = if compact_tbl { 7.0 } else { 8.5 };
            let win_bar_w = if compact_tbl { 22.0 } else { 52.0 };
            let pip_w = if compact_tbl { 3.0 } else { 4.0 };
            let hdr_font = if compact_tbl { 8.0 } else { 9.5 };
            let body_font = if compact_tbl { 9.5 } else { 12.0 };
            let stat_font = if compact_tbl { 9.5 } else { 11.5 };
            let col_gap = if compact_tbl {
                ((usable_left_w - 415.0 - hero_col_w) / 9.0).clamp(3.0, 12.0)
            } else {
                ((usable_left_w - 535.0 - hero_col_w) / 9.0).clamp(6.0, 36.0)
            };

            let filtered_recs: Vec<_> = recs_all
                .iter()
                .enumerate()
                .filter(|(_, rec)| {
                    if filter_lower.is_empty() {
                        return true;
                    }
                    self.session
                        .matrix
                        .hero(rec.hero_id)
                        .map(|h| h.display_name.as_str())
                        .unwrap_or("Unknown")
                        .to_ascii_lowercase()
                        .contains(&filter_lower)
                })
                .collect();

            egui::ScrollArea::vertical()
                .id_salt("v4_suggested_picks_vscroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let v_top_y = ui.cursor().top();
                    let mut hdr_y_center = v_top_y + 8.0;

                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = split_gap;

                        // Left horizontally scrollable metrics columns (# .. COMFORT)
                        ui.allocate_ui_with_layout(
                            vec2(left_w, ui.available_height()),
                            Layout::top_down(Align::Min),
                            |ui| {
                                egui::ScrollArea::horizontal()
                                    .id_salt("v4_suggested_picks_hscroll")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        egui::Grid::new("v4_suggested_picks_grid")
                                            .striped(true)
                                            .spacing([col_gap, row_gap])
                                            .show(ui, |ui| {
                                                let rank_hdr = ui.label(
                                                    RichText::new("#")
                                                        .size(hdr_font)
                                                        .strong()
                                                        .color(P_MUTED),
                                                );
                                                hdr_y_center = rank_hdr.rect.center().y;
                                                ui.allocate_ui_with_layout(
                                                    vec2(hero_col_w, 16.0),
                                                    Layout::left_to_right(Align::Center),
                                                    |ui| {
                                                        ui.set_min_width(hero_col_w);
                                                        ui.label(
                                                            RichText::new("HERO")
                                                                .size(hdr_font)
                                                                .strong()
                                                                .color(P_MUTED),
                                                        );
                                                    },
                                                );
                                                ui.label(
                                                    RichText::new(if compact_tbl {
                                                        "WIN%"
                                                    } else {
                                                        "PROJECTED WIN"
                                                    })
                                                    .size(hdr_font)
                                                    .strong()
                                                    .color(P_MUTED),
                                                );
                                                ui.label(
                                                    RichText::new("GAIN")
                                                        .size(hdr_font)
                                                        .strong()
                                                        .color(P_MUTED),
                                                );
                                                ui.label(
                                                    RichText::new(if compact_tbl {
                                                        "META"
                                                    } else {
                                                        "META WR"
                                                    })
                                                    .size(hdr_font)
                                                    .strong()
                                                    .color(P_MUTED),
                                                );
                                                ui.label(
                                                    RichText::new("7D")
                                                        .size(hdr_font)
                                                        .strong()
                                                        .color(P_MUTED),
                                                );
                                                ui.label(
                                                    RichText::new("VS ENEMIES")
                                                        .size(hdr_font)
                                                        .strong()
                                                        .color(P_MUTED),
                                                );
                                                ui.label(
                                                    RichText::new(if compact_tbl {
                                                        "CNT"
                                                    } else {
                                                        "AVG CNT"
                                                    })
                                                    .size(hdr_font)
                                                    .strong()
                                                    .color(P_MUTED),
                                                );
                                                ui.label(
                                                    RichText::new(if compact_tbl {
                                                        "SYN"
                                                    } else {
                                                        "AVG SYN"
                                                    })
                                                    .size(hdr_font)
                                                    .strong()
                                                    .color(P_MUTED),
                                                );
                                                let comfort_hdr = ui.label(
                                                    RichText::new(if compact_tbl {
                                                        "CMFT"
                                                    } else {
                                                        "COMFORT"
                                                    })
                                                    .size(hdr_font)
                                                    .strong()
                                                    .color(P_MUTED),
                                                );
                                                comfort_hdr_max_x = comfort_hdr.rect.max.x;
                                                ui.end_row();

                                                for &(idx, rec) in &filtered_recs {
                                                    let profile =
                                                        self.session.matrix.hero(rec.hero_id);
                                                    let name = profile
                                                        .map(|h| h.display_name.as_str())
                                                        .unwrap_or("Unknown");

                                                    // 1. Rank
                                                    ui.label(
                                                        RichText::new(format!("{}", idx + 1))
                                                            .size(body_font)
                                                            .strong()
                                                            .color(P_MUTED),
                                                    );

                                                    // 2. Hero Portrait + Name (exact row bounds for pinned PICK)
                                                    let hero_cell = ui.allocate_ui_with_layout(
                                                        vec2(hero_col_w, row_h),
                                                        Layout::left_to_right(Align::Center),
                                                        |ui| {
                                                            ui.set_min_width(hero_col_w);
                                                            ui.set_max_width(hero_col_w);
                                                            ui.spacing_mut().item_spacing.x =
                                                                if compact_tbl { 5.0 } else { 7.0 };
                                                            let tex = self
                                                                .textures
                                                                .heroes
                                                                .get(&rec.hero_id)
                                                                .map(|s| &s.color);
                                                            paint_portrait_16_9(
                                                                ui,
                                                                tex,
                                                                portrait_sz,
                                                                P_LINE,
                                                                1.0,
                                                            );
                                                            ui.add(
                                                                egui::Label::new(
                                                                    RichText::new(name)
                                                                        .size(body_font)
                                                                        .strong()
                                                                        .color(P_GLINT),
                                                                )
                                                                .truncate(),
                                                            );
                                                        },
                                                    );
                                                    row_rects.push(hero_cell.response.rect);

                                                    // 3. Projected Win% + bar
                                                    let proj =
                                                        rec.projected_win_probability * 100.0;
                                                    ui.allocate_ui_with_layout(
                                                        vec2(
                                                            ui.available_size_before_wrap().x,
                                                            row_h,
                                                        ),
                                                        Layout::left_to_right(Align::Center),
                                                        |ui| {
                                                            ui.set_min_height(row_h);
                                                            ui.spacing_mut().item_spacing.x =
                                                                if compact_tbl { 4.0 } else { 6.0 };
                                                            ui.label(
                                                                RichText::new(format!(
                                                                    "{proj:4.1}%"
                                                                ))
                                                                .size(body_font)
                                                                .strong()
                                                                .color(P_GLINT),
                                                            );
                                                            let (bar_r, _) = ui
                                                                .allocate_exact_size(
                                                                    vec2(win_bar_w, 4.0),
                                                                    Sense::hover(),
                                                                );
                                                            ui.painter().rect_filled(
                                                                bar_r, 0.0, P_INDIGO_3,
                                                            );
                                                            let fw = bar_r.width()
                                                                * (proj / 100.0).clamp(0.0, 1.0);
                                                            ui.painter().rect_filled(
                                                                Rect::from_min_max(
                                                                    bar_r.min,
                                                                    pos2(
                                                                        bar_r.min.x + fw,
                                                                        bar_r.max.y,
                                                                    ),
                                                                ),
                                                                0.0,
                                                                P_GEM,
                                                            );
                                                        },
                                                    );

                                                    // 4. Draft Gain
                                                    let gain = rec.net_probability_delta * 100.0;
                                                    ui.label(
                                                        RichText::new(fmt_pct(gain))
                                                            .size(body_font)
                                                            .strong()
                                                            .color(
                                                                tone_for_user(gain, 1.0).color(),
                                                            ),
                                                    );

                                                    // 5. Meta WR
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "{:4.1}%",
                                                            rec.base_meta_wr * 100.0
                                                        ))
                                                        .size(stat_font)
                                                        .color(P_SILVER),
                                                    );

                                                    // 6. 7d Trend
                                                    let trend_pct = profile
                                                        .map(|h| h.trend_delta * 100.0)
                                                        .unwrap_or(0.0);
                                                    if trend_pct.abs() >= EPS_PCT {
                                                        paint_trend_badge_scaled(
                                                            ui,
                                                            trend_pct,
                                                            if compact_tbl { 7.5 } else { 8.5 },
                                                            if compact_tbl { 2 } else { 4 },
                                                        );
                                                    } else {
                                                        ui.label(
                                                            RichText::new("·")
                                                                .size(stat_font)
                                                                .color(P_MUTED),
                                                        );
                                                    }

                                                    // 7. VS Enemies (5 square tinted portraits, aligned to row_h)
                                                    let vs_total_w = TEAM_SIZE as f32 * vs_cell_sz
                                                        + (TEAM_SIZE.saturating_sub(1)) as f32
                                                            * vs_gap;
                                                    let vs_cell = ui.allocate_ui_with_layout(
                                                        vec2(vs_total_w, row_h),
                                                        Layout::left_to_right(Align::Center),
                                                        |ui| {
                                                            ui.set_min_size(vec2(
                                                                vs_total_w, row_h,
                                                            ));
                                                            ui.spacing_mut().item_spacing.x =
                                                                vs_gap;
                                                            let mut bounds = Rect::NOTHING;
                                                            for j in 0..TEAM_SIZE {
                                                                let cell_resp = if let Some(ep) =
                                                                    self.session.draft.enemies[j]
                                                                {
                                                                    let d = self
                                                                        .session
                                                                        .matrix
                                                                        .cell(
                                                                            rec.hero_id,
                                                                            ep.hero_id,
                                                                        )
                                                                        .counter_delta
                                                                        * 100.0;
                                                                    let tex = self
                                                                        .textures
                                                                        .heroes
                                                                        .get(&ep.hero_id)
                                                                        .map(|s| &s.toned);
                                                                    paint_tinted_square_cell(
                                                                        ui, vs_cell_sz, tex, d,
                                                                        vs_font, 1.5,
                                                                    )
                                                                } else {
                                                                    let (r, resp) = ui
                                                                        .allocate_exact_size(
                                                                            vec2(
                                                                                vs_cell_sz,
                                                                                vs_cell_sz,
                                                                            ),
                                                                            Sense::hover(),
                                                                        );
                                                                    ui.painter().rect_stroke(
                                                                        r,
                                                                        0.0,
                                                                        Stroke::new(
                                                                            1.0_f32, P_LINE,
                                                                        ),
                                                                        StrokeKind::Inside,
                                                                    );
                                                                    resp
                                                                };
                                                                bounds =
                                                                    bounds.union(cell_resp.rect);
                                                            }
                                                            bounds
                                                        },
                                                    );
                                                    vs_row_rects.push(vs_cell.inner);

                                                    // 8. Avg Cnt
                                                    let cnt = rec.counter_delta_sum * 100.0;
                                                    ui.label(
                                                        RichText::new(fmt_pct(cnt))
                                                            .size(stat_font)
                                                            .strong()
                                                            .color(tone_for_user(cnt, 1.0).color()),
                                                    );

                                                    // 9. Avg Syn
                                                    let syn = rec.synergy_delta_sum * 100.0;
                                                    ui.label(
                                                        RichText::new(fmt_pct(syn))
                                                            .size(stat_font)
                                                            .strong()
                                                            .color(tone_for_user(syn, 1.0).color()),
                                                    );

                                                    // 10. Comfort (% + 5-pip cyan meter)
                                                    let com = rec.comfort_delta * 100.0;
                                                    ui.allocate_ui_with_layout(
                                                        vec2(
                                                            ui.available_size_before_wrap().x,
                                                            row_h,
                                                        ),
                                                        Layout::left_to_right(Align::Center),
                                                        |ui| {
                                                            ui.set_min_height(row_h);
                                                            ui.spacing_mut().item_spacing.x =
                                                                if compact_tbl { 3.0 } else { 5.0 };
                                                            ui.label(
                                                                RichText::new(fmt_pct(com))
                                                                    .size(stat_font)
                                                                    .strong()
                                                                    .color(
                                                                        tone_for_user(com, 1.0)
                                                                            .color(),
                                                                    ),
                                                            );
                                                            let pips =
                                                                ((com.max(0.0) / 0.5).round()
                                                                    as usize)
                                                                    .clamp(0, 5);
                                                            ui.spacing_mut().item_spacing.x =
                                                                if compact_tbl { 1.5 } else { 2.0 };
                                                            for k in 0..5 {
                                                                let (pr, _) = ui
                                                                    .allocate_exact_size(
                                                                        vec2(
                                                                            pip_w,
                                                                            if compact_tbl {
                                                                                8.0
                                                                            } else {
                                                                                10.0
                                                                            },
                                                                        ),
                                                                        Sense::hover(),
                                                                    );
                                                                ui.painter().rect_filled(
                                                                    pr,
                                                                    0.0,
                                                                    if k < pips {
                                                                        P_CYAN
                                                                    } else {
                                                                        P_INDIGO_3
                                                                    },
                                                                );
                                                            }
                                                        },
                                                    );

                                                    ui.end_row();
                                                }
                                            });
                                    });
                            },
                        );

                        // Right pinned PICK column placed at exact measured Grid row centers
                        let col_h = row_rects
                            .last()
                            .map_or(24.0, |last_r| (last_r.max.y - v_top_y).max(24.0));
                        let (pick_area, _) =
                            ui.allocate_exact_size(vec2(pick_col_w, col_h), Sense::hover());
                        ui.painter().text(
                            pos2(pick_area.center().x, hdr_y_center),
                            Align2::CENTER_CENTER,
                            "ACT",
                            FontId::proportional(hdr_font),
                            P_MUTED,
                        );

                        for (display_idx, (&(_, rec), row_r)) in
                            filtered_recs.iter().zip(row_rects.iter()).enumerate()
                        {
                            // Match egui::Grid::striped(true) cadence: header is grid row 0,
                            // so display_idx 0, 2, 4... correspond to odd striped rows.
                            let is_striped = display_idx % 2 == 0;
                            let row_bg = if is_striped {
                                ui.visuals().faint_bg_color
                            } else {
                                P_INDIGO_1
                            };
                            let stripe_rect = if is_striped {
                                let r = Rect::from_min_max(
                                    pos2(pick_area.min.x, row_r.min.y - 0.5 * row_gap),
                                    pos2(pick_area.max.x, row_r.max.y + 0.5 * row_gap),
                                );
                                ui.painter().rect_filled(r, CornerRadius::same(2), row_bg);
                                Some(r)
                            } else {
                                None
                            };
                            pick_stripe_rects.push(stripe_rect);

                            let picked_name = self
                                .session
                                .matrix
                                .hero(rec.hero_id)
                                .map(|h| h.display_name.clone())
                                .unwrap_or_else(|| "Unknown".to_owned());
                            let btn_rect = Rect::from_center_size(
                                pos2(pick_area.center().x, row_r.center().y),
                                pick_btn_sz,
                            );
                            let pick_btn = egui::Button::new(
                                RichText::new("PICK").size(pick_font).strong().color(P_GEM),
                            )
                            .fill(row_bg)
                            .stroke(Stroke::new(1.0_f32, P_GEM));
                            let pick_resp = ui.put(btn_rect, pick_btn);
                            last_col_max_x = last_col_max_x.max(pick_resp.rect.max.x);
                            pick_rects.push(pick_resp.rect);
                            if pick_resp.clicked() {
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
                        }
                    });
                });
        });

        SuggestedRenderOutput {
            card: card_rect,
            last_col_max_x,
            comfort_hdr_max_x,
            row_rects,
            vs_row_rects,
            pick_rects,
            pick_stripe_rects,
        }
    }
}
