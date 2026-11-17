use super::theme::*;
use super::{PickerTarget, TrueSightGuiApp};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Layout, Margin, Rect, RichText, Sense, Stroke,
    StrokeKind, pos2, vec2,
};
use truesight_core::domain::{Bracket, Faction, Role, Side, TEAM_SIZE};

const TOP_BTN_H: f32 = 22.0;

pub(super) struct TopBarRenderOutput {
    pub(super) bar: Rect,
    pub(super) options_button: Rect,
    pub(super) reset_button: Rect,
    pub(super) bracket_bar: Rect,
}

impl TrueSightGuiApp {
    fn vision_status_color(&self) -> Color32 {
        if self.session.vision_error {
            P_DIRE
        } else if self.session.vision_available {
            P_GEM
        } else {
            P_NEUTRAL
        }
    }

    fn load_demo_5v5(&mut self) {
        self.session.reset_draft();
        self.drag_slot = None;
        let _ = crate::execute_command(&mut self.session, "ally 1 faceless void");
        let _ = crate::execute_command(&mut self.session, "ally 3 magnus");
        let _ = crate::execute_command(&mut self.session, "ally 4 shadow demon");
        let _ = crate::execute_command(&mut self.session, "ally 5 crystal maiden");
        let _ = crate::execute_command(&mut self.session, "enemy 1 medusa");
        let _ = crate::execute_command(&mut self.session, "enemy 2 storm spirit");
        let _ = crate::execute_command(&mut self.session, "enemy 3 axe");
        let _ = crate::execute_command(&mut self.session, "enemy 4 tusk");
        let _ = crate::execute_command(&mut self.session, "enemy 5 witch doctor");
        self.session.ally_locked_slots = [false; TEAM_SIZE];
        self.session.enemy_locked_slots = [false; TEAM_SIZE];
        self.session.draft.target_role = Role::Mid;
        self.picker_target = Some(PickerTarget::Slot(Side::Allies, 1));
        self.status_banner = "Loaded Demo 5v5 (Pos 2 Mid picking · 9/10 picked).".into();
    }

    fn render_brand_block(&self, ui: &mut egui::Ui, compact: bool) {
        if let Some(ref gem_tex) = self.textures.gem {
            let gem_sz = if compact {
                vec2(24.0, 18.0)
            } else {
                vec2(28.0, 21.0)
            };
            let (rect, _) = ui.allocate_exact_size(gem_sz, Sense::hover());
            ui.painter()
                .image(gem_tex.id(), rect, uv_full(), Color32::WHITE);
            ui.painter().rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, P_GEM_DEEP),
                StrokeKind::Inside,
            );
        }
        let title_sz = if compact { 13.0 } else { 14.0 };
        ui.label(RichText::new("TRUE").size(title_sz).strong().color(P_GLINT));
        ui.add_space(-6.0);
        ui.label(RichText::new("SIGHT").size(title_sz).strong().color(P_GEM));
        if !compact {
            ui.label(
                RichText::new("· DRAFT ORACLE")
                    .size(13.0)
                    .strong()
                    .color(P_GLINT),
            );
        }
    }

    fn render_faction_button(&mut self, ui: &mut egui::Ui, compact: bool) {
        let fac_col = match self.session.draft.ally_faction {
            Faction::Radiant => P_GEM,
            Faction::Dire => P_DIRE,
        };
        let label = if compact {
            format!("YOU: {}", self.session.draft.ally_faction.label())
        } else {
            format!("YOU: {} <->", self.session.draft.ally_faction.label())
        };
        let fac_btn = egui::Button::new(
            RichText::new(label)
                .size(if compact { 10.0 } else { 10.5 })
                .strong()
                .color(fac_col),
        )
        .fill(P_INDIGO_2)
        .stroke(Stroke::new(1.0_f32, P_GEM_DEEP))
        .min_size(vec2(0.0, TOP_BTN_H));
        if ui
            .add(fac_btn)
            .on_hover_text("Toggle your team side (Radiant / Dire)")
            .clicked()
        {
            self.session
                .set_ally_faction(self.session.draft.ally_faction.opposite());
        }
    }

    fn render_bracket_buttons(&mut self, ui: &mut egui::Ui, compact: bool, reverse: bool) -> Rect {
        ui.spacing_mut().item_spacing.x = 1.0;
        if compact {
            ui.spacing_mut().button_padding = vec2(3.5, 2.0);
        }
        let mut union_rect = Rect::NOTHING;
        let brackets: [Bracket; 8] = if reverse {
            let mut arr = Bracket::ALL;
            arr.reverse();
            arr
        } else {
            Bracket::ALL
        };
        for bracket in brackets {
            let is_on = self.session.draft.bracket == bracket;
            let btn = egui::Button::new(
                RichText::new(bracket.label())
                    .size(if compact { 9.5 } else { 10.5 })
                    .strong()
                    .color(if is_on { P_VOID } else { P_MUTED }),
            )
            .fill(if is_on { P_GEM } else { P_INDIGO_2 })
            .stroke(Stroke::new(1.0_f32, if is_on { P_GEM } else { P_LINE }))
            .min_size(vec2(0.0, TOP_BTN_H));
            let resp = ui.add(btn);
            union_rect = if union_rect == Rect::NOTHING {
                resp.rect
            } else {
                union_rect.union(resp.rect)
            };
            if resp.clicked() {
                self.session.set_bracket(bracket);
            }
        }
        union_rect
    }

    fn render_options_button(&mut self, ui: &mut egui::Ui, compact: bool) -> Rect {
        let dot_col = self.vision_status_color();
        let active = self.options_open;
        let btn_w = if compact { 76.0 } else { 82.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(btn_w, TOP_BTN_H), Sense::click());
        let bg = if active {
            P_GEM
        } else if resp.hovered() {
            P_INDIGO_3
        } else {
            P_INDIGO_2
        };
        let border = if active || resp.hovered() {
            P_GEM
        } else {
            P_LINE
        };
        let fg = if active { P_VOID } else { P_GLINT };

        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(2), bg);
        painter.rect_stroke(
            rect,
            CornerRadius::same(2),
            Stroke::new(1.0_f32, border),
            StrokeKind::Inside,
        );

        // Paint status dot cleanly to the left of the label so it never overlaps text.
        let dot_rect = Rect::from_center_size(
            pos2(
                rect.min.x + if compact { 10.0 } else { 11.0 },
                rect.center().y,
            ),
            vec2(6.0, 6.0),
        );
        painter.rect_filled(dot_rect, 0.0, if active { P_VOID } else { dot_col });

        painter.text(
            pos2(dot_rect.max.x + 5.0, rect.center().y),
            Align2::LEFT_CENTER,
            "OPTIONS",
            egui::FontId::proportional(if compact { 10.0 } else { 10.5 }),
            fg,
        );

        let resp = resp.on_hover_text(format!(
            "Open dev & telemetry options\nGSI · {}\nCapture · {}\n{}",
            self.session.last_gsi_state, self.session.vision_status, self.session.db_status
        ));
        if resp.clicked() {
            self.options_open = !self.options_open;
        }
        rect
    }

    fn render_reset_button(&mut self, ui: &mut egui::Ui, compact: bool) -> Rect {
        let resp = ui.add(
            egui::Button::new(
                RichText::new("RESET")
                    .size(if compact { 10.0 } else { 10.5 })
                    .strong()
                    .color(P_GLINT),
            )
            .fill(P_INDIGO_2)
            .min_size(vec2(0.0, TOP_BTN_H)),
        );
        if resp.clicked() {
            self.session.reset_draft();
            self.picker_target = None;
            self.enemy_scout_role = None;
            self.drag_slot = None;
            self.status_banner = "Draft cleared.".into();
        }
        resp.rect
    }

    fn paint_top_bar_separator(ui: &mut egui::Ui) {
        let (sep_r, _) = ui.allocate_exact_size(vec2(1.0, 14.0), Sense::hover());
        ui.painter().line_segment(
            [sep_r.center_top(), sep_r.center_bottom()],
            Stroke::new(1.0_f32, P_LINE),
        );
    }

    /// Renders the user-facing top bar and returns bounds for alignment verification.
    pub(super) fn render_top_bar(&mut self, ui: &mut egui::Ui) -> TopBarRenderOutput {
        let mut options_btn_rect = Rect::NOTHING;
        let mut reset_btn_rect = Rect::NOTHING;
        let mut bracket_bar_rect = Rect::NOTHING;
        let bar_rect = hud_card(ui, |ui| {
            let avail_w = ui.available_width();
            let compact = avail_w < 1120.0;

            if avail_w >= 910.0 {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = if compact { 4.0 } else { 8.0 };
                    if compact {
                        ui.spacing_mut().button_padding = vec2(4.0, 2.0);
                    }
                    self.render_brand_block(ui, compact);
                    self.render_faction_button(ui, compact);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = if compact { 4.0 } else { 6.0 };
                        reset_btn_rect = self.render_reset_button(ui, compact);
                        options_btn_rect = self.render_options_button(ui, compact);
                        ui.add_space(if compact { 2.0 } else { 6.0 });
                        Self::paint_top_bar_separator(ui);
                        ui.add_space(if compact { 2.0 } else { 6.0 });
                        bracket_bar_rect = self.render_bracket_buttons(ui, compact, true);
                    });
                });
            } else {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        self.render_brand_block(ui, true);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            reset_btn_rect = self.render_reset_button(ui, true);
                            options_btn_rect = self.render_options_button(ui, true);
                            Self::paint_top_bar_separator(ui);
                            self.render_faction_button(ui, true);
                        });
                    });
                    ui.horizontal_wrapped(|ui| {
                        bracket_bar_rect = self.render_bracket_buttons(ui, true, false);
                    });
                });
            }
        });
        TopBarRenderOutput {
            bar: bar_rect,
            options_button: options_btn_rect,
            reset_button: reset_btn_rect,
            bracket_bar: bracket_bar_rect,
        }
    }

    /// Renders the floating `OPTIONS` drawer holding dev/telemetry indicators and actions.
    ///
    /// Anchors top-right below the measured `top_bar` bottom (`top_bar_bottom_y`) so single- and
    /// two-row headers remain unobstructed; overlapping the upper draft row underneath while
    /// reviewing telemetry is intentional.
    pub(super) fn render_options_window(
        &mut self,
        ctx: &egui::Context,
        top_bar_bottom_y: f32,
    ) -> (Option<Rect>, Option<Rect>) {
        if !self.options_open {
            return (None, None);
        }

        let mut close_window =
            !self.account_editor_open && ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut demo_btn_rect = None;

        let max_opt_w = (ctx.screen_rect().width() - 24.0).clamp(260.0, 370.0);
        let anchor_y =
            (top_bar_bottom_y + 4.0).clamp(14.0, (ctx.screen_rect().height() - 120.0).max(14.0));
        let resp = egui::Window::new("OPTIONS · DEV & TELEMETRY")
            .collapsible(false)
            .resizable(false)
            .fade_in(false)
            .anchor(Align2::RIGHT_TOP, vec2(-12.0, anchor_y))
            .constrain_to(ctx.screen_rect())
            .frame(hud_modal_frame(ctx))
            .default_width(max_opt_w)
            .max_width(max_opt_w)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;

                ui.label(
                    RichText::new("LIVE CAPTURE & GSI")
                        .size(9.5)
                        .strong()
                        .color(P_MUTED),
                );
                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_LINE))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (dot_r, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
                            ui.painter().rect_filled(dot_r, 0.0, P_GEM);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("GSI · {}", self.session.last_gsi_state))
                                        .size(10.5)
                                        .color(P_SILVER),
                                )
                                .wrap(),
                            );
                        });
                        ui.horizontal(|ui| {
                            let (dot_r, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
                            ui.painter()
                                .rect_filled(dot_r, 0.0, self.vision_status_color());
                            ui.label(
                                RichText::new(&self.session.vision_status)
                                    .size(10.5)
                                    .strong()
                                    .color(P_GLINT),
                            );
                        });
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.session.vision_detail)
                                    .size(9.5)
                                    .color(P_MUTED),
                            )
                            .wrap(),
                        );
                    });

                ui.label(
                    RichText::new("DATABASE & STEAM LINK")
                        .size(9.5)
                        .strong()
                        .color(P_MUTED),
                );
                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_LINE))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.session.db_status)
                                    .size(10.5)
                                    .color(P_SILVER),
                            )
                            .wrap(),
                        );
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            let sync_label = if self.session.sync_in_flight {
                                "SYNCING..."
                            } else {
                                "SYNC DB"
                            };
                            if ui
                                .add_enabled(
                                    !self.session.sync_in_flight,
                                    egui::Button::new(
                                        RichText::new(sync_label)
                                            .size(10.5)
                                            .strong()
                                            .color(P_GLINT),
                                    )
                                    .fill(P_INDIGO_1),
                                )
                                .clicked()
                            {
                                if self.session.start_background_sync(true) {
                                    self.status_banner = "Syncing OpenDota in background...".into();
                                } else {
                                    self.status_banner = "Sync already in progress...".into();
                                }
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("STEAM ACCOUNT...").size(10.5).color(P_GLINT),
                                    )
                                    .fill(P_INDIGO_1),
                                )
                                .clicked()
                            {
                                self.account_editor_open = !self.account_editor_open;
                                self.account_input = self
                                    .session
                                    .cache_mgr
                                    .player_account_id
                                    .map(|id| id.to_string())
                                    .unwrap_or_default();
                            }
                        });
                    });

                ui.horizontal(|ui| {
                    let demo_resp = ui.add(
                        egui::Button::new(
                            RichText::new("DEMO 5V5").size(10.5).strong().color(P_GEM),
                        )
                        .fill(P_INDIGO_2)
                        .stroke(Stroke::new(1.0_f32, P_GEM_DEEP)),
                    );
                    demo_btn_rect = Some(demo_resp.rect);
                    if demo_resp.clicked() {
                        self.load_demo_5v5();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("CLOSE").clicked() {
                            close_window = true;
                        }
                    });
                });
            });

        if close_window {
            self.options_open = false;
        }
        let win_rect = resp.as_ref().map(|r| r.response.rect);
        if let Some(r) = &resp {
            let painter = ctx.layer_painter(r.response.layer_id);
            paint_hud_corner_ticks(&painter, r.response.rect, P_GEM);
        }
        (win_rect, demo_btn_rect)
    }
}
