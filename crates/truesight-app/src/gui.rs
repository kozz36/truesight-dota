use crate::AppSession;
use eframe::egui::{self, Color32, RichText};
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_core::domain::{Bracket, DraftPick, Role, Side, TEAM_SIZE};
use truesight_core::ports::DraftEventSource;

pub struct TrueSightGuiApp {
    session: AppSession,
    gsi_listener: Option<BoundedGsiListener>,
    search_query: String,
    selected_side: Side,
    selected_slot: usize,
    status_banner: String,
}

impl TrueSightGuiApp {
    pub fn new(session: AppSession, gsi_listener: Option<BoundedGsiListener>) -> Self {
        Self {
            session,
            gsi_listener,
            search_query: String::new(),
            selected_side: Side::Allies,
            selected_slot: 0,
            status_banner: "Select an Ally/Enemy slot and click a hero or type in Quick Search."
                .into(),
        }
    }
}

impl eframe::App for TrueSightGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(ref mut listener) = self.gsi_listener {
            if let Ok(events) = listener.poll_events() {
                for ev in events {
                    self.session.apply_event(ev);
                }
            }
        }

        let (analysis, recs) = self.session.evaluate();
        let green = Color32::from_rgb(46, 204, 113);
        let red = Color32::from_rgb(231, 76, 60);
        let gold = Color32::from_rgb(241, 196, 15);
        let dim = Color32::from_rgb(149, 165, 166);

        egui::TopBottomPanel::top("header_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("TRUESIGHT-DOTA · 5v5 LIVE DRAFT ORACLE").strong());
                ui.separator();
                ui.label(RichText::new(&self.session.last_gsi_state).color(dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset Draft").clicked() {
                        self.session.draft.clear();
                        self.status_banner = "Draft cleared.".into();
                    }
                    if ui.button("Load Demo 5v5").clicked() {
                        self.session.draft.clear();
                        let _ = crate::execute_command(&mut self.session, "ally 3 magnus");
                        let _ = crate::execute_command(&mut self.session, "ally 5 crystal maiden");
                        let _ = crate::execute_command(&mut self.session, "enemy 1 medusa");
                        let _ = crate::execute_command(&mut self.session, "enemy 2 storm spirit");
                        let _ = crate::execute_command(&mut self.session, "enemy 3 axe");
                        self.status_banner = "Loaded demo 5v5 state.".into();
                    }
                });
            });

            ui.separator();

            // Win Probability Thermometer
            let ally_pct = analysis.ally_win_probability * 100.0;
            let enemy_pct = 100.0 - ally_pct;
            let win_color = if ally_pct >= 50.0 { green } else { red };

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("ALLIES WIN PROBABILITY: {ally_pct:5.1}%"))
                        .size(18.0)
                        .strong()
                        .color(win_color),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("ENEMIES: {enemy_pct:5.1}%"))
                        .size(16.0)
                        .color(dim),
                );
                ui.separator();
                ui.label(format!(
                    "Meta Edge: {:+.2}%  |  Counter Edge: {:+.2}%  |  Synergy Edge: {:+.2}%",
                    analysis.total_meta_edge * 100.0,
                    analysis.total_counter_edge * 100.0,
                    analysis.total_synergy_edge * 100.0
                ));
            });

            ui.add(
                egui::ProgressBar::new(analysis.ally_win_probability.clamp(0.0, 1.0))
                    .fill(win_color)
                    .desired_height(10.0),
            );
        });

        egui::SidePanel::right("recommendations_panel")
            .min_width(430.0)
            .show(ctx, |ui| {
                ui.heading(
                    RichText::new(format!(
                        "Suggested Picks ({})",
                        self.session.draft.target_role.label()
                    ))
                    .color(gold),
                );

                ui.horizontal(|ui| {
                    ui.label("Target Role:");
                    for role in Role::ALL {
                        if ui
                            .selectable_label(
                                self.session.draft.target_role == role,
                                role.label(),
                            )
                            .clicked()
                        {
                            self.session.draft.target_role = role;
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Bracket:");
                    for bracket in Bracket::ALL {
                        if ui
                            .selectable_label(
                                self.session.draft.bracket == bracket,
                                bracket.label(),
                            )
                            .clicked()
                        {
                            self.session.set_bracket(bracket);
                        }
                    }
                });

                ui.separator();

                // Quick search / assign bar
                ui.horizontal(|ui| {
                    ui.label("Quick Pick:");
                    let resp = ui.text_edit_singleline(&mut self.search_query);
                    if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        || ui.button("Assign to Slot").clicked()
                    {
                        if let Some(hero_id) = self.session.find_hero(&self.search_query) {
                            let pick = Some(DraftPick {
                                hero_id,
                                role: Role::ALL.get(self.selected_slot).copied(),
                            });
                            let res = match self.selected_side {
                                Side::Allies => self.session.draft.set_ally(self.selected_slot, pick),
                                Side::Enemies => {
                                    self.session.draft.set_enemy(self.selected_slot, pick)
                                }
                            };
                            match res {
                                Ok(()) => {
                                    self.status_banner = format!(
                                        "Assigned hero #{} to {:?} slot {}",
                                        hero_id.0,
                                        self.selected_side,
                                        self.selected_slot + 1
                                    );
                                    self.search_query.clear();
                                }
                                Err(e) => self.status_banner = format!("Rejected: {e}"),
                            }
                        } else {
                            self.status_banner =
                                format!("No hero matching '{}'", self.search_query);
                        }
                    }
                });

                ui.label(RichText::new(&self.status_banner).color(dim));
                ui.separator();

                let recs_all = truesight_core::math::recommend_heroes(
                    &self.session.draft,
                    &self.session.matrix,
                    &self.session.weights,
                    25,
                );

                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("recs_grid")
                        .striped(true)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("Hero").strong());
                            ui.label(RichText::new("Proj Win%").strong());
                            ui.label(RichText::new("Draft Gain").strong());
                            ui.label(RichText::new("Meta WR").strong());
                            ui.label(RichText::new("vs Enemy").strong());
                            ui.label(RichText::new("w/ Allies").strong());
                            ui.label(RichText::new("Comfort").strong());
                            ui.end_row();

                            for rec in &recs_all {
                                let name = self
                                    .session
                                    .matrix
                                    .hero(rec.hero_id)
                                    .map(|h| h.display_name.as_str())
                                    .unwrap_or("Unknown");
                                if !self.search_query.is_empty()
                                    && !name
                                        .to_ascii_lowercase()
                                        .contains(&self.search_query.to_ascii_lowercase())
                                {
                                    continue;
                                }

                                if ui.button(name).clicked() {
                                    let pick = Some(DraftPick {
                                        hero_id: rec.hero_id,
                                        role: Role::ALL.get(self.selected_slot).copied(),
                                    });
                                    let _ = match self.selected_side {
                                        Side::Allies => {
                                            self.session.draft.set_ally(self.selected_slot, pick)
                                        }
                                        Side::Enemies => {
                                            self.session.draft.set_enemy(self.selected_slot, pick)
                                        }
                                    };
                                }

                                let gain = rec.net_probability_delta * 100.0;
                                let cnt = rec.counter_delta_sum * 100.0;
                                let syn = rec.synergy_delta_sum * 100.0;
                                let com = rec.comfort_delta * 100.0;

                                ui.label(format!("{:5.1}%", rec.projected_win_probability * 100.0));
                                ui.label(
                                    RichText::new(format!("{gain:+5.2}%"))
                                        .strong()
                                        .color(if gain >= 0.0 { green } else { red }),
                                );
                                ui.label(format!("{:5.1}%", rec.base_meta_wr * 100.0));
                                ui.label(
                                    RichText::new(format!("{cnt:+5.2}%"))
                                        .color(if cnt >= 0.0 { green } else { red }),
                                );
                                ui.label(
                                    RichText::new(format!("{syn:+5.2}%"))
                                        .color(if syn >= 0.0 { green } else { red }),
                                );
                                ui.label(format!("{com:+5.2}%"));
                                ui.end_row();
                            }
                        });
                });
                let _ = recs;
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("5v5 Live Counter & Synergy Matrix");
            ui.label(
                RichText::new(
                    "Click any Ally (A1..A5) or Enemy (E1..E5) slot button to target it for quick-pick assignment.",
                )
                .color(dim),
            );
            ui.add_space(6.0);

            egui::Grid::new("matrix_5v5_grid")
                .striped(true)
                .min_col_width(92.0)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("ALLY \\ ENEMY").strong());
                    for j in 0..TEAM_SIZE {
                        let enemy_name = self.session.draft.enemies[j]
                            .and_then(|p| self.session.matrix.hero(p.hero_id))
                            .map(|h| format!("E{}: {}", j + 1, h.display_name))
                            .unwrap_or_else(|| format!("E{}: [Empty]", j + 1));
                        let selected =
                            self.selected_side == Side::Enemies && self.selected_slot == j;
                        if ui.selectable_label(selected, RichText::new(enemy_name).strong()).clicked()
                        {
                            self.selected_side = Side::Enemies;
                            self.selected_slot = j;
                        }
                    }
                    ui.label(RichText::new("Net Slot Edge").strong());
                    ui.end_row();

                    for i in 0..TEAM_SIZE {
                        let ally_name = self.session.draft.allies[i]
                            .and_then(|p| self.session.matrix.hero(p.hero_id))
                            .map(|h| format!("A{}: {}", i + 1, h.display_name))
                            .unwrap_or_else(|| format!("A{}: [Empty]", i + 1));
                        let selected =
                            self.selected_side == Side::Allies && self.selected_slot == i;
                        if ui.selectable_label(selected, RichText::new(ally_name).strong()).clicked()
                        {
                            self.selected_side = Side::Allies;
                            self.selected_slot = i;
                        }

                        for j in 0..TEAM_SIZE {
                            if let Some(delta) = analysis.counter_grid[i][j] {
                                let pct = delta * 100.0;
                                let c = if pct > 0.15 {
                                    green
                                } else if pct < -0.15 {
                                    red
                                } else {
                                    dim
                                };
                                ui.label(RichText::new(format!("{pct:+6.2}%")).strong().color(c));
                            } else {
                                ui.label(RichText::new("·").color(dim));
                            }
                        }

                        if let Some(net) = analysis.ally_slot_net_delta[i] {
                            let pct = net * 100.0;
                            ui.label(
                                RichText::new(format!("{pct:+6.2}%"))
                                    .strong()
                                    .color(if pct >= 0.0 { green } else { red }),
                            );
                        } else {
                            ui.label(RichText::new("—").color(dim));
                        }
                        ui.end_row();
                    }
                });

            ui.add_space(16.0);
            ui.separator();
            ui.heading("Allied Pairwise Synergy Matrix");

            egui::Grid::new("synergy_grid")
                .striped(true)
                .min_col_width(90.0)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("");
                    for k in 0..TEAM_SIZE {
                        ui.label(RichText::new(format!("Ally #{}", k + 1)).strong());
                    }
                    ui.end_row();

                    for i in 0..TEAM_SIZE {
                        let name_i = self.session.draft.allies[i]
                            .and_then(|p| self.session.matrix.hero(p.hero_id))
                            .map(|h| h.display_name.as_str())
                            .unwrap_or("Empty");
                        ui.label(RichText::new(format!("A{}: {name_i}", i + 1)).strong());
                        for k in 0..TEAM_SIZE {
                            if i == k {
                                ui.label(RichText::new("—").color(dim));
                            } else if let Some(syn) = analysis.ally_synergy_grid[i][k] {
                                let pct = syn * 100.0;
                                ui.label(
                                    RichText::new(format!("{pct:+6.2}%"))
                                        .strong()
                                        .color(if pct >= 0.0 { green } else { red }),
                                );
                            } else {
                                ui.label(RichText::new("·").color(dim));
                            }
                        }
                        ui.end_row();
                    }
                });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

pub fn run_gui(session: AppSession, gsi_listener: Option<BoundedGsiListener>) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1260.0, 760.0])
            .with_title("TrueSight Dota — 5v5 Live Draft Oracle"),
        ..Default::default()
    };
    eframe::run_native(
        "TrueSight Dota",
        options,
        Box::new(move |_cc| Ok(Box::new(TrueSightGuiApp::new(session, gsi_listener)))),
    )
    .map_err(|e| e.to_string())
}
