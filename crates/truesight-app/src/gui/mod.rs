mod matrix_card;
mod oracle_card;
mod suggested_table;
mod team_card;
pub mod theme;
mod top_bar;
pub mod viewmodel;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub use theme::{Tone, fmt_pct, fmt_val, tone_for_user};
#[cfg(test)]
pub use viewmodel::{compute_comp_diagnostic, compute_slot_breakdown, enemy_counter_col_avg};

use crate::AppSession;
use eframe::egui::{
    self, Align, Align2, CornerRadius, Layout, Margin, Rect, RichText, Stroke, Vec2, vec2,
};
use theme::{GuiTextureCache, P_GEM, P_GEM_DEEP, P_INDIGO_1, P_INDIGO_2, P_INDIGO_3, P_LINE, P_SILVER, P_VOID};
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_adapters::sync::{AccountLinkConfig, normalize_steam32_id, save_account_config};
use truesight_adapters::vision::BoundedVisionWorker;
use truesight_core::domain::{Side, TEAM_SIZE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixTab {
    Counter,
    Synergy,
    Comp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerTarget {
    Slot(Side, usize),
    Ban,
}


// =============================================================================
// Main Application State & eframe::App Implementation
// =============================================================================

pub struct TrueSightGuiApp {
    session: AppSession,
    gsi_listener: Option<BoundedGsiListener>,
    vision_worker: Option<BoundedVisionWorker>,
    textures: GuiTextureCache,
    style_configured: bool,
    matrix_tab: MatrixTab,
    picker_target: Option<PickerTarget>,
    picker_search: String,
    focus_picker_input: bool,
    drag_slot: Option<(Side, usize)>,
    suggest_filter: String,
    status_banner: String,
    account_editor_open: bool,
    account_input: String,
}

impl TrueSightGuiApp {
    pub fn new(
        session: AppSession,
        gsi_listener: Option<BoundedGsiListener>,
        vision_worker: Option<BoundedVisionWorker>,
    ) -> Self {
        let account_input = session
            .cache_mgr
            .player_account_id
            .map(|id| id.to_string())
            .unwrap_or_default();
        Self {
            session,
            gsi_listener,
            vision_worker,
            textures: GuiTextureCache::default(),
            style_configured: false,
            matrix_tab: MatrixTab::Counter,
            picker_target: None,
            picker_search: String::new(),
            focus_picker_input: false,
            drag_slot: None,
            suggest_filter: String::new(),
            status_banner:
                "Click any Ally (P1..P5) or Enemy (P1..P5) slot card, type hero name, and press Enter."
                    .into(),
            account_editor_open: false,
            account_input,
        }
    }

    fn configure_visuals(&mut self, ctx: &egui::Context) {
        if self.style_configured {
            return;
        }
        self.style_configured = true;
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = P_VOID;
        visuals.window_fill = P_INDIGO_1;
        visuals.extreme_bg_color = P_INDIGO_2;
        visuals.faint_bg_color = P_INDIGO_2;
        visuals.override_text_color = Some(P_SILVER);
        visuals.widgets.noninteractive.bg_fill = P_INDIGO_2;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, P_LINE);
        visuals.widgets.noninteractive.corner_radius = CornerRadius::same(2);
        visuals.widgets.inactive.bg_fill = P_INDIGO_2;
        visuals.widgets.inactive.weak_bg_fill = P_INDIGO_2;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, P_LINE);
        visuals.widgets.inactive.corner_radius = CornerRadius::same(2);
        visuals.widgets.hovered.bg_fill = P_INDIGO_3;
        visuals.widgets.hovered.weak_bg_fill = P_INDIGO_3;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, P_GEM);
        visuals.widgets.hovered.corner_radius = CornerRadius::same(2);
        visuals.widgets.active.bg_fill = P_GEM_DEEP;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, P_GEM);
        visuals.widgets.active.corner_radius = CornerRadius::same(2);
        visuals.selection.bg_fill = P_GEM;
        visuals.selection.stroke = Stroke::new(1.0_f32, P_VOID);
        ctx.set_visuals(visuals);
    }

}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub struct SlotTelemetry {
    pub row: Rect,
    pub portrait: Rect,
    pub mid: Rect,
    pub net: Rect,
    pub clear: Option<Rect>,
}

impl Default for SlotTelemetry {
    fn default() -> Self {
        Self {
            row: Rect::NOTHING,
            portrait: Rect::NOTHING,
            mid: Rect::NOTHING,
            net: Rect::NOTHING,
            clear: None,
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub struct LayoutRects {
    pub top_bar: Rect,
    pub allies: Rect,
    pub oracle: Rect,
    pub enemies: Rect,
    pub suggested: Rect,
    pub matrix: Rect,
    pub ally_slots: [SlotTelemetry; TEAM_SIZE],
    pub enemy_slots: [SlotTelemetry; TEAM_SIZE],
    pub matrix_cell_size: Vec2,
    pub matrix_h_gap: f32,
    pub matrix_v_gap: f32,
    pub matrix_bottom_y: f32,
    pub suggested_last_col_max_x: f32,
}

impl TrueSightGuiApp {
    pub fn render_frame(&mut self, ctx: &egui::Context) -> LayoutRects {
        self.configure_visuals(ctx);
        self.textures.ensure_loaded(ctx);

        self.session
            .poll_live_sources(self.gsi_listener.as_mut(), self.vision_worker.as_mut());

        let (analysis, _) = self.session.evaluate();
        let mut rects = LayoutRects {
            top_bar: Rect::NOTHING,
            allies: Rect::NOTHING,
            oracle: Rect::NOTHING,
            enemies: Rect::NOTHING,
            suggested: Rect::NOTHING,
            matrix: Rect::NOTHING,
            ally_slots: [SlotTelemetry::default(); TEAM_SIZE],
            enemy_slots: [SlotTelemetry::default(); TEAM_SIZE],
            matrix_cell_size: Vec2::ZERO,
            matrix_h_gap: 0.0,
            matrix_v_gap: 0.0,
            matrix_bottom_y: 0.0,
            suggested_last_col_max_x: 0.0,
        };

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(P_VOID).inner_margin(Margin::same(10)))
            .show(ctx, |ui| {
                // 1. Top Bar
                rects.top_bar = self.render_top_bar(ui);
                ui.add_space(10.0);

                // 2. Top Row: Allies (356px) | Oracle (flex) | Enemies (356px)
                let total_w = ui.available_width();
                let side_w = 356.0_f32.min(total_w * 0.275);
                let gap = 10.0;
                let center_w = (total_w - 2.0 * side_w - 2.0 * gap).max(360.0);
                let top_row_h = 368.0_f32;

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.allocate_ui_with_layout(
                        vec2(side_w, top_row_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            rects.allies = self.render_team_card(
                                ui,
                                &analysis,
                                Side::Allies,
                                &mut rects.ally_slots,
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(center_w, top_row_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            rects.oracle = self.render_oracle_card(ui, &analysis);
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(side_w, top_row_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            rects.enemies = self.render_team_card(
                                ui,
                                &analysis,
                                Side::Enemies,
                                &mut rects.enemy_slots,
                            );
                        },
                    );
                });

                ui.add_space(10.0);

                // 3. Bottom Row: Suggested Picks (flex) | 5v5 Matrix / Comp (square-coupled)
                let bottom_h = ui.available_height().max(320.0);
                let ideal_cell = ((bottom_h - 134.0) / 7.0).floor().clamp(32.0, 56.0);
                let matrix_w = (7.0 * ideal_cell + 6.0 * 4.0 + 20.0)
                    .min(total_w * 0.34)
                    .max(380.0);
                let recs_w = (total_w - matrix_w - gap).max(480.0);

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.allocate_ui_with_layout(
                        vec2(recs_w, bottom_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            let (s_rect, last_x) = self.render_suggested_picks_card(ui);
                            rects.suggested = s_rect;
                            rects.suggested_last_col_max_x = last_x;
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(matrix_w, bottom_h),
                        Layout::top_down(Align::Min),
                        |ui| {
                            let (m_rect, cell_sz, h_gap, v_gap, bottom_y) =
                                self.render_matrix_card(ui, &analysis);
                            rects.matrix = m_rect;
                            rects.matrix_cell_size = cell_sz;
                            rects.matrix_h_gap = h_gap;
                            rects.matrix_v_gap = v_gap;
                            rects.matrix_bottom_y = bottom_y;
                        },
                    );
                });
            });

        if self.drag_slot.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            if ctx.input(|i| i.pointer.any_released()) {
                self.drag_slot = None;
            }
        }

        if self.account_editor_open {
            let mut close_modal = false;
            egui::Window::new("STEAM ACCOUNT LINK")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_TOP, vec2(0.0, 62.0))
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("Enter Steam32 or Steam64 ID (leave Auto for GSI + Steam VDF):")
                            .size(11.0)
                            .color(P_SILVER),
                    );
                    ui.add_space(4.0);
                    let enter_pressed = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.account_input)
                                .desired_width(240.0)
                                .hint_text("e.g. Steam32 or 7656119..."),
                        )
                        .lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("APPLY").clicked() || enter_pressed {
                            if let Ok(raw) = self.account_input.trim().parse::<u64>() {
                                if let Some(s32) = normalize_steam32_id(raw) {
                                    self.session.set_account_id(Some(s32), true);
                                    let _ = save_account_config(AccountLinkConfig::Explicit(s32));
                                    self.status_banner = format!("Linked Steam32 account #{s32}.");
                                    close_modal = true;
                                } else {
                                    self.status_banner = "Invalid Steam32/Steam64 account ID.".into();
                                }
                            } else {
                                self.status_banner = "Enter a numeric Steam32 or Steam64 ID.".into();
                            }
                        }
                        if ui.button("AUTO (VDF)").clicked() {
                            self.session.set_account_auto();
                            let _ = save_account_config(AccountLinkConfig::Auto);
                            self.status_banner = format!("Auto account mode: {}.", self.session.db_status);
                            close_modal = true;
                        }
                        if ui.button("UNLINK").clicked() {
                            self.session.set_account_id(None, true);
                            let _ = save_account_config(AccountLinkConfig::Unlinked);
                            self.status_banner = "Account unlinked (neutral 0.0% comfort).".into();
                            close_modal = true;
                        }
                        if ui.button("CLOSE").clicked() {
                            close_modal = true;
                        }
                    });
                });
            if close_modal {
                self.account_editor_open = false;
            }
        }

        rects
    }
}

impl eframe::App for TrueSightGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _ = self.render_frame(ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

pub fn run_gui(
    session: AppSession,
    gsi_listener: Option<BoundedGsiListener>,
    vision_worker: Option<BoundedVisionWorker>,
) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1468.0, 960.0])
            .with_title("TrueSight Dota — 5v5 Live Draft Oracle"),
        ..Default::default()
    };
    eframe::run_native(
        "TrueSight Dota",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(TrueSightGuiApp::new(
                session,
                gsi_listener,
                vision_worker,
            )))
        }),
    )
    .map_err(|e| e.to_string())
}

