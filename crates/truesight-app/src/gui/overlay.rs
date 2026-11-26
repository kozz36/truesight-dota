use super::TrueSightGuiApp;
use super::overlay_backend::OverlayHostMode;
use super::surfaces::{paint_empty_matrix_slot, translucent};
use super::theme::*;
use super::viewmodel::compute_overlay_summary;
use crate::session::OverlayGeometry;
use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind,
    Vec2, pos2, vec2,
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
        translucent(P_INDIGO_3, 155)
    } else {
        translucent(P_INDIGO_2, 72)
    };
    let border = if active || resp.hovered() {
        accent
    } else {
        translucent(P_LINE, 130)
    };
    let fg = if active || resp.hovered() {
        accent
    } else {
        P_GLINT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(3),
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

        let bg_idx = ui.painter().add(egui::Shape::Noop);
        let panel_resp = egui::Frame::NONE
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
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

                    if let Some(gem_tex) = textures.gem.as_ref() {
                        let (gem_r, _) = ui.allocate_exact_size(vec2(14.0, 10.5), Sense::hover());
                        ui.painter()
                            .image(gem_tex.id(), gem_r, uv_full(), Color32::WHITE);
                        drag_hit_rect = drag_hit_rect.union(gem_r);
                    }

                    let win_col = if summary.projected_ally_win_pct >= 50.0 {
                        P_GEM
                    } else {
                        super::surfaces::OVERLAY_DIRE_TEXT
                    };
                    super::surfaces::overlay_text_scrim(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let title_resp = ui.add(
                                egui::Label::new(
                                    RichText::new(tr.overlay_title)
                                        .size(9.5)
                                        .strong()
                                        .color(P_GEM),
                                )
                                .sense(Sense::hover()),
                            );
                            drag_hit_rect = if drag_hit_rect == Rect::NOTHING {
                                grip_rect.union(title_resp.rect)
                            } else {
                                drag_hit_rect.union(grip_rect).union(title_resp.rect)
                            };
                            if interactive {
                                title_resp
                                    .on_hover_cursor(if drag_active {
                                        egui::CursorIcon::Grabbing
                                    } else {
                                        egui::CursorIcon::Grab
                                    })
                                    .on_hover_text(tr.overlay_drag_hover);
                            }
                            ui.label(
                                RichText::new(format!(
                                    "{} {:.1}%",
                                    tr.overlay_win_prefix, summary.projected_ally_win_pct
                                ))
                                .size(10.0)
                                .strong()
                                .color(win_col),
                            );
                        });
                    });

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
                                .fill(translucent(P_INDIGO_2, 110))
                                .corner_radius(CornerRadius::same(3))
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
                                    .color(if is_on { P_VOID } else { P_SILVER }),
                            )
                            .fill(if is_on {
                                P_GEM
                            } else {
                                translucent(P_INDIGO_2, 76)
                            })
                            .stroke(Stroke::new(
                                1.0_f32,
                                if is_on {
                                    P_GEM
                                } else {
                                    translucent(P_LINE, 130)
                                },
                            ))
                            .corner_radius(CornerRadius::same(3));
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
                                super::surfaces::overlay_scrim_label(
                                    ui,
                                    format!(
                                        "TOP {} PICKS",
                                        summary.target_role.label().to_ascii_uppercase()
                                    ),
                                    9.0,
                                    true,
                                    P_SILVER,
                                );
                                if summary.top_suggestions.is_empty() {
                                    super::surfaces::overlay_scrim_label(
                                        ui,
                                        tr.overlay_no_suggestions,
                                        9.5,
                                        false,
                                        P_SILVER,
                                    );
                                } else {
                                    for (idx, (hid, name, win_pct)) in
                                        summary.top_suggestions.iter().take(max_picks).enumerate()
                                    {
                                        let col = if *win_pct >= 50.0 {
                                            P_GEM
                                        } else {
                                            super::surfaces::OVERLAY_DIRE_TEXT
                                        };
                                        let htex = textures.heroes.get(hid).map(|s| &s.color);
                                        super::surfaces::overlay_text_scrim(ui, |ui| {
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
                                                        RichText::new(format!(
                                                            "{}. {name}",
                                                            idx + 1
                                                        ))
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
                                        });
                                    }
                                }
                            },
                        );

                        ui.vertical(|ui| {
                            super::surfaces::overlay_scrim_label(
                                ui,
                                tr.overlay_cnt_label,
                                9.0,
                                true,
                                P_SILVER,
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
                                        let lbl = format!("E{}", col + 1);
                                        paint_empty_matrix_slot(
                                            ui.painter(),
                                            r,
                                            36,
                                            120,
                                            Some(&lbl),
                                            (cell_side * 0.32).max(6.5),
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
                                        let lbl = format!("A{}", row + 1);
                                        paint_empty_matrix_slot(
                                            ui.painter(),
                                            rh,
                                            36,
                                            120,
                                            Some(&lbl),
                                            (cell_side * 0.32).max(6.5),
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
                                            paint_empty_matrix_slot(
                                                ui.painter(),
                                                er,
                                                44,
                                                115,
                                                None,
                                                0.0,
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
        let grain_id = super::surfaces::gem_grain_texture_id(ui.ctx());
        let glow_id = super::surfaces::gem_glow_texture_id(ui.ctx());
        ui.painter().set(
            bg_idx,
            super::surfaces::build_overlay_glass_background(
                panel_rect,
                self.session.overlay_opacity,
                grain_id,
                glow_id,
            ),
        );
        super::surfaces::paint_overlay_glass_frame(ui.painter(), panel_rect);
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
                            self.overlay_viewport_sync.begin_wm_gesture();
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
                            self.overlay_viewport_sync.begin_wm_gesture();
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
            self.session.dismiss_overlay();
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
