use super::theme::*;
use super::viewmodel::*;
use super::{PickerTarget, SlotTelemetry, TrueSightGuiApp};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Id, Layout, Rect, RichText, Sense, Stroke, StrokeKind,
    pos2, vec2,
};
use truesight_core::domain::{AttackType, Faction, Role, Side, TEAM_SIZE};
use truesight_core::math::DraftAnalysis5v5;

impl TrueSightGuiApp {
    pub(super) fn render_team_card(
        &mut self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
        side: Side,
        slots_out: &mut [SlotTelemetry; TEAM_SIZE],
    ) -> Rect {
        let tr = super::i18n::strings(self.session.ui_language);
        let is_ally = side == Side::Allies;
        let accent = if is_ally { P_GEM } else { P_DIRE };
        let title = if is_ally {
            tr.allies_title
        } else {
            tr.enemies_title
        };
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
            let card_w = ui.available_width();
            let compact_w = card_w < 305.0;
            let compact_h = ui.available_height() < 340.0;
            let slot_h = if compact_h { 52.0 } else { 56.0 };
            let slot_gap = if compact_h { 5.0 } else { 8.0 };
            let portrait_sz = if compact_w {
                vec2(48.0, 27.0)
            } else if compact_h {
                vec2(52.0, 29.25)
            } else {
                vec2(64.0, 36.0)
            };
            let net_col_w = if compact_w { 42.0 } else { 54.0 };

            ui.spacing_mut().item_spacing.y = slot_gap;
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
            ui.add_space(if compact_h { 1.0 } else { 4.0 });

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

                let is_dragging_this = self.drag_slot == Some((side, slot));
                let is_local_you = is_ally
                    && current_pick
                        .is_some_and(|p| self.session.local_player_hero == Some(p.hero_id));
                let is_locked = match side {
                    Side::Allies => self.session.ally_locked_slots[slot],
                    Side::Enemies => self.session.enemy_locked_slots[slot],
                };
                let bg_fill = if is_picking {
                    Color32::from_rgba_unmultiplied(46, 230, 122, 22)
                } else {
                    P_INDIGO_2
                };
                let border_col = if is_dragging_this {
                    P_NEUTRAL
                } else if is_picking {
                    P_GEM
                } else {
                    P_INDIGO_3
                };

                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), slot_h), Sense::hover());
                ui.painter().rect_filled(r, 0.0, bg_fill);
                ui.painter().rect_stroke(
                    r,
                    0.0,
                    Stroke::new(1.0_f32, border_col),
                    StrokeKind::Inside,
                );
                ui.painter().rect_filled(
                    Rect::from_min_max(r.min, pos2(r.min.x + 3.0, r.max.y)),
                    0.0,
                    accent,
                );

                // 1. Portrait (exact 16:9) or dashed '?'
                let portrait_rect = Rect::from_min_size(
                    pos2(r.min.x + 8.0, (r.center().y - portrait_sz.y * 0.5).round()),
                    portrait_sz,
                );
                if let Some(pick) = current_pick {
                    if let Some(tex) = self.textures.heroes.get(&pick.hero_id).map(|s| &s.color) {
                        ui.painter()
                            .image(tex.id(), portrait_rect, uv_full(), Color32::WHITE);
                    }
                    ui.painter().rect_stroke(
                        portrait_rect,
                        0.0,
                        Stroke::new(1.0_f32, P_LINE),
                        StrokeKind::Inside,
                    );
                } else {
                    ui.painter().rect_stroke(
                        portrait_rect,
                        0.0,
                        Stroke::new(1.0_f32, if is_picking { P_GEM } else { P_LINE }),
                        StrokeKind::Inside,
                    );
                    ui.painter().text(
                        portrait_rect.center(),
                        Align2::CENTER_CENTER,
                        "?",
                        FontId::proportional(15.0),
                        if is_picking { P_GEM } else { P_MUTED },
                    );
                }

                // 2. Right Clear Button (`×`) outside card hit zone
                let clear_rect_opt = if current_pick.is_some() {
                    let cr = Rect::from_center_size(
                        pos2(r.max.x - 11.0, r.center().y),
                        vec2(14.0, 14.0),
                    );
                    let clear_resp = ui
                        .interact(
                            cr,
                            Id::new(("slot_clear_btn", is_ally, slot)),
                            Sense::click(),
                        )
                        .on_hover_text("Clear hero slot");
                    let x_col = if clear_resp.hovered() {
                        P_DIRE
                    } else {
                        P_MUTED
                    };
                    ui.painter().text(
                        cr.center(),
                        Align2::CENTER_CENTER,
                        "×",
                        FontId::proportional(13.0),
                        x_col,
                    );
                    if clear_resp.clicked() {
                        self.session.clear_slot_manual(side, slot);
                        if self.picker_target == Some(PickerTarget::Slot(side, slot)) {
                            self.picker_target = None;
                        }
                        self.status_banner = format!(
                            "Cleared {} P{}.",
                            if is_ally { "Ally" } else { "Enemy" },
                            slot + 1
                        );
                    }
                    Some(cr)
                } else {
                    None
                };

                // 3. Right Net / Threat Column (deterministic rect left of clear button)
                let net_right = match clear_rect_opt {
                    Some(cr) => cr.min.x - 4.0,
                    None => r.max.x - 8.0,
                };
                let net_rect = Rect::from_min_max(
                    pos2(
                        (net_right - net_col_w).max(portrait_rect.max.x + 12.0),
                        r.min.y + 8.0,
                    ),
                    pos2(net_right, r.max.y - 8.0),
                );
                {
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(net_rect)
                            .layout(Layout::top_down(Align::Max)),
                    );
                    let ui = &mut child;
                    ui.set_clip_rect(net_rect);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if let Some(b) = breakdown {
                        let net_pct = b.net * 100.0;
                        ui.label(
                            RichText::new(fmt_pct(net_pct))
                                .size(if compact_w { 11.5 } else { 13.5 })
                                .strong()
                                .color(tone_for_user(net_pct, sign).color()),
                        );
                        ui.label(
                            RichText::new(net_caption)
                                .size(if compact_w { 7.5 } else { 8.5 })
                                .color(P_MUTED),
                        );
                    } else {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("[Enter]")
                                .size(if compact_w { 9.5 } else { 11.0 })
                                .strong()
                                .color(if is_picking { P_GEM } else { P_MUTED }),
                        );
                    }
                }

                // 4. Clipped Middle 3-line Telemetry Column
                let mid_rect = Rect::from_min_max(
                    pos2(
                        portrait_rect.max.x + if compact_w { 6.0 } else { 8.0 },
                        r.min.y + 2.0,
                    ),
                    pos2(
                        (net_rect.min.x - if compact_w { 4.0 } else { 6.0 })
                            .max(portrait_rect.max.x + 24.0),
                        r.max.y - 2.0,
                    ),
                );
                let tight_mid = mid_rect.width() < 165.0;
                let mid_content_bottom;
                {
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(mid_rect)
                            .layout(Layout::top_down(Align::Min)),
                    );
                    let ui = &mut child;
                    ui.set_clip_rect(mid_rect);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.spacing_mut().interact_size.y = 11.0;
                    ui.spacing_mut().item_spacing.y = 0.5;
                    let badge_font = if tight_mid { 7.0 } else { 8.5 };
                    let badge_pad = if tight_mid { 1 } else { 4 };
                    if let Some(pick) = current_pick {
                        let profile = self.session.matrix.hero(pick.hero_id);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = if tight_mid { 1.5 } else { 2.0 };
                            let role_lbl = if tight_mid {
                                match slot {
                                    0 => "P1",
                                    1 => "P2",
                                    2 => "P3",
                                    3 => "P4",
                                    _ => "P5",
                                }
                            } else {
                                role_short[slot]
                            };
                            ui.label(
                                RichText::new(role_lbl)
                                    .size(if tight_mid { 8.0 } else { 9.0 })
                                    .color(P_MUTED),
                            );
                            if let Some(h) = profile {
                                let atk_lbl = match h.attack_type {
                                    AttackType::Melee => "M",
                                    AttackType::Ranged => "R",
                                };
                                paint_mini_badge_scaled(
                                    ui, atk_lbl, P_SILVER, P_LINE, badge_font, badge_pad,
                                );
                                if h.has_disable {
                                    paint_mini_badge_scaled(
                                        ui, "DIS", P_CYAN, P_CYAN, badge_font, badge_pad,
                                    );
                                }
                                if h.has_initiation {
                                    paint_mini_badge_scaled(
                                        ui,
                                        if tight_mid { "IN" } else { "INIT" },
                                        P_GEM,
                                        P_GEM_DEEP,
                                        badge_font,
                                        badge_pad,
                                    );
                                }
                                paint_trend_badge_scaled(
                                    ui,
                                    h.trend_delta * 100.0,
                                    badge_font,
                                    badge_pad,
                                );
                            }
                            if is_local_you {
                                paint_mini_badge_scaled(
                                    ui, "YOU", P_GEM, P_GEM, badge_font, badge_pad,
                                );
                            }
                            if is_locked {
                                paint_mini_badge_scaled(
                                    ui,
                                    if tight_mid { "LCK" } else { "LOCK" },
                                    P_SILVER,
                                    P_LINE,
                                    badge_font,
                                    badge_pad,
                                );
                            }
                        });

                        let hero_name = profile
                            .map(|h| h.display_name.as_str())
                            .unwrap_or("Unknown");
                        ui.add(
                            egui::Label::new(
                                RichText::new(hero_name)
                                    .size(if tight_mid { 11.0 } else { 12.5 })
                                    .strong()
                                    .color(P_GLINT),
                            )
                            .truncate(),
                        );

                        if let Some(b) = breakdown {
                            let m_pct = b.meta * 100.0;
                            let c_pct = b.cnt * 100.0;
                            let s_pct = b.syn * 100.0;
                            let stat_sz = if tight_mid { 8.0 } else { 9.5 };
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = if tight_mid { 1.5 } else { 2.5 };
                                ui.label(
                                    RichText::new(if tight_mid { "M" } else { "Meta" })
                                        .size(stat_sz)
                                        .color(P_MUTED),
                                );
                                ui.label(
                                    RichText::new(fmt_val(m_pct))
                                        .size(stat_sz)
                                        .strong()
                                        .color(tone_for_user(m_pct, sign).color()),
                                );
                                ui.label(RichText::new("·").size(stat_sz).color(P_MUTED));
                                ui.label(
                                    RichText::new(if tight_mid { "C" } else { "Cnt" })
                                        .size(stat_sz)
                                        .color(P_MUTED),
                                );
                                ui.label(
                                    RichText::new(fmt_val(c_pct))
                                        .size(stat_sz)
                                        .strong()
                                        .color(tone_for_user(c_pct, sign).color()),
                                );
                                ui.label(RichText::new("·").size(stat_sz).color(P_MUTED));
                                ui.label(
                                    RichText::new(if tight_mid { "S" } else { "Syn" })
                                        .size(stat_sz)
                                        .color(P_MUTED),
                                );
                                ui.label(
                                    RichText::new(fmt_val(s_pct))
                                        .size(stat_sz)
                                        .strong()
                                        .color(tone_for_user(s_pct, sign).color()),
                                );
                            });
                        }
                    } else {
                        ui.label(
                            RichText::new(format!(
                                "{} · {}",
                                if tight_mid {
                                    match slot {
                                        0 => "P1",
                                        1 => "P2",
                                        2 => "P3",
                                        3 => "P4",
                                        _ => "P5",
                                    }
                                } else {
                                    role_short[slot]
                                },
                                if is_picking { "PICKING" } else { "EMPTY" }
                            ))
                            .size(if tight_mid { 8.0 } else { 9.0 })
                            .color(if is_picking {
                                P_GEM
                            } else {
                                P_MUTED
                            }),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(if tight_mid {
                                    "Click to pick..."
                                } else {
                                    "Click to pick hero..."
                                })
                                .size(if tight_mid { 10.5 } else { 12.0 })
                                .color(P_MUTED),
                            )
                            .truncate(),
                        );
                        ui.label(
                            RichText::new(format!("Synced with Pos {}", slot + 1))
                                .size(if tight_mid { 8.0 } else { 9.5 })
                                .color(P_MUTED),
                        );
                    }
                    mid_content_bottom = ui.min_rect().max.y;
                }

                slots_out[slot] = SlotTelemetry {
                    row: r,
                    portrait: portrait_rect,
                    mid: mid_rect,
                    mid_content_bottom,
                    net: net_rect,
                    clear: clear_rect_opt,
                };

                // Drag-to-swap target highlight and drop commit
                if let Some((drag_side, src_slot)) = self.drag_slot
                    && drag_side == side
                    && src_slot != slot
                {
                    let (pointer_over, released) = ui.input(|i| {
                        let over = i
                            .pointer
                            .interact_pos()
                            .or_else(|| i.pointer.hover_pos())
                            .is_some_and(|p| r.contains(p));
                        (over, i.pointer.any_released())
                    });
                    if pointer_over {
                        ui.painter().rect_stroke(
                            r,
                            0.0,
                            Stroke::new(2.0_f32, accent),
                            StrokeKind::Inside,
                        );
                        if released {
                            self.session.swap_slots(side, src_slot, slot);
                            self.drag_slot = None;
                            self.picker_target = None;
                            self.status_banner = format!(
                                "Swapped {} P{} <-> P{}.",
                                if is_ally { "Ally" } else { "Enemy" },
                                src_slot + 1,
                                slot + 1
                            );
                        }
                    }
                }

                // Restrict hit rect strictly left of the clear (`×`) button
                let card_hit_rect = match clear_rect_opt {
                    Some(cr) => Rect::from_min_max(
                        r.min,
                        pos2((cr.min.x - 2.0).max(r.min.x + 40.0), r.max.y),
                    ),
                    None => r,
                };
                let card_resp = ui.interact(
                    card_hit_rect,
                    Id::new(("slot_card_click", is_ally, slot)),
                    Sense::click_and_drag(),
                );
                if card_resp.drag_started() && current_pick.is_some() {
                    self.drag_slot = Some((side, slot));
                    self.picker_target = None;
                }
                if card_resp.clicked() {
                    if is_ally {
                        self.session.draft.target_role =
                            Role::ALL.get(slot).copied().unwrap_or(Role::Carry);
                        if !self.session.local_player_hero_from_gsi
                            && let Some(pick) = current_pick
                        {
                            self.session.local_player_hero = Some(pick.hero_id);
                        }
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
}
