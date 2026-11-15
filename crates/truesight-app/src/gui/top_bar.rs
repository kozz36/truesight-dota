use super::theme::*;
use super::{PickerTarget, TrueSightGuiApp};
use eframe::egui::{self, Align, Color32, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, vec2};
use truesight_core::domain::{Bracket, Faction, Role, Side, TEAM_SIZE};

impl TrueSightGuiApp {
    pub(super) fn render_top_bar(&mut self, ui: &mut egui::Ui) -> Rect {
        hud_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if let Some(ref gem_tex) = self.textures.gem {
                    let (rect, _) = ui.allocate_exact_size(vec2(28.0, 21.0), Sense::hover());
                    ui.painter().image(gem_tex.id(), rect, uv_full(), Color32::WHITE);
                    ui.painter().rect_stroke(
                        rect,
                        0.0,
                        Stroke::new(1.0_f32, P_GEM_DEEP),
                        StrokeKind::Inside,
                    );
                }
                ui.label(
                    RichText::new("TRUE")
                        .size(14.0)
                        .strong()
                        .color(P_GLINT),
                );
                ui.add_space(-6.0);
                ui.label(
                    RichText::new("SIGHT")
                        .size(14.0)
                        .strong()
                        .color(P_GEM),
                );
                ui.label(
                    RichText::new("· DRAFT ORACLE")
                        .size(13.0)
                        .strong()
                        .color(P_GLINT),
                );

                // GSI pill
                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_LINE))
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot_r, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
                            ui.painter().rect_filled(dot_r, 0.0, P_GEM);
                            ui.label(
                                RichText::new(format!("GSI · {}", self.session.last_gsi_state))
                                    .size(10.5)
                                    .color(P_MUTED),
                            );
                        });
                    });

                // Interactive Faction Toggle Pill (YOU: RADIANT <-> / YOU: DIRE <->)
                let fac_col = match self.session.draft.ally_faction {
                    Faction::Radiant => P_GEM,
                    Faction::Dire => P_DIRE,
                };
                let fac_btn = egui::Button::new(
                    RichText::new(format!("YOU: {} <->", self.session.draft.ally_faction.label()))
                        .size(10.5)
                        .strong()
                        .color(fac_col),
                )
                .fill(P_INDIGO_2)
                .stroke(Stroke::new(1.0_f32, P_GEM_DEEP));
                if ui.add(fac_btn).clicked() {
                    self.session
                        .set_ally_faction(self.session.draft.ally_faction.opposite());
                }

                // OpenDota DB & Steam32 Account Link Pill (click to edit Steam32 / Auto / Unlinked)
                let db_btn = egui::Button::new(
                    RichText::new(&self.session.db_status)
                        .size(10.5)
                        .color(P_MUTED),
                )
                .fill(P_INDIGO_2)
                .stroke(Stroke::new(1.0_f32, P_LINE));
                if ui
                    .add(db_btn)
                    .on_hover_text("Click to configure Steam32 / Steam64 account link")
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

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("RESET").size(10.5).strong().color(P_GLINT),
                            )
                            .fill(P_INDIGO_2),
                        )
                        .clicked()
                    {
                        self.session.reset_draft();
                        self.picker_target = None;
                        self.drag_slot = None;
                        self.status_banner = "Draft cleared.".into();
                    }
                    let sync_label = if self.session.sync_in_flight {
                        "SYNCING..."
                    } else {
                        "SYNC DB"
                    };
                    if ui
                        .add_enabled(
                            !self.session.sync_in_flight,
                            egui::Button::new(
                                RichText::new(sync_label).size(10.5).strong().color(P_GLINT),
                            )
                            .fill(P_INDIGO_2),
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
                                RichText::new("DEMO 5V5").size(10.5).strong().color(P_GEM),
                            )
                            .fill(P_INDIGO_2)
                            .stroke(Stroke::new(1.0_f32, P_GEM_DEEP)),
                        )
                        .clicked()
                    {
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
                        self.status_banner =
                            "Loaded Demo 5v5 (Pos 2 Mid picking · 9/10 picked).".into();
                    }

                    // 8-Medal Bracket Segmented Control
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        for bracket in Bracket::ALL.into_iter().rev() {
                            let is_on = self.session.draft.bracket == bracket;
                            let btn = egui::Button::new(
                                RichText::new(bracket.label())
                                    .size(10.5)
                                    .strong()
                                    .color(if is_on { P_VOID } else { P_MUTED }),
                            )
                            .fill(if is_on { P_GEM } else { P_INDIGO_2 })
                            .stroke(Stroke::new(1.0_f32, if is_on { P_GEM } else { P_LINE }));
                            if ui.add(btn).clicked() {
                                self.session.set_bracket(bracket);
                            }
                        }
                    });
                });
            });
        })
    }

}
