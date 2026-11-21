mod build_bar;
mod matrix_card;
mod onboarding;
mod oracle_card;
mod overlay;
mod suggested_table;
mod team_card;
#[cfg(test)]
mod tests;
pub mod theme;
mod top_bar;
pub mod viewmodel;

#[cfg(test)]
pub use theme::{Tone, fmt_pct, fmt_val, tone_for_user};
#[cfg(test)]
pub use viewmodel::{compute_comp_diagnostic, compute_slot_breakdown, enemy_counter_col_avg};

use crate::AppSession;
use eframe::egui::{
    self, Align, Align2, CornerRadius, Layout, Margin, Rect, RichText, Stroke, Vec2, vec2,
};
use theme::{
    GuiTextureCache, P_GEM, P_GEM_DEEP, P_INDIGO_1, P_INDIGO_2, P_INDIGO_3, P_LINE, P_SILVER,
    P_VOID, hud_modal_frame, paint_hud_corner_ticks,
};
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_adapters::sync::{AccountLinkConfig, normalize_steam32_id, save_account_config};
use truesight_adapters::vision::BoundedVisionWorker;
use truesight_core::domain::{Role, Side, TEAM_SIZE};
use truesight_core::math::DraftAnalysis5v5;

const MIN_UNSCROLLED_W: f32 = 832.0;
const MIN_UNSCROLLED_H: f32 = 640.0;

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
    options_open: bool,
    account_editor_open: bool,
    account_input: String,
    pub(super) enemy_scout_role: Option<Role>,
    pub(super) onboarding_open: bool,
    pub(super) onboarding_dismissed: bool,
    pub(super) onboarding_step: usize,
    pub(super) stratz_clipboard_watch_until: Option<u64>,
    pub(super) stratz_clipboard_rx: Option<std::sync::mpsc::Receiver<String>>,
    pub(super) stratz_clipboard_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub(super) stratz_clipboard_baseline: Option<String>,
    pub(super) stratz_paste_rx: Option<std::sync::mpsc::Receiver<Option<String>>>,
    pub(super) stratz_paste_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(test)]
    pub(super) test_clipboard_override: Option<String>,
    pub(super) stratz_paste_open: bool,
    pub(super) stratz_token_input: String,
}

impl Drop for TrueSightGuiApp {
    fn drop(&mut self) {
        self.stop_stratz_clipboard_watch();
        if let Some(stop) = self.stratz_paste_stop.take() {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
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
        let status_banner = if vision_worker.is_none() && !session.vision_detail.is_empty() {
            session.vision_detail.clone()
        } else {
            "Click any Ally (P1..P5) or Enemy (P1..P5) slot card, type hero name, and press Enter."
                .into()
        };
        let onboarding_dismissed = session.onboarding_dismissed;
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
            status_banner,
            options_open: false,
            account_editor_open: false,
            account_input,
            enemy_scout_role: None,
            onboarding_open: !onboarding_dismissed,
            onboarding_dismissed,
            onboarding_step: 0,
            stratz_clipboard_watch_until: None,
            stratz_clipboard_rx: None,
            stratz_clipboard_stop: None,
            stratz_clipboard_baseline: None,
            stratz_paste_rx: None,
            stratz_paste_stop: None,
            #[cfg(test)]
            test_clipboard_override: None,
            stratz_paste_open: false,
            stratz_token_input: String::new(),
        }
    }

    pub(super) fn apply_account_input(&mut self) -> Option<AccountLinkConfig> {
        let Ok(raw) = self.account_input.trim().parse::<u64>() else {
            self.status_banner = "Enter a numeric Steam32 or Steam64 ID.".into();
            return None;
        };
        let Some(s32) = normalize_steam32_id(raw) else {
            self.status_banner = "Invalid Steam32/Steam64 account ID.".into();
            return None;
        };
        self.session.set_account_id(Some(s32), true);
        let syncing = self.session.sync_if_player_stale();
        self.status_banner = if syncing || self.session.sync_in_flight {
            format!("Linked Steam32 account #{s32} (syncing player cache...).")
        } else {
            format!("Linked Steam32 account #{s32}.")
        };
        Some(AccountLinkConfig::Explicit(s32))
    }

    pub(super) fn apply_auto_account(&mut self) -> AccountLinkConfig {
        self.session.set_account_auto();
        let _ = self.session.sync_if_player_stale();
        self.status_banner = format!("Auto account mode: {}.", self.session.db_status);
        AccountLinkConfig::Auto
    }

    pub(super) fn active_enemy_scout_role(&self) -> Role {
        if let Some(role) = self.enemy_scout_role {
            return role;
        }
        (0..TEAM_SIZE)
            .find(|&idx| self.session.draft.enemies[idx].is_none())
            .and_then(|idx| Role::ALL.get(idx).copied())
            .unwrap_or(Role::Carry)
    }

    fn configure_visuals(&mut self, ctx: &egui::Context) {
        if self.style_configured {
            return;
        }
        self.style_configured = true;
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = P_VOID;
        visuals.window_fill = P_INDIGO_1;
        visuals.window_corner_radius = CornerRadius::ZERO;
        visuals.window_stroke = Stroke::new(1.0_f32, P_LINE);
        visuals.window_shadow = egui::Shadow::NONE;
        visuals.extreme_bg_color = P_INDIGO_2;
        visuals.faint_bg_color = P_INDIGO_2;
        visuals.override_text_color = Some(P_SILVER);
        visuals.widgets.noninteractive.bg_fill = P_INDIGO_2;
        visuals.widgets.noninteractive.weak_bg_fill = P_INDIGO_1;
        visuals.widgets.open.weak_bg_fill = P_INDIGO_2;
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
    pub mid_content_bottom: f32,
    pub net: Rect,
    pub clear: Option<Rect>,
}

impl Default for SlotTelemetry {
    fn default() -> Self {
        Self {
            row: Rect::NOTHING,
            portrait: Rect::NOTHING,
            mid: Rect::NOTHING,
            mid_content_bottom: 0.0,
            net: Rect::NOTHING,
            clear: None,
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct LayoutRects {
    pub top_bar: Rect,
    pub options_button: Rect,
    pub info_button: Rect,
    pub reset_button: Rect,
    pub bracket_bar: Rect,
    pub options_window: Option<Rect>,
    pub options_demo_button: Option<Rect>,
    pub onboarding_window: Option<Rect>,
    pub account_window: Option<Rect>,
    pub account_apply_button: Option<Rect>,
    pub account_close_button: Option<Rect>,
    pub allies: Rect,
    pub oracle: Rect,
    pub enemies: Rect,
    pub suggested: Rect,
    pub matrix: Rect,
    pub ally_slots: [SlotTelemetry; TEAM_SIZE],
    pub enemy_slots: [SlotTelemetry; TEAM_SIZE],
    pub enemy_scout_cards: Vec<Rect>,
    pub matrix_cell_size: Vec2,
    pub matrix_grid_rect: Rect,
    pub matrix_h_gap: f32,
    pub matrix_v_gap: f32,
    pub matrix_bottom_y: f32,
    pub docked_comp: Option<Rect>,
    pub comp_enemies_hdr: Rect,
    pub suggested_last_col_max_x: f32,
    pub suggested_comfort_hdr_max_x: f32,
    pub suggested_row_rects: Vec<Rect>,
    pub suggested_vs_row_rects: Vec<Rect>,
    pub suggested_pick_rects: Vec<Rect>,
    pub suggested_pick_stripe_rects: Vec<Option<Rect>>,
    pub scrollable_fallback: bool,
}

impl TrueSightGuiApp {
    pub fn render_frame(&mut self, ctx: &egui::Context) -> LayoutRects {
        self.configure_visuals(ctx);
        self.textures.ensure_loaded(ctx);

        self.session
            .poll_live_sources(self.gsi_listener.as_mut(), self.vision_worker.as_mut());
        self.poll_stratz_clipboard_watcher();
        self.poll_stratz_clipboard_paste();
        if self
            .is_watching_stratz_clipboard(truesight_adapters::sync::LocalCacheManager::now_unix())
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        if self.stratz_paste_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(notice) = self.session.last_sync_notice.take() {
            self.status_banner = notice;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F8)) {
            let next = !self.session.overlay_mode;
            self.session.set_overlay_mode(next);
        }

        let (analysis, _) = self.session.evaluate();
        let mut rects = LayoutRects {
            top_bar: Rect::NOTHING,
            options_button: Rect::NOTHING,
            info_button: Rect::NOTHING,
            reset_button: Rect::NOTHING,
            bracket_bar: Rect::NOTHING,
            options_window: None,
            options_demo_button: None,
            onboarding_window: None,
            account_window: None,
            account_apply_button: None,
            account_close_button: None,
            allies: Rect::NOTHING,
            oracle: Rect::NOTHING,
            enemies: Rect::NOTHING,
            suggested: Rect::NOTHING,
            matrix: Rect::NOTHING,
            ally_slots: [SlotTelemetry::default(); TEAM_SIZE],
            enemy_slots: [SlotTelemetry::default(); TEAM_SIZE],
            enemy_scout_cards: Vec::new(),
            matrix_cell_size: Vec2::ZERO,
            matrix_grid_rect: Rect::NOTHING,
            matrix_h_gap: 0.0,
            matrix_v_gap: 0.0,
            matrix_bottom_y: 0.0,
            docked_comp: None,
            comp_enemies_hdr: Rect::NOTHING,
            suggested_last_col_max_x: 0.0,
            suggested_comfort_hdr_max_x: 0.0,
            suggested_row_rects: Vec::new(),
            suggested_vs_row_rects: Vec::new(),
            suggested_pick_rects: Vec::new(),
            suggested_pick_stripe_rects: Vec::new(),
            scrollable_fallback: false,
        };

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(P_VOID)
                    .inner_margin(Margin::same(10)),
            )
            .show(ctx, |ui| {
                let avail_w = ui.available_width();
                let avail_h = ui.available_height();
                let needs_h_scroll = avail_w < MIN_UNSCROLLED_W;
                let needs_v_scroll = avail_h < MIN_UNSCROLLED_H;
                let needs_scroll = needs_h_scroll || needs_v_scroll;
                rects.scrollable_fallback = needs_scroll;

                let layout_w = avail_w.max(MIN_UNSCROLLED_W);
                let layout_h = avail_h.max(MIN_UNSCROLLED_H);
                if needs_scroll {
                    egui::ScrollArea::new([needs_h_scroll, needs_v_scroll])
                        .id_salt("truesight_viewport_fallback_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.allocate_ui_with_layout(
                                vec2(layout_w, layout_h),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    self.render_workspace_body(
                                        ui, &analysis, &mut rects, layout_w, layout_h,
                                    );
                                },
                            );
                        });
                } else {
                    self.render_workspace_body(ui, &analysis, &mut rects, layout_w, layout_h);
                }
            });

        let (opts_win, opts_demo) = self.render_options_window(ctx, rects.top_bar.max.y);
        rects.options_window = opts_win;
        rects.options_demo_button = opts_demo;
        rects.onboarding_window = self.render_onboarding_window(ctx);

        if self.drag_slot.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            if ctx.input(|i| i.pointer.any_released()) {
                self.drag_slot = None;
            }
        }

        if self.account_editor_open {
            let mut close_modal = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            let screen = ctx.screen_rect();
            let top_anchor_y =
                (rects.top_bar.max.y + 4.0).clamp(14.0, (screen.height() - 120.0).max(14.0));
            let (anchor_offset, max_acct_w, input_w) = if let Some(opts_rect) = rects.options_window
            {
                let left_space = opts_rect.min.x - screen.left() - 20.0;
                if left_space >= 260.0 {
                    let mw = (left_space - 22.0).clamp(238.0, 340.0);
                    let iw = if mw >= 310.0 {
                        240.0
                    } else {
                        (mw - 40.0).clamp(160.0, 240.0)
                    };
                    (
                        vec2(
                            opts_rect.min.x - screen.right() - 8.0,
                            opts_rect.min.y - screen.top(),
                        ),
                        mw,
                        iw,
                    )
                } else {
                    let mw = (screen.width() - 24.0).clamp(240.0, 340.0);
                    (
                        vec2(-12.0, opts_rect.max.y + 8.0 - screen.top()),
                        mw,
                        240.0_f32.min((mw - 40.0).max(160.0)),
                    )
                }
            } else {
                let mw = (screen.width() - 24.0).clamp(260.0, 340.0);
                (vec2(-12.0, top_anchor_y - screen.top()), mw, 240.0)
            };

            let acct_resp = egui::Window::new("STEAM ACCOUNT LINK")
                .collapsible(false)
                .resizable(false)
                .fade_in(false)
                .anchor(Align2::RIGHT_TOP, anchor_offset)
                .constrain_to(screen)
                .frame(hud_modal_frame(ctx))
                .max_width(max_acct_w)
                .show(ctx, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "Enter Steam32 or Steam64 ID (leave Auto for GSI + Steam VDF):",
                            )
                            .size(11.0)
                            .color(P_SILVER),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.0);
                    let enter_pressed = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.account_input)
                                .desired_width(input_w)
                                .hint_text("e.g. Steam32 or 7656119..."),
                        )
                        .lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(6.0);
                    let mut apply_rect = None;
                    let mut close_rect = None;
                    ui.horizontal(|ui| {
                        let apply_resp = ui.button("APPLY");
                        apply_rect = Some(apply_resp.rect);
                        if apply_resp.clicked() || enter_pressed {
                            if let Some(cfg) = self.apply_account_input() {
                                let _ = save_account_config(cfg);
                                close_modal = true;
                            }
                        }
                        if ui.button("AUTO (VDF)").clicked() {
                            let cfg = self.apply_auto_account();
                            let _ = save_account_config(cfg);
                            close_modal = true;
                        }
                        if ui.button("UNLINK").clicked() {
                            self.session.set_account_id(None, true);
                            let _ = save_account_config(AccountLinkConfig::Unlinked);
                            self.status_banner = "Account unlinked (neutral 0.0% comfort).".into();
                            close_modal = true;
                        }
                        let close_resp = ui.button("CLOSE");
                        close_rect = Some(close_resp.rect);
                        if close_resp.clicked() {
                            close_modal = true;
                        }
                    });
                    (apply_rect, close_rect)
                });
            if close_modal {
                self.account_editor_open = false;
            }
            if let Some(r) = &acct_resp {
                let painter = ctx.layer_painter(r.response.layer_id);
                paint_hud_corner_ticks(&painter, r.response.rect, P_GEM);
                if let Some((apply_r, close_r)) = r.inner {
                    rects.account_apply_button = apply_r;
                    rects.account_close_button = close_r;
                }
            }
            rects.account_window = acct_resp.map(|r| r.response.rect);
        }

        if self.session.overlay_mode {
            let overlay_id = egui::ViewportId::from_hash_of("truesight_compact_overlay");
            let builder = overlay::build_overlay_viewport_builder(self.session.overlay_geometry);
            let click_through = self.session.overlay_geometry.click_through;
            ctx.show_viewport_immediate(overlay_id, builder, |vctx, _class| {
                self.sync_live_overlay_viewport_geometry(vctx);
                vctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(click_through));
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(vctx, |ui| {
                        let _ = self.render_overlay_panel(ui);
                    });
            });
        }

        rects
    }

    fn render_workspace_body(
        &mut self,
        ui: &mut egui::Ui,
        analysis: &DraftAnalysis5v5,
        rects: &mut LayoutRects,
        total_w: f32,
        total_h: f32,
    ) {
        let gap = if total_w < 1120.0 { 8.0 } else { 10.0 };
        let compact_h = total_h < 760.0;

        // 1. Top Bar
        let top_out = self.render_top_bar(ui);
        rects.top_bar = top_out.bar;
        rects.options_button = top_out.options_button;
        rects.info_button = top_out.info_button;
        rects.reset_button = top_out.reset_button;
        rects.bracket_bar = top_out.bracket_bar;
        ui.add_space(if compact_h { 8.0 } else { 10.0 });

        // 2. Top Row: Allies (238..356px) | Oracle (flex) | Enemies (238..356px)
        let side_w = (total_w * 0.275).clamp(238.0, 356.0);
        let center_w = (total_w - 2.0 * side_w - 2.0 * gap).max(320.0);
        let top_row_h = if compact_h { 324.0_f32 } else { 368.0_f32 };

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                vec2(side_w, top_row_h),
                Layout::top_down(Align::Min),
                |ui| {
                    rects.allies =
                        self.render_team_card(ui, analysis, Side::Allies, &mut rects.ally_slots);
                },
            );
            ui.allocate_ui_with_layout(
                vec2(center_w, top_row_h),
                Layout::top_down(Align::Min),
                |ui| {
                    rects.oracle =
                        self.render_oracle_card(ui, analysis, &mut rects.enemy_scout_cards);
                },
            );
            ui.allocate_ui_with_layout(
                vec2(side_w, top_row_h),
                Layout::top_down(Align::Min),
                |ui| {
                    rects.enemies =
                        self.render_team_card(ui, analysis, Side::Enemies, &mut rects.enemy_slots);
                },
            );
        });

        ui.add_space(if compact_h { 8.0 } else { 10.0 });

        // 3. Bottom Row: Suggested Picks (flex) | 5v5 Matrix / Comp (square-coupled)
        let bottom_h = ui.available_height().max(220.0);
        let ideal_cell = ((bottom_h - 122.0) / 7.0).floor().clamp(20.0, 56.0);
        let matrix_min_w = if total_w < 1100.0 { 276.0 } else { 360.0 };
        let matrix_max_w = (total_w * 0.36).max(matrix_min_w);
        let matrix_w = (7.0 * ideal_cell + 6.0 * 4.0 + 24.0).clamp(matrix_min_w, matrix_max_w);
        let recs_w = (total_w - matrix_w - gap).max(320.0);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                vec2(recs_w, bottom_h),
                Layout::top_down(Align::Min),
                |ui| {
                    if self.session.is_post_draft_build_view() {
                        let b_out = self.render_item_build_card(ui);
                        rects.suggested = b_out.card;
                    } else {
                        let s_out = self.render_suggested_picks_card(ui);
                        rects.suggested = s_out.card;
                        rects.suggested_last_col_max_x = s_out.last_col_max_x;
                        rects.suggested_comfort_hdr_max_x = s_out.comfort_hdr_max_x;
                        rects.suggested_row_rects = s_out.row_rects;
                        rects.suggested_vs_row_rects = s_out.vs_row_rects;
                        rects.suggested_pick_rects = s_out.pick_rects;
                        rects.suggested_pick_stripe_rects = s_out.pick_stripe_rects;
                    }
                },
            );
            let dock_comp = bottom_h >= 740.0;
            ui.allocate_ui_with_layout(
                vec2(matrix_w, bottom_h),
                Layout::top_down(Align::Min),
                |ui| {
                    if dock_comp {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let matrix_h = (bottom_h * 0.56).clamp(380.0, 496.0);
                        let comp_h = (bottom_h - matrix_h - gap).max(220.0);
                        let mut m_out = None;
                        ui.allocate_ui_with_layout(
                            vec2(matrix_w, matrix_h),
                            Layout::top_down(Align::Min),
                            |ui| {
                                m_out = Some(self.render_matrix_card(ui, analysis, true));
                            },
                        );
                        ui.add_space(gap);
                        let mut comp_out = None;
                        ui.allocate_ui_with_layout(
                            vec2(matrix_w, comp_h),
                            Layout::top_down(Align::Min),
                            |ui| {
                                comp_out = Some(self.render_comp_card(ui, analysis));
                            },
                        );
                        if let (Some(m), Some((c_rect, c_hdr))) = (m_out, comp_out) {
                            rects.matrix = m.card.union(c_rect);
                            rects.matrix_grid_rect = m.grid;
                            rects.matrix_cell_size = m.cell_size;
                            rects.matrix_h_gap = m.h_gap;
                            rects.matrix_v_gap = m.v_gap;
                            rects.matrix_bottom_y = m.bottom_y;
                            rects.docked_comp = Some(c_rect);
                            rects.comp_enemies_hdr = c_hdr;
                        }
                    } else {
                        let m = self.render_matrix_card(ui, analysis, false);
                        rects.matrix = m.card;
                        rects.matrix_grid_rect = m.grid;
                        rects.matrix_cell_size = m.cell_size;
                        rects.matrix_h_gap = m.h_gap;
                        rects.matrix_v_gap = m.v_gap;
                        rects.matrix_bottom_y = m.bottom_y;
                        rects.comp_enemies_hdr = m.comp_enemies_hdr;
                    }
                },
            );
        });
    }
}

impl eframe::App for TrueSightGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _ = self.render_frame(ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Builds the desktop viewport configuration with low-resolution bounds, Wayland `app_id`, and the Gem icon.
/// Keeps the native minimum independent of `MIN_UNSCROLLED_*` so 150% DPI scaling on 1280x720 displays
/// (`640*1.5 = 960`, `440*1.5 = 660`) fits comfortably within the physical screen work area.
pub(super) fn build_viewport_builder() -> egui::ViewportBuilder {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 700.0])
        .with_min_inner_size([640.0, 440.0])
        .with_app_id("truesight-dota")
        .with_title("TrueSight Dota — 5v5 Live Draft Oracle");
    if let Some(icon) = theme::build_gem_window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    viewport
}

pub fn run_gui(
    session: AppSession,
    gsi_listener: Option<BoundedGsiListener>,
    vision_worker: Option<BoundedVisionWorker>,
) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: build_viewport_builder(),
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
