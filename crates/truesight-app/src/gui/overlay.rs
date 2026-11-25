use super::TrueSightGuiApp;
use super::overlay_backend::OverlayHostMode;
use super::theme::*;
use super::viewmodel::compute_overlay_summary;
use crate::session::OverlayGeometry;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense,
    Stroke, StrokeKind, Vec2, pos2, vec2,
};
use truesight_core::domain::{ItemId, Role, TEAM_SIZE};

fn paint_monitor_switch_icon(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.2_f32, color);
    let back = Rect::from_min_max(pos2(c.x - 5.5, c.y - 4.5), pos2(c.x + 1.5, c.y + 1.5));
    let front = Rect::from_min_max(pos2(c.x - 1.5, c.y - 1.5), pos2(c.x + 5.5, c.y + 4.5));
    painter.rect_stroke(back, 1.0, stroke, StrokeKind::Inside);
    painter.rect_filled(front, 1.0, P_INDIGO_2);
    painter.rect_stroke(front, 1.0, stroke, StrokeKind::Inside);
}

fn overlay_icon_button(
    ui: &mut egui::Ui,
    size: Vec2,
    accent: Color32,
    active: bool,
    hover_text: &str,
    paint_icon: impl FnOnce(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let bg = if active || resp.hovered() {
        P_INDIGO_3
    } else {
        P_INDIGO_2
    };
    let border = if active || resp.hovered() {
        accent
    } else {
        P_LINE
    };
    let fg = if active || resp.hovered() {
        accent
    } else {
        P_GLINT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );
    paint_icon(painter, rect, fg);
    resp.on_hover_text(hover_text)
}

/// Layout and interaction telemetry returned by [`TrueSightGuiApp::render_overlay_panel`].
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct OverlayPanelOutput {
    pub(super) rect: Rect,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(super) drag_hit_rect: Rect,
    pub(super) drag_handle: Rect,
    pub(super) resize_handle: Rect,
    pub(super) reset_button: Rect,
    pub(super) drag_started: bool,
    pub(super) resize_started: bool,
}

/// Builds the always-on-top frameless viewport configuration for the compact overlay window,
/// allowing tighter vertical sizing in the icon-only Build view (`220x104`) than Picks (`280x160`).
pub(super) fn build_overlay_viewport_builder(
    geom: OverlayGeometry,
    is_post_draft: bool,
) -> egui::ViewportBuilder {
    let min_size = if is_post_draft {
        [220.0, 104.0]
    } else {
        [280.0, 160.0]
    };
    egui::ViewportBuilder::default()
        .with_inner_size([geom.width, geom.height])
        .with_min_inner_size(min_size)
        .with_max_inner_size([840.0, 720.0])
        .with_resizable(true)
        .with_position([geom.x, geom.y])
        .with_always_on_top()
        .with_decorations(false)
        .with_transparent(true)
        .with_mouse_passthrough(geom.click_through)
        .with_title("TrueSight Dota — Overlay")
}

impl TrueSightGuiApp {
    /// Reads live overlay viewport position, size, and monitor bounds, clamps them against
    /// the active monitor and top-bar capture strip, enforces any correction via viewport commands,
    /// and persists updated geometry to `ui_config.toml`.
    pub(super) fn sync_live_overlay_viewport_geometry(&mut self, vctx: &egui::Context) {
        if !self.native_monitors_probed
            && self.session.known_monitors.is_empty()
            && self.session.active_monitor_rect.is_none()
        {
            self.session.refresh_native_monitors();
            self.native_monitors_probed = true;
        }

        if self.overlay_wm_gesture_cooldown > 0 {
            self.overlay_wm_gesture_cooldown -= 1;
        }

        let zoom_factor = vctx.zoom_factor();
        let (vp_monitor_size, vp_outer_rect, vp_inner_rect, vp_native_ppp, ctx_ppp, screen_rect) =
            vctx.input(|i| {
                let vp = i.viewport();
                (
                    vp.monitor_size,
                    vp.outer_rect,
                    vp.inner_rect,
                    vp.native_pixels_per_point,
                    i.pixels_per_point(),
                    i.screen_rect(),
                )
            });
        let ppp = crate::session::AppSession::resolve_effective_pixels_per_point(
            vp_native_ppp,
            ctx_ppp,
            zoom_factor,
        );

        let fallback_monitor_size_pts = vp_monitor_size
            .map(|s| (s.x, s.y))
            .filter(|(w, h)| *w > 0.0 && *h > 0.0);
        let fallback_monitor_size_px = fallback_monitor_size_pts.map(|(w, h)| (w * ppp, h * ppp));

        let is_post_draft = self.session.is_post_draft_build_view();
        let active_geom = self.session.active_overlay_geometry();
        let (min_w_pts, min_h_pts) = if is_post_draft {
            (220.0, 104.0)
        } else {
            (280.0, 160.0)
        };

        let has_live_position = vp_outer_rect.is_some() || vp_inner_rect.is_some();
        let (x, y) = vp_outer_rect
            .or(vp_inner_rect)
            .map(|r| (r.min.x, r.min.y))
            .unwrap_or((active_geom.x, active_geom.y));

        let live_screen_size = {
            let s = screen_rect.size();
            (s.x > 0.0 && s.y > 0.0).then_some((s.x, s.y))
        };
        let (width, height) = vp_inner_rect
            .or(vp_outer_rect)
            .map(|r| (r.width(), r.height()))
            .or(live_screen_size)
            .unwrap_or((active_geom.width, active_geom.height));

        let candidate_pts = OverlayGeometry {
            x,
            y,
            width,
            height,
            click_through: active_geom.click_through,
        };
        let candidate_px = candidate_pts.scale_to_desktop_pixels(ppp);
        let monitor_rect_px = self.session.resolve_monitor_for_overlay(candidate_px);
        let clamped_px =
            crate::session::AppSession::clamp_overlay_geometry_with_optional_monitor_min_scaled(
                candidate_px,
                monitor_rect_px,
                fallback_monitor_size_px,
                self.session.active_capture_strip_rect,
                ppp,
                min_w_pts,
                min_h_pts,
            );
        let clamped_pts = clamped_px.scale_to_egui_points(ppp);

        // Avoid sending corrective position/size commands while a drag/resize is armed or settling.
        let actively_interacting = self.overlay_drag_armed.is_some()
            || self.overlay_resize_armed.is_some()
            || self.overlay_wm_gesture_cooldown > 0
            || vctx.input(|i| i.pointer.primary_down());
        if !actively_interacting {
            if has_live_position
                && ((clamped_pts.x - candidate_pts.x).abs() > 0.5
                    || (clamped_pts.y - candidate_pts.y).abs() > 0.5)
            {
                vctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                    clamped_pts.x,
                    clamped_pts.y,
                )));
            }
            if (clamped_pts.width - candidate_pts.width).abs() > 0.5
                || (clamped_pts.height - candidate_pts.height).abs() > 0.5
            {
                vctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    clamped_pts.width,
                    clamped_pts.height,
                )));
            }

            if (clamped_pts.x - active_geom.x).abs() > 0.5
                || (clamped_pts.y - active_geom.y).abs() > 0.5
                || (clamped_pts.width - active_geom.width).abs() > 0.5
                || (clamped_pts.height - active_geom.height).abs() > 0.5
            {
                self.session.set_active_overlay_geometry(clamped_pts);
            }
        }
    }

    /// Renders the compact overlay panel for both draft-phase and post-draft item build views.
    pub(super) fn render_overlay_panel(&mut self, ui: &mut egui::Ui) -> OverlayPanelOutput {
        self.render_overlay_panel_with_host(ui, OverlayHostMode::Viewport)
    }

    /// Renders the compact overlay panel using either the primary `eframe` viewport texture atlas
    /// (`OverlayHostMode::Viewport`) or the dedicated Wayland layer-shell `egui::Context` atlas
    /// (`OverlayHostMode::LayerShell`), delegating drag/resize appropriately.
    pub(super) fn render_overlay_panel_with_host(
        &mut self,
        ui: &mut egui::Ui,
        host_mode: OverlayHostMode,
    ) -> OverlayPanelOutput {
        match host_mode {
            OverlayHostMode::Viewport => self.textures.ensure_loaded(ui.ctx()),
            OverlayHostMode::LayerShell => self.overlay_textures.ensure_loaded(ui.ctx()),
        }
        let textures = match host_mode {
            OverlayHostMode::Viewport => &self.textures,
            OverlayHostMode::LayerShell => &self.overlay_textures,
        };
        let tr = super::i18n::strings(self.session.ui_language);
        let summary = compute_overlay_summary(&self.session);
        let interactive = !self.session.overlay_geometry.click_through;
        let mut clicked_role: Option<Role> = None;
        let mut toggle_build_view = false;
        let mut reset_draft = false;
        let mut toggle_click_through = false;
        let mut cycle_monitor = false;
        let mut exit_overlay = false;
        let mut clicked_variant: Option<(ItemId, ItemId)> = None;
        let mut drag_handle = Rect::NOTHING;
        let mut drag_hit_rect = Rect::NOTHING;
        let mut reset_button = Rect::NOTHING;

        let panel_resp = egui::Frame::NONE
            .fill(P_VOID.gamma_multiply(0.92))
            .stroke(Stroke::new(1.5_f32, P_GEM_DEEP))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.set_min_height(ui.available_height());

                // Top mini-bar: hold-drag grip + title + WIN% on left; crisp vector icon controls on right
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    let (grip_rect, grip_resp) =
                        ui.allocate_exact_size(vec2(20.0, 18.0), Sense::hover());
                    drag_handle = grip_rect;
                    let drag_active = self.overlay_drag_armed.is_some();
                    let grip_color = if drag_active || grip_resp.hovered() {
                        P_GEM
                    } else {
                        P_MUTED
                    };
                    paint_drag_grip_icon(ui.painter(), grip_rect, grip_color);
                    if interactive {
                        grip_resp
                            .on_hover_cursor(if drag_active {
                                egui::CursorIcon::Grabbing
                            } else {
                                egui::CursorIcon::Grab
                            })
                            .on_hover_text(tr.overlay_drag_hover);
                    }

                    let title_resp = ui.add(
                        egui::Label::new(
                            RichText::new(tr.overlay_title)
                                .size(9.5)
                                .strong()
                                .color(P_GEM),
                        )
                        .sense(Sense::hover()),
                    );
                    drag_hit_rect = grip_rect.union(title_resp.rect);
                    if interactive {
                        title_resp
                            .on_hover_cursor(if drag_active {
                                egui::CursorIcon::Grabbing
                            } else {
                                egui::CursorIcon::Grab
                            })
                            .on_hover_text(tr.overlay_drag_hover);
                    }

                    let win_col = if summary.projected_ally_win_pct >= 50.0 {
                        P_GEM
                    } else {
                        P_DIRE
                    };
                    ui.label(
                        RichText::new(format!(
                            "{} {:.1}%",
                            tr.overlay_win_prefix, summary.projected_ally_win_pct
                        ))
                        .size(10.0)
                        .strong()
                        .color(win_col),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        let icon_sz = vec2(20.0, 18.0);

                        if overlay_icon_button(
                            ui,
                            icon_sz,
                            P_DIRE,
                            false,
                            tr.overlay_exit_hover,
                            paint_close_icon,
                        )
                        .clicked()
                        {
                            exit_overlay = true;
                        }

                        if self.session.overlay_geometry.click_through {
                            egui::Frame::NONE
                                .fill(P_INDIGO_2)
                                .stroke(Stroke::new(1.0_f32, P_NEUTRAL))
                                .inner_margin(Margin::symmetric(4, 1))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 2.0;
                                        let (pr, _) = ui
                                            .allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
                                        paint_pass_icon(ui.painter(), pr, P_NEUTRAL, true);
                                        ui.label(
                                            RichText::new(tr.overlay_pass_on_badge)
                                                .size(8.5)
                                                .strong()
                                                .color(P_NEUTRAL),
                                        );
                                    });
                                })
                                .response
                                .on_hover_text(tr.top_bar_pass_unlock_hover);
                        } else if overlay_icon_button(
                            ui,
                            icon_sz,
                            P_NEUTRAL,
                            false,
                            tr.overlay_pass_hover,
                            |p, r, c| paint_pass_icon(p, r, c, false),
                        )
                        .clicked()
                        {
                            toggle_click_through = true;
                        }

                        let reset_resp = overlay_icon_button(
                            ui,
                            icon_sz,
                            P_GEM,
                            false,
                            tr.reset_hover,
                            paint_reset_icon,
                        );
                        reset_button = reset_resp.rect;
                        if reset_resp.clicked() {
                            reset_draft = true;
                        }

                        let mode_hover = if summary.is_post_draft {
                            tr.picks_view_hover
                        } else {
                            tr.build_view_hover
                        };
                        if overlay_icon_button(
                            ui,
                            icon_sz,
                            P_CYAN,
                            summary.is_post_draft,
                            mode_hover,
                            |p, r, c| paint_mode_icon(p, r, c, summary.is_post_draft),
                        )
                        .clicked()
                        {
                            toggle_build_view = true;
                        }

                        if self.has_multiple_overlay_monitors(host_mode)
                            && overlay_icon_button(
                                ui,
                                icon_sz,
                                P_CYAN,
                                false,
                                "Move overlay to next monitor",
                                paint_monitor_switch_icon,
                            )
                            .clicked()
                        {
                            cycle_monitor = true;
                        }
                    });
                });

                ui.add_space(4.0);

                if !summary.is_post_draft {
                    // Role selector P1..P5
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (idx, role) in Role::ALL.into_iter().enumerate() {
                            let is_on = summary.target_role == role;
                            let btn = egui::Button::new(
                                RichText::new(format!("P{}", idx + 1))
                                    .size(9.0)
                                    .strong()
                                    .color(if is_on { P_VOID } else { P_MUTED }),
                            )
                            .fill(if is_on {
                                P_GEM
                            } else {
                                P_INDIGO_2
                            });
                            if ui.add(btn).clicked() {
                                clicked_role = Some(role);
                            }
                        }
                    });

                    ui.add_space(4.0);

                    let body_w = ui.available_width();
                    // Reserve 14px strip at the bottom so the resize corner handle stays clear.
                    let body_h = (ui.available_height() - 14.0).max(52.0);
                    let matrix_col_w = (body_w * 0.44).clamp(84.0, 260.0);
                    let picks_col_w = (body_w - matrix_col_w - 8.0).max(96.0);
                    let grid_gap = 2.0_f32;
                    let cell_side = ((matrix_col_w - 5.0 * grid_gap) / 6.0)
                        .min(((body_h - 14.0 - 5.0 * grid_gap) / 6.0).max(10.0))
                        .clamp(10.0, 38.0);
                    let cell_sz = vec2(cell_side, cell_side);
                    let cell_font = (cell_side * 0.27).clamp(6.8, 10.5);
                    let max_picks = ((body_h - 14.0) / 18.0).floor().clamp(2.0, 8.0) as usize;

                    // Draft phase: Hero-portrait Top Suggestions (left) + 5x5 Intensity Matrix (right)
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;

                        ui.allocate_ui_with_layout(
                            vec2(picks_col_w, body_h),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(
                                    RichText::new(format!(
                                        "TOP {} PICKS",
                                        summary.target_role.label().to_ascii_uppercase()
                                    ))
                                    .size(9.0)
                                    .strong()
                                    .color(P_MUTED),
                                );
                                if summary.top_suggestions.is_empty() {
                                    ui.label(
                                        RichText::new(tr.overlay_no_suggestions)
                                            .size(9.5)
                                            .color(P_MUTED),
                                    );
                                } else {
                                    for (idx, (hid, name, win_pct)) in
                                        summary.top_suggestions.iter().take(max_picks).enumerate()
                                    {
                                        let col = if *win_pct >= 50.0 { P_GEM } else { P_DIRE };
                                        let htex = textures.heroes.get(hid).map(|s| &s.color);
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            paint_portrait_16_9(
                                                ui,
                                                htex,
                                                vec2(26.0, 14.6),
                                                P_GEM_DEEP,
                                                1.0,
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(format!("{}. {name}", idx + 1))
                                                        .size(9.5)
                                                        .color(P_GLINT),
                                                )
                                                .truncate(),
                                            );
                                            ui.label(
                                                RichText::new(format!("{win_pct:.1}%"))
                                                    .size(9.5)
                                                    .strong()
                                                    .color(col),
                                            );
                                        });
                                    }
                                }
                            },
                        );

                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(tr.overlay_cnt_label)
                                    .size(9.0)
                                    .strong()
                                    .color(P_MUTED),
                            );
                            ui.spacing_mut().item_spacing = vec2(grid_gap, grid_gap);

                            // Column Header Row: Blank corner + 5 Enemy Hero Portraits
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = grid_gap;
                                ui.allocate_exact_size(cell_sz, Sense::hover());
                                for (col, enemy) in self
                                    .session
                                    .draft
                                    .enemies
                                    .iter()
                                    .enumerate()
                                    .take(TEAM_SIZE)
                                {
                                    let (r, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                                    if let Some(ep) = enemy {
                                        if let Some(t) =
                                            textures.heroes.get(&ep.hero_id).map(|s| &s.color)
                                        {
                                            ui.painter().image(
                                                t.id(),
                                                r,
                                                uv_aspect_crop(cell_sz),
                                                Color32::WHITE,
                                            );
                                        } else {
                                            ui.painter().rect_filled(r, 0.0, P_INDIGO_2);
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
                                            format!("E{}", col + 1),
                                            FontId::proportional((cell_side * 0.32).max(6.5)),
                                            P_MUTED,
                                        );
                                    }
                                    ui.painter().line_segment(
                                        [pos2(r.min.x, r.max.y), r.max],
                                        Stroke::new(1.5_f32, P_DIRE),
                                    );
                                }
                            });

                            // 5 Ally Rows: Ally Portrait + Tinted Enemy Matchup Intensity Cells
                            for row in 0..TEAM_SIZE {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = grid_gap;
                                    let (rh, _) = ui.allocate_exact_size(cell_sz, Sense::hover());
                                    if let Some(ap) = self.session.draft.allies[row] {
                                        if let Some(t) =
                                            textures.heroes.get(&ap.hero_id).map(|s| &s.color)
                                        {
                                            ui.painter().image(
                                                t.id(),
                                                rh,
                                                uv_aspect_crop(cell_sz),
                                                Color32::WHITE,
                                            );
                                        } else {
                                            ui.painter().rect_filled(rh, 0.0, P_INDIGO_2);
                                        }
                                    } else {
                                        ui.painter().rect_stroke(
                                            rh,
                                            0.0,
                                            Stroke::new(1.0_f32, P_LINE),
                                            StrokeKind::Inside,
                                        );
                                        ui.painter().text(
                                            rh.center(),
                                            Align2::CENTER_CENTER,
                                            format!("A{}", row + 1),
                                            FontId::proportional((cell_side * 0.32).max(6.5)),
                                            P_MUTED,
                                        );
                                    }
                                    ui.painter().line_segment(
                                        [pos2(rh.max.x, rh.min.y), rh.max],
                                        Stroke::new(1.5_f32, P_GEM),
                                    );

                                    for col in 0..TEAM_SIZE {
                                        if let Some(delta) = summary.counter_grid[row][col] {
                                            let enemy_toned = self.session.draft.enemies[col]
                                                .and_then(|p| textures.heroes.get(&p.hero_id))
                                                .map(|s| &s.toned);
                                            let cell_resp = paint_tinted_cell(
                                                ui,
                                                cell_sz,
                                                enemy_toned,
                                                delta * 100.0,
                                                cell_font,
                                                1.0,
                                            );
                                            let a_name = self.session.draft.allies[row]
                                                .and_then(|p| self.session.matrix.hero(p.hero_id))
                                                .map(|h| h.display_name.as_str())
                                                .unwrap_or("Ally");
                                            let e_name = self.session.draft.enemies[col]
                                                .and_then(|p| self.session.matrix.hero(p.hero_id))
                                                .map(|h| h.display_name.as_str())
                                                .unwrap_or("Enemy");
                                            cell_resp.on_hover_text(format!(
                                                "{a_name} vs {e_name}: {}",
                                                fmt_pct(delta * 100.0)
                                            ));
                                        } else {
                                            let (er, _) =
                                                ui.allocate_exact_size(cell_sz, Sense::hover());
                                            ui.painter().rect_filled(er, 0.0, P_INDIGO_1);
                                            ui.painter().rect_stroke(
                                                er,
                                                0.0,
                                                Stroke::new(1.0_f32, P_LINE),
                                                StrokeKind::Inside,
                                            );
                                        }
                                    }
                                });
                            }
                        });
                    });
                } else {
                    if let Some(pair) = self.render_overlay_build_panel(ui, &summary, textures) {
                        clicked_variant = Some(pair);
                    }
                }
            });

        let panel_rect = panel_resp.response.rect;
        let visible_rect = ui.ctx().screen_rect();
        let clamped_panel = visible_rect.intersect(panel_rect);
        let handle_anchor = if clamped_panel.width() >= 20.0 && clamped_panel.height() >= 20.0 {
            clamped_panel
        } else {
            visible_rect
        };
        let resize_handle = Rect::from_min_max(
            egui::pos2(
                (handle_anchor.right() - 16.0).max(handle_anchor.left()),
                (handle_anchor.bottom() - 16.0).max(handle_anchor.top()),
            ),
            egui::pos2(handle_anchor.right() - 2.0, handle_anchor.bottom() - 2.0),
        );

        let mut drag_started = false;
        let mut resize_started = false;

        if interactive {
            let resize_resp = ui
                .interact(
                    resize_handle,
                    ui.id().with("truesight_overlay_resize_grip"),
                    Sense::hover(),
                )
                .on_hover_cursor(egui::CursorIcon::ResizeSouthEast)
                .on_hover_text(tr.overlay_resize_hover);
            let resize_active = self.overlay_resize_armed.is_some();
            let line_col = if resize_active || resize_resp.hovered() {
                P_GEM
            } else {
                P_MUTED
            };
            let fg_painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                ui.id().with("truesight_overlay_resize_grip_layer"),
            ));
            fg_painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(resize_handle.right(), resize_handle.top()),
                    resize_handle.max,
                    egui::pos2(resize_handle.left(), resize_handle.bottom()),
                ],
                P_VOID.gamma_multiply(0.92),
                Stroke::NONE,
            ));
            for offset in [3.5_f32, 7.0, 10.5] {
                fg_painter.line_segment(
                    [
                        egui::pos2(resize_handle.right() - offset, resize_handle.bottom()),
                        egui::pos2(resize_handle.right(), resize_handle.bottom() - offset),
                    ],
                    Stroke::new(1.2_f32, line_col),
                );
            }

            // Hold-only state machine for WM drag & resize with explicit Wayland pointer reset
            // so releasing outside the compositor grab (`emilk/egui#7959`) never leaves drag stuck.
            // In LayerShell mode the native Wayland presenter handles drag/resize directly via margins.
            if host_mode == OverlayHostMode::Viewport {
                let (
                    primary_pressed,
                    primary_released,
                    primary_down,
                    latest_pos,
                    press_origin,
                    pointer_gone,
                    focused,
                ) = ui.ctx().input(|i| {
                    let gone = i
                        .events
                        .iter()
                        .any(|e| matches!(e, egui::Event::PointerGone));
                    (
                        i.pointer.primary_pressed(),
                        i.pointer.primary_released(),
                        i.pointer.primary_down(),
                        i.pointer.latest_pos(),
                        i.pointer.press_origin(),
                        gone,
                        i.viewport().focused.unwrap_or(true),
                    )
                });

                if primary_released || pointer_gone || !focused {
                    self.overlay_drag_armed = None;
                    self.overlay_resize_armed = None;
                } else {
                    if primary_pressed && let Some(pos) = press_origin.or(latest_pos) {
                        if resize_handle.contains(pos) {
                            self.overlay_resize_armed = Some(pos);
                            self.overlay_drag_armed = None;
                        } else if drag_hit_rect.contains(pos) {
                            self.overlay_drag_armed = Some(pos);
                            self.overlay_resize_armed = None;
                        }
                    }

                    if primary_down {
                        if let Some(origin) = self.overlay_drag_armed
                            && let Some(pos) = latest_pos
                            && (pos - origin).length() >= 3.0
                        {
                            drag_started = true;
                            self.overlay_drag_armed = None;
                            self.overlay_wm_gesture_cooldown = 2;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                            ui.ctx().stop_dragging();
                            ui.ctx()
                                .input_mut(|i| i.pointer = egui::PointerState::default());
                        } else if let Some(origin) = self.overlay_resize_armed
                            && let Some(pos) = latest_pos
                            && (pos - origin).length() >= 3.0
                        {
                            resize_started = true;
                            self.overlay_resize_armed = None;
                            self.overlay_wm_gesture_cooldown = 2;
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::BeginResize(
                                    egui::ResizeDirection::SouthEast,
                                ));
                            ui.ctx().stop_dragging();
                            ui.ctx()
                                .input_mut(|i| i.pointer = egui::PointerState::default());
                        }
                    }
                }
            } else {
                self.overlay_drag_armed = None;
                self.overlay_resize_armed = None;
            }
        } else {
            self.overlay_drag_armed = None;
            self.overlay_resize_armed = None;
        }

        if let Some(role) = clicked_role {
            self.session.draft.target_role = role;
        }
        if reset_draft {
            self.reset_draft_ui();
        }
        if toggle_build_view {
            self.session.toggle_post_draft_build_view();
        }
        if toggle_click_through {
            let next = !self.session.overlay_geometry.click_through;
            self.set_overlay_click_through_live(ui.ctx(), next);
        }
        if cycle_monitor {
            self.cycle_overlay_monitor(ui.ctx(), host_mode);
        }
        if exit_overlay {
            self.session.set_overlay_mode(false);
            ui.ctx().request_repaint();
        }
        if let Some((primary_id, var_id)) = clicked_variant {
            self.session
                .select_build_variant_for_item(primary_id, var_id);
        }

        OverlayPanelOutput {
            rect: panel_rect,
            drag_hit_rect,
            drag_handle,
            resize_handle,
            reset_button,
            drag_started,
            resize_started,
        }
    }
}
