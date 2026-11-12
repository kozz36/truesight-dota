use crate::AppSession;
use eframe::egui::{self, Color32, RichText};
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_core::domain::{Bracket, DraftPick, HeroId, Role, TEAM_SIZE};
use truesight_core::ports::DraftEventSource;

pub struct TrueSightGuiApp {
    session: AppSession,
    gsi_listener: Option<BoundedGsiListener>,
    ally_search: [String; TEAM_SIZE],
    enemy_search: [String; TEAM_SIZE],
    suggest_filter: String,
    status_banner: String,
}

impl TrueSightGuiApp {
    pub fn new(session: AppSession, gsi_listener: Option<BoundedGsiListener>) -> Self {
        Self {
            session,
            gsi_listener,
            ally_search: Default::default(),
            enemy_search: Default::default(),
            suggest_filter: String::new(),
            status_banner:
                "Click any Ally (A1..A5) or Enemy (E1..E5) dropdown to pick from all 127 heroes independently."
                    .into(),
        }
    }

    fn all_heroes_sorted(&self) -> Vec<(HeroId, String)> {
        let mut list: Vec<(HeroId, String)> = self
            .session
            .matrix
            .heroes()
            .map(|h| (h.id, h.display_name.clone()))
            .collect();
        list.sort_by(|a, b| a.1.cmp(&b.1));
        list
    }
}

#[inline]
fn delta_color(pct: f32, green: Color32, red: Color32, neutral: Color32) -> Color32 {
    if pct > 0.05 {
        green
    } else if pct < -0.05 {
        red
    } else {
        neutral
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

        let (analysis, _top_recs) = self.session.evaluate();
        let all_heroes = self.all_heroes_sorted();

        let green = Color32::from_rgb(46, 204, 113);
        let red = Color32::from_rgb(231, 76, 60);
        let gold = Color32::from_rgb(241, 196, 15);
        let cyan = Color32::from_rgb(52, 152, 219);
        let dim = Color32::from_rgb(149, 165, 166);

        // =========================================================================
        // 1. TOP HEADER PANEL: Win Probability Thermometer + Global Controls
        // =========================================================================
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
                        let _ = crate::execute_command(&mut self.session, "ally 1 faceless void");
                        let _ = crate::execute_command(&mut self.session, "ally 3 magnus");
                        let _ = crate::execute_command(&mut self.session, "ally 5 crystal maiden");
                        let _ = crate::execute_command(&mut self.session, "enemy 1 medusa");
                        let _ = crate::execute_command(&mut self.session, "enemy 2 storm spirit");
                        let _ = crate::execute_command(&mut self.session, "enemy 3 axe");
                        self.status_banner =
                            "Loaded Demo 5v5 (Faceless Void + Magnus + CM vs Medusa + Storm + Axe)."
                                .into();
                    }
                });
            });

            ui.separator();

            let ally_pct = analysis.ally_win_probability * 100.0;
            let enemy_pct = 100.0 - ally_pct;
            let win_color = delta_color(ally_pct - 50.0, green, red, gold);

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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for bracket in Bracket::ALL.into_iter().rev() {
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
                    ui.label(RichText::new("Bracket:").color(cyan));
                });
            });

            ui.add(
                egui::ProgressBar::new(analysis.ally_win_probability.clamp(0.0, 1.0))
                    .fill(win_color)
                    .desired_height(9.0),
            );
            ui.label(RichText::new(&self.status_banner).small().color(dim));
        });

        // =========================================================================
        // 2. DEDICATED BOTTOM SECTION: Suggested Picks & Mathematical Breakdown
        // =========================================================================
        egui::TopBottomPanel::bottom("suggestions_section")
            .resizable(true)
            .default_height(285.0)
            .min_height(190.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        RichText::new(format!(
                            "Suggested Heroes Section — {}",
                            self.session.draft.target_role.label()
                        ))
                        .color(gold),
                    );
                    ui.separator();
                    ui.label("Role Filter:");
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
                    ui.separator();
                    ui.label("Filter Name:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.suggest_filter)
                            .desired_width(140.0)
                            .hint_text("e.g. anti, jugg..."),
                    );
                    if !self.suggest_filter.is_empty() && ui.small_button("×").clicked() {
                        self.suggest_filter.clear();
                    }
                });

                ui.separator();

                let recs_all = truesight_core::math::recommend_heroes(
                    &self.session.draft,
                    &self.session.matrix,
                    &self.session.weights,
                    40,
                );
                let target_slot_idx = self.session.draft.target_role.index();

                egui::ScrollArea::vertical()
                    .id_salt("bottom_suggestions_scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("bottom_recs_grid")
                            .striped(true)
                            .min_col_width(95.0)
                            .spacing([14.0, 6.0])
                            .show(ui, |ui| {
                                ui.label(RichText::new("Rank & Hero").strong());
                                ui.label(RichText::new("Action").strong());
                                ui.label(RichText::new("Projected Win%").strong());
                                ui.label(
                                    RichText::new(format!(
                                        "Draft Gain (A{})",
                                        target_slot_idx + 1
                                    ))
                                    .strong(),
                                );
                                ui.label(RichText::new("Role Meta WR").strong());
                                ui.label(RichText::new("vs Picked Enemies").strong());
                                ui.label(RichText::new("w/ Picked Allies").strong());
                                ui.label(RichText::new("Personal Comfort").strong());
                                ui.end_row();

                                let filter_lower = self.suggest_filter.to_ascii_lowercase();
                                for (idx, rec) in recs_all.iter().enumerate() {
                                    let name = self
                                        .session
                                        .matrix
                                        .hero(rec.hero_id)
                                        .map(|h| h.display_name.as_str())
                                        .unwrap_or("Unknown");
                                    if !filter_lower.is_empty()
                                        && !name.to_ascii_lowercase().contains(&filter_lower)
                                    {
                                        continue;
                                    }

                                    ui.label(
                                        RichText::new(format!("#{:>2}  {}", idx + 1, name)).strong(),
                                    );
                                    if ui
                                        .small_button(format!("Pick into A{}", target_slot_idx + 1))
                                        .clicked()
                                    {
                                        let _ = self.session.draft.set_ally(
                                            target_slot_idx,
                                            Some(DraftPick {
                                                hero_id: rec.hero_id,
                                                role: Some(self.session.draft.target_role),
                                            }),
                                        );
                                        self.status_banner = format!(
                                            "Picked {} into Ally Slot A{}.",
                                            name,
                                            target_slot_idx + 1
                                        );
                                    }

                                    let proj = rec.projected_win_probability * 100.0;
                                    let gain = rec.net_probability_delta * 100.0;
                                    let cnt = rec.counter_delta_sum * 100.0;
                                    let syn = rec.synergy_delta_sum * 100.0;
                                    let com = rec.comfort_delta * 100.0;

                                    ui.label(
                                        RichText::new(format!("{proj:5.1}%"))
                                            .strong()
                                            .color(delta_color(proj - 50.0, green, red, dim)),
                                    );
                                    ui.label(
                                        RichText::new(format!("{gain:+5.2}%"))
                                            .strong()
                                            .color(delta_color(gain, green, red, dim)),
                                    );
                                    ui.label(format!("{:5.1}%", rec.base_meta_wr * 100.0));
                                    ui.label(
                                        RichText::new(format!("{cnt:+5.2}%"))
                                            .color(delta_color(cnt, green, red, dim)),
                                    );
                                    ui.label(
                                        RichText::new(format!("{syn:+5.2}%"))
                                            .color(delta_color(syn, green, red, dim)),
                                    );
                                    ui.label(
                                        RichText::new(format!("{com:+5.2}%"))
                                            .color(delta_color(com, green, red, dim)),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
            });

        // =========================================================================
        // 3. CENTRAL PANEL: Independent Per-Slot Hero Selectors + 5v5 Live Matrices
        // =========================================================================
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("central_matrices_scroll")
                .show(ui, |ui| {
                    // --- 3A. INDEPENDENT 10-SLOT HERO PICKER BAR (ALL 127 HEROES) ---
                    ui.heading("Independent 5v5 Slot Pickers (All 127 Heroes)");
                    ui.add_space(4.0);

                    egui::Grid::new("independent_slot_pickers")
                        .striped(true)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("ALLIES").color(green).strong());
                            for i in 0..TEAM_SIZE {
                                let current_name = self.session.draft.allies[i]
                                    .and_then(|p| self.session.matrix.hero(p.hero_id))
                                    .map(|h| h.display_name.clone())
                                    .unwrap_or_else(|| format!("A{} [Select Hero]", i + 1));

                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt(format!("ally_combo_{i}"))
                                        .selected_text(format!("A{}: {}", i + 1, current_name))
                                        .width(155.0)
                                        .show_ui(ui, |ui| {
                                            ui.text_edit_singleline(&mut self.ally_search[i]);
                                            let q = self.ally_search[i].to_ascii_lowercase();
                                            if ui.selectable_label(false, "— [Clear Slot] —").clicked()
                                            {
                                                let _ = self.session.draft.set_ally(i, None);
                                            }
                                            for &(hid, ref hname) in &all_heroes {
                                                if !q.is_empty()
                                                    && !hname.to_ascii_lowercase().contains(&q)
                                                {
                                                    continue;
                                                }
                                                let unavailable =
                                                    self.session.draft.is_unavailable(hid);
                                                let is_cur = self.session.draft.allies[i]
                                                    .map(|p| p.hero_id == hid)
                                                    .unwrap_or(false);
                                                if (!unavailable || is_cur)
                                                    && ui.selectable_label(is_cur, hname).clicked()
                                                {
                                                    let _ = self.session.draft.set_ally(
                                                        i,
                                                        Some(DraftPick {
                                                            hero_id: hid,
                                                            role: Role::ALL.get(i).copied(),
                                                        }),
                                                    );
                                                    self.ally_search[i].clear();
                                                }
                                            }
                                        });
                                    if self.session.draft.allies[i].is_some()
                                        && ui.small_button("×").clicked()
                                    {
                                        let _ = self.session.draft.set_ally(i, None);
                                    }
                                });
                            }
                            ui.end_row();

                            ui.label(RichText::new("ENEMIES").color(red).strong());
                            for j in 0..TEAM_SIZE {
                                let current_name = self.session.draft.enemies[j]
                                    .and_then(|p| self.session.matrix.hero(p.hero_id))
                                    .map(|h| h.display_name.clone())
                                    .unwrap_or_else(|| format!("E{} [Select Hero]", j + 1));

                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt(format!("enemy_combo_{j}"))
                                        .selected_text(format!("E{}: {}", j + 1, current_name))
                                        .width(155.0)
                                        .show_ui(ui, |ui| {
                                            ui.text_edit_singleline(&mut self.enemy_search[j]);
                                            let q = self.enemy_search[j].to_ascii_lowercase();
                                            if ui.selectable_label(false, "— [Clear Slot] —").clicked()
                                            {
                                                let _ = self.session.draft.set_enemy(j, None);
                                            }
                                            for &(hid, ref hname) in &all_heroes {
                                                if !q.is_empty()
                                                    && !hname.to_ascii_lowercase().contains(&q)
                                                {
                                                    continue;
                                                }
                                                let unavailable =
                                                    self.session.draft.is_unavailable(hid);
                                                let is_cur = self.session.draft.enemies[j]
                                                    .map(|p| p.hero_id == hid)
                                                    .unwrap_or(false);
                                                if (!unavailable || is_cur)
                                                    && ui.selectable_label(is_cur, hname).clicked()
                                                {
                                                    let _ = self.session.draft.set_enemy(
                                                        j,
                                                        Some(DraftPick {
                                                            hero_id: hid,
                                                            role: Role::ALL.get(j).copied(),
                                                        }),
                                                    );
                                                    self.enemy_search[j].clear();
                                                }
                                            }
                                        });
                                    if self.session.draft.enemies[j].is_some()
                                        && ui.small_button("×").clicked()
                                    {
                                        let _ = self.session.draft.set_enemy(j, None);
                                    }
                                });
                            }
                            ui.end_row();
                        });

                    ui.add_space(10.0);
                    ui.separator();

                    // --- 3B. SIDE-BY-SIDE 5v5 COUNTER MATRIX & ALLIED SYNERGY MATRIX ---
                    ui.columns(2, |cols| {
                        // Left Column: 5v5 Counter Matrix (Allies vs Enemies)
                        cols[0].heading("5v5 Counter Matrix (Allies vs Enemies)");
                        cols[0].add_space(4.0);
                        egui::Grid::new("matrix_5v5_grid")
                            .striped(true)
                            .min_col_width(82.0)
                            .spacing([8.0, 7.0])
                            .show(&mut cols[0], |ui| {
                                ui.label(RichText::new("ALLY \\ ENEMY").strong());
                                for j in 0..TEAM_SIZE {
                                    let enemy_name = self.session.draft.enemies[j]
                                        .and_then(|p| self.session.matrix.hero(p.hero_id))
                                        .map(|h| format!("E{}: {}", j + 1, h.display_name))
                                        .unwrap_or_else(|| format!("E{}: Empty", j + 1));
                                    ui.label(RichText::new(enemy_name).strong().color(red));
                                }
                                ui.label(RichText::new("Avg Counter").strong());
                                ui.end_row();

                                for i in 0..TEAM_SIZE {
                                    let ally_name = self.session.draft.allies[i]
                                        .and_then(|p| self.session.matrix.hero(p.hero_id))
                                        .map(|h| format!("A{}: {}", i + 1, h.display_name))
                                        .unwrap_or_else(|| format!("A{}: Empty", i + 1));
                                    ui.label(RichText::new(ally_name).strong().color(green));

                                    for j in 0..TEAM_SIZE {
                                        if let Some(delta) = analysis.counter_grid[i][j] {
                                            let pct = delta * 100.0;
                                            let c = delta_color(pct, green, red, dim);
                                            ui.label(
                                                RichText::new(format!("{pct:+6.2}%"))
                                                    .strong()
                                                    .color(c),
                                            );
                                        } else {
                                            ui.label(RichText::new("·").color(dim));
                                        }
                                    }

                                    if let Some(avg_c) = analysis.ally_counter_avg[i] {
                                        let pct = avg_c * 100.0;
                                        ui.label(
                                            RichText::new(format!("{pct:+6.2}%"))
                                                .strong()
                                                .color(delta_color(pct, green, red, dim)),
                                        );
                                    } else {
                                        ui.label(RichText::new("—").color(dim));
                                    }
                                    ui.end_row();
                                }
                            });

                        // Right Column: 5v5 Allied Pairwise Synergy Matrix
                        cols[1].heading("Allied Pairwise Synergy Matrix");
                        cols[1].add_space(4.0);
                        egui::Grid::new("synergy_grid")
                            .striped(true)
                            .min_col_width(82.0)
                            .spacing([8.0, 7.0])
                            .show(&mut cols[1], |ui| {
                                ui.label(RichText::new("ALLY \\ ALLY").strong());
                                for k in 0..TEAM_SIZE {
                                    let col_name = self.session.draft.allies[k]
                                        .and_then(|p| self.session.matrix.hero(p.hero_id))
                                        .map(|h| format!("A{}: {}", k + 1, h.display_name))
                                        .unwrap_or_else(|| format!("A{}: Empty", k + 1));
                                    ui.label(RichText::new(col_name).strong().color(cyan));
                                }
                                ui.label(RichText::new("Avg Synergy").strong());
                                ui.end_row();

                                for i in 0..TEAM_SIZE {
                                    let row_name = self.session.draft.allies[i]
                                        .and_then(|p| self.session.matrix.hero(p.hero_id))
                                        .map(|h| format!("A{}: {}", i + 1, h.display_name))
                                        .unwrap_or_else(|| format!("A{}: Empty", i + 1));
                                    ui.label(RichText::new(row_name).strong().color(cyan));

                                    for k in 0..TEAM_SIZE {
                                        if i == k {
                                            ui.label(RichText::new("—").color(dim));
                                        } else if let Some(syn) = analysis.ally_synergy_grid[i][k] {
                                            let pct = syn * 100.0;
                                            let c = delta_color(pct, green, red, dim);
                                            ui.label(
                                                RichText::new(format!("{pct:+6.2}%"))
                                                    .strong()
                                                    .color(c),
                                            );
                                        } else {
                                            ui.label(RichText::new("·").color(dim));
                                        }
                                    }

                                    if let Some(avg_s) = analysis.ally_synergy_avg[i] {
                                        let pct = avg_s * 100.0;
                                        ui.label(
                                            RichText::new(format!("{pct:+6.2}%"))
                                                .strong()
                                                .color(delta_color(pct, green, red, dim)),
                                        );
                                    } else {
                                        ui.label(RichText::new("—").color(dim));
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

pub fn run_gui(session: AppSession, gsi_listener: Option<BoundedGsiListener>) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 820.0])
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
