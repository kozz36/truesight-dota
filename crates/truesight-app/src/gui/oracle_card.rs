use super::theme::*;
use super::viewmodel::*;
use super::{PickerTarget, TrueSightGuiApp};
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use truesight_core::domain::{HeroId, Role, Side, TEAM_SIZE};
use truesight_core::math::DraftAnalysis5v5;

impl TrueSightGuiApp {
    pub(super) fn render_oracle_card(
        &mut self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
        scout_cards_out: &mut Vec<Rect>,
    ) -> Rect {
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

            ui.add_space(4.0);

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

            ui.add_space(5.0);

            // 3. Four Edge Cards (Meta, Counter, Synergy, Composition)
            let pair_cnt = analysis
                .counter_grid
                .iter()
                .flatten()
                .flatten()
                .count();
            let mut ally_meta_sum = 0.0_f32;
            let mut enemy_meta_sum = 0.0_f32;
            for slot in 0..TEAM_SIZE {
                if let Some(b) = compute_slot_breakdown(&self.session, analysis, Side::Allies, slot)
                {
                    ally_meta_sum += b.meta * 100.0;
                }
                if let Some(b) =
                    compute_slot_breakdown(&self.session, analysis, Side::Enemies, slot)
                {
                    enemy_meta_sum += b.meta * 100.0;
                }
            }
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
                    format!(
                        "A {} · E {}",
                        fmt_val(ally_meta_sum),
                        fmt_val(enemy_meta_sum)
                    ),
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

            ui.add_space(5.0);

            // 4. Insight Strip (Strongest Pair + Hardest Counter)
            self.render_insight_strip(ui, analysis);

            ui.add_space(5.0);

            // 5. Draft Order (10 slots) & Bans (16 slots) side-by-side
            let thumb_row_h = 26.0_f32;
            ui.columns(2, |cols| {
                // Draft Order Box
                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_INDIGO_3))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(&mut cols[0], |ui| {
                        let inner_w = ui.available_width();
                        ui.set_min_width(inner_w);
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
                        ui.allocate_ui_with_layout(
                            vec2(inner_w, thumb_row_h),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                let w = ((inner_w - 9.0 * 3.0) / 10.0).max(20.0);
                                let h = (w * 9.0 / 16.0).round().min(thumb_row_h);
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
                            },
                        );
                    });

                // Bans Box
                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_INDIGO_3))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(&mut cols[1], |ui| {
                        let inner_w = ui.available_width();
                        ui.set_min_width(inner_w);
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
                        ui.allocate_ui_with_layout(
                            vec2(inner_w, thumb_row_h),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                let slots_shown = 8usize;
                                let w = ((inner_w - 7.0 * 3.0) / 8.0).max(24.0);
                                let h = (w * 9.0 / 16.0).round().min(thumb_row_h);
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
                            },
                        );
                    });
            });

            // 6. Active Slot / Ban Search Bar (inline inside Oracle when active), or Enemy Scout strip
            ui.add_space(4.0);
            if let Some(target) = self.picker_target {
                self.render_inline_picker(ui, target);
            } else {
                self.render_enemy_scout_strip(ui, scout_cards_out);
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

    fn render_enemy_scout_strip(&mut self, ui: &mut egui::Ui, scout_cards_out: &mut Vec<Rect>) {
        let active_role = self.active_enemy_scout_role();
        let role_labels = ["POS 1", "POS 2", "POS 3", "POS 4", "POS 5"];
        let card_size = vec2(48.0, 27.0);
        let gap = 5.0_f32;

        egui::Frame::NONE
            .fill(P_INDIGO_2)
            .stroke(Stroke::new(1.0_f32, P_DIRE_DEEP))
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                let inner_w = ui.available_width();
                ui.set_min_width(inner_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(
                        RichText::new("ENEMY SCOUT")
                            .size(9.0)
                            .strong()
                            .color(P_DIRE),
                    );
                    for (idx, &role) in Role::ALL.iter().enumerate() {
                        let selected = active_role == role;
                        let btn = egui::Button::new(
                            RichText::new(role_labels[idx])
                                .size(8.5)
                                .strong()
                                .color(if selected { P_VOID } else { P_MUTED }),
                        )
                        .fill(if selected { P_DIRE } else { P_INDIGO_1 })
                        .stroke(Stroke::new(
                            1.0_f32,
                            if selected { P_DIRE } else { P_LINE },
                        ))
                        .min_size(vec2(36.0, 15.0));
                        if ui.add(btn).clicked() {
                            self.enemy_scout_role = Some(role);
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} · Neutral WR", active_role.label()))
                                .size(8.5)
                                .color(P_MUTED),
                        );
                    });
                });

                ui.add_space(2.0);
                let max_cards = ((inner_w + gap) / (card_size.x + gap))
                    .floor()
                    .clamp(1.0, 11.0) as usize;
                let recs = self.session.recommend_enemy_heroes(active_role, max_cards);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for rec in recs {
                        let (rect, resp) = ui.allocate_exact_size(card_size, Sense::click());
                        scout_cards_out.push(rect);
                        let painter = ui.painter();
                        painter.rect_filled(rect, 0.0, P_INDIGO_3);
                        if let Some(tex) = self.textures.heroes.get(&rec.hero_id).map(|s| &s.color)
                        {
                            painter.image(tex.id(), rect, uv_full(), Color32::WHITE);
                        }

                        // Dark legibility shade across the lower half for overlaid WR%
                        let shade_top = rect.min.y + rect.height() * 0.48;
                        painter.rect_filled(
                            Rect::from_min_max(pos2(rect.min.x, shade_top), rect.max),
                            0.0,
                            Color32::from_rgba_unmultiplied(10, 9, 24, 210),
                        );

                        let enemy_wr = rec.projected_win_probability * 100.0;
                        let border_col = if resp.hovered() {
                            P_GLINT
                        } else if enemy_wr >= 50.0 {
                            P_DIRE
                        } else {
                            P_INDIGO_3
                        };
                        painter.rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(1.2_f32, border_col),
                            StrokeKind::Inside,
                        );
                        painter.text(
                            pos2(rect.center().x, rect.max.y - 1.5),
                            Align2::CENTER_BOTTOM,
                            format!("{enemy_wr:4.1}%"),
                            FontId::proportional(9.0),
                            P_GLINT,
                        );

                        let hname = self
                            .session
                            .matrix
                            .hero(rec.hero_id)
                            .map(|h| h.display_name.clone())
                            .unwrap_or_else(|| format!("Hero #{}", rec.hero_id.0));
                        let tip = format!(
                            "{hname} ({})\nProjected Enemy Win: {enemy_wr:4.1}% ({})\nClick to assign to Enemy P{}",
                            active_role.label(),
                            fmt_pct(rec.net_probability_delta * 100.0),
                            active_role.index() + 1,
                        );
                        let resp = resp.on_hover_text(tip);
                        if resp.clicked() {
                            self.session.set_slot_manual(
                                Side::Enemies,
                                active_role.index(),
                                rec.hero_id,
                            );
                            self.enemy_scout_role = None;
                            self.status_banner = format!(
                                "Scout assigned {hname} into Enemy P{}.",
                                active_role.index() + 1
                            );
                        }
                    }
                });
            });
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

    pub(super) fn apply_picker_choice(&mut self, target: PickerTarget, hid: HeroId, hname: &str) {
        match target {
            PickerTarget::Slot(side, slot) => {
                self.session.set_slot_manual(side, slot, hid);
                match side {
                    Side::Allies => {
                        self.status_banner = format!("Picked {hname} into Ally P{}.", slot + 1);
                    }
                    Side::Enemies => {
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

}
