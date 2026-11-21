use super::TrueSightGuiApp;
use super::theme::*;
use super::viewmodel::compute_overlay_summary;
use crate::session::OverlayGeometry;
use eframe::egui::{self, Align, Layout, Margin, Rect, RichText, Sense, Stroke, vec2};
use truesight_core::domain::{ItemId, Role, TEAM_SIZE};
use truesight_core::ports::ItemMetaRepository;

fn format_clock(secs: i32) -> String {
    let abs = secs.unsigned_abs();
    let mins = abs / 60;
    let rem = abs % 60;
    if secs < 0 {
        format!("-{mins:02}:{rem:02}")
    } else {
        format!("{mins:02}:{rem:02}")
    }
}

/// Builds the always-on-top frameless viewport configuration for the compact overlay window.
pub(super) fn build_overlay_viewport_builder(geom: OverlayGeometry) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_inner_size([geom.width, geom.height])
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
        let ppp =
            crate::session::AppSession::sanitize_pixels_per_point(vp_native_ppp.unwrap_or(ctx_ppp));

        let fallback_monitor_size_pts = vp_monitor_size
            .map(|s| (s.x, s.y))
            .filter(|(w, h)| *w > 0.0 && *h > 0.0)
            .or_else(|| {
                let s = screen_rect.size();
                if s.x > 0.0 && s.y > 0.0 {
                    Some((s.x, s.y))
                } else {
                    None
                }
            });
        let fallback_monitor_size_px = fallback_monitor_size_pts.map(|(w, h)| (w * ppp, h * ppp));

        let (x, y) = vp_outer_rect
            .or(vp_inner_rect)
            .map(|r| (r.min.x, r.min.y))
            .unwrap_or((
                self.session.overlay_geometry.x,
                self.session.overlay_geometry.y,
            ));

        let (width, height) = vp_inner_rect
            .or(vp_outer_rect)
            .map(|r| (r.width(), r.height()))
            .unwrap_or((
                self.session.overlay_geometry.width,
                self.session.overlay_geometry.height,
            ));

        let candidate_pts = OverlayGeometry {
            x,
            y,
            width,
            height,
            click_through: self.session.overlay_geometry.click_through,
        };
        let candidate_px = candidate_pts.scale_to_desktop_pixels(ppp);
        let monitor_rect_px = self.session.resolve_monitor_for_overlay(candidate_px);
        let clamped_px =
            crate::session::AppSession::clamp_overlay_geometry_with_optional_monitor_scaled(
                candidate_px,
                monitor_rect_px,
                fallback_monitor_size_px,
                self.session.active_capture_strip_rect,
                ppp,
            );
        let clamped_pts = clamped_px.scale_to_egui_points(ppp);

        if (clamped_pts.x - candidate_pts.x).abs() > 0.5
            || (clamped_pts.y - candidate_pts.y).abs() > 0.5
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

        let prev = self.session.overlay_geometry;
        if (clamped_pts.x - prev.x).abs() > 0.5
            || (clamped_pts.y - prev.y).abs() > 0.5
            || (clamped_pts.width - prev.width).abs() > 0.5
            || (clamped_pts.height - prev.height).abs() > 0.5
        {
            self.session.set_overlay_geometry(clamped_pts);
        }
    }

    /// Renders the compact overlay panel for both draft-phase and post-draft item build views.
    pub(super) fn render_overlay_panel(&mut self, ui: &mut egui::Ui) -> Rect {
        let summary = compute_overlay_summary(&self.session);
        let mut clicked_role: Option<Role> = None;
        let mut toggle_build_view = false;
        let mut toggle_click_through = false;
        let mut exit_overlay = false;
        let mut clicked_variant: Option<(ItemId, ItemId)> = None;

        let panel_resp = egui::Frame::NONE
            .fill(P_VOID.gamma_multiply(0.92))
            .stroke(Stroke::new(1.5_f32, P_GEM_DEEP))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                // Top mini-bar
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(
                        RichText::new("TRUESIGHT OVERLAY")
                            .size(9.5)
                            .strong()
                            .color(P_GEM),
                    );
                    let win_col = if summary.projected_ally_win_pct >= 50.0 {
                        P_GEM
                    } else {
                        P_DIRE
                    };
                    ui.label(
                        RichText::new(format!("WIN: {:.1}%", summary.projected_ally_win_pct))
                            .size(10.0)
                            .strong()
                            .color(win_col),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("[ EXIT ]").size(9.0).strong().color(P_DIRE),
                                )
                                .fill(P_INDIGO_2),
                            )
                            .clicked()
                        {
                            exit_overlay = true;
                        }

                        let pass_lbl = if self.session.overlay_geometry.click_through {
                            "[ PASS: ON ]"
                        } else {
                            "[ PASS ]"
                        };
                        if ui
                            .add(
                                egui::Button::new(RichText::new(pass_lbl).size(9.0).color(
                                    if self.session.overlay_geometry.click_through {
                                        P_NEUTRAL
                                    } else {
                                        P_MUTED
                                    },
                                ))
                                .fill(P_INDIGO_2),
                            )
                            .clicked()
                        {
                            toggle_click_through = true;
                        }

                        let mode_lbl = if summary.is_post_draft {
                            "[ PICKS ]"
                        } else {
                            "[ BUILD ]"
                        };
                        if ui
                            .add(
                                egui::Button::new(RichText::new(mode_lbl).size(9.0).color(P_GLINT))
                                    .fill(P_INDIGO_2),
                            )
                            .clicked()
                        {
                            toggle_build_view = true;
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

                    // Draft phase: top 3 suggestions + compact 5x5 mini counter grid
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;

                        ui.vertical(|ui| {
                            ui.set_min_width(180.0);
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
                                    RichText::new("No hero suggestions")
                                        .size(9.5)
                                        .color(P_MUTED),
                                );
                            } else {
                                for (idx, (_id, name, win_pct)) in
                                    summary.top_suggestions.iter().enumerate()
                                {
                                    let col = if *win_pct >= 50.0 { P_GEM } else { P_DIRE };
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("{}. {name}", idx + 1))
                                                .size(9.5)
                                                .color(P_GLINT),
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
                        });

                        // Compact 5x5 colored mini counter grid
                        ui.vertical(|ui| {
                            ui.label(RichText::new("5v5 CNT").size(9.0).strong().color(P_MUTED));
                            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                            for row in &summary.counter_grid {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                                    for cell in row {
                                        let (rect, _) = ui
                                            .allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                                        let fill = match cell {
                                            Some(delta) if *delta > 0.005 => P_GEM_DEEP,
                                            Some(delta) if *delta < -0.005 => P_DIRE_DEEP,
                                            Some(_) => P_INDIGO_3,
                                            None => P_INDIGO_1,
                                        };
                                        ui.painter().rect_filled(rect, 1.0, fill);
                                    }
                                });
                            }
                        });
                    });
                } else {
                    // Post-draft phase: compact [Item 1] > [Item 2] > [Item 3] + clock + variants
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("T = {}", format_clock(summary.game_time_secs)))
                                .size(9.5)
                                .strong()
                                .color(P_CYAN),
                        );
                        ui.label(
                            RichText::new("ITEM BUILD")
                                .size(9.5)
                                .strong()
                                .color(P_MUTED),
                        );
                    });
                    ui.add_space(4.0);

                    if let Some(build) = &summary.build {
                        egui::ScrollArea::horizontal()
                            .id_salt("truesight_overlay_build_scroll")
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    for (idx, step) in build.steps.iter().enumerate() {
                                        if idx > 0 {
                                            ui.label(
                                                RichText::new(">").size(11.0).strong().color(P_GEM),
                                            );
                                        }
                                        let is_next = build.next_unowned_index == Some(idx);
                                        let name = self
                                            .session
                                            .item_repo
                                            .item_catalog(step.item_id)
                                            .map(|e| e.display_name)
                                            .unwrap_or_else(|| format!("#{}", step.item_id.0));
                                        let badge = if step.owned {
                                            "[OK]"
                                        } else if is_next {
                                            "[NEXT]"
                                        } else {
                                            ""
                                        };
                                        let step_resp = ui
                                            .vertical(|ui| {
                                                ui.spacing_mut().item_spacing.y = 2.0;
                                                egui::Frame::NONE
                                                    .fill(P_INDIGO_2)
                                                    .stroke(Stroke::new(
                                                        1.0_f32,
                                                        if is_next { P_GEM } else { P_LINE },
                                                    ))
                                                    .inner_margin(Margin::symmetric(5, 3))
                                                    .show(ui, |ui| {
                                                        if !badge.is_empty() {
                                                            ui.label(
                                                                RichText::new(badge)
                                                                    .size(8.5)
                                                                    .strong()
                                                                    .color(P_GEM),
                                                            );
                                                        }
                                                        ui.label(
                                                            RichText::new(name)
                                                                .size(9.5)
                                                                .strong()
                                                                .color(if step.owned {
                                                                    P_MUTED
                                                                } else {
                                                                    P_GLINT
                                                                }),
                                                        );
                                                    });
                                                for var in step.variants.iter().take(2) {
                                                    let vname = self
                                                        .session
                                                        .item_repo
                                                        .item_catalog(var.item_id)
                                                        .map(|e| e.display_name)
                                                        .unwrap_or_else(|| {
                                                            format!("#{}", var.item_id.0)
                                                        });
                                                    if ui
                                                        .add(
                                                            egui::Button::new(
                                                                RichText::new(format!(
                                                                    "[v] {vname}"
                                                                ))
                                                                .size(8.5)
                                                                .color(P_SILVER),
                                                            )
                                                            .fill(P_INDIGO_1),
                                                        )
                                                        .clicked()
                                                    {
                                                        clicked_variant =
                                                            Some((step.item_id, var.item_id));
                                                    }
                                                }
                                            })
                                            .response;
                                        if is_next {
                                            step_resp.scroll_to_me(Some(Align::Center));
                                        }
                                    }
                                });
                            });
                    } else {
                        ui.label(
                            RichText::new("Pick an allied hero to view item build.")
                                .size(9.5)
                                .color(P_MUTED),
                        );
                    }
                }
            });

        if let Some(role) = clicked_role {
            self.session.draft.target_role = role;
        }
        if toggle_build_view {
            self.session.toggle_post_draft_build_view();
        }
        if toggle_click_through {
            let next = !self.session.overlay_geometry.click_through;
            self.session.set_overlay_click_through(next);
        }
        if exit_overlay {
            self.session.set_overlay_mode(false);
        }
        if let Some((primary_id, var_id)) = clicked_variant {
            self.session
                .select_build_variant_for_item(primary_id, var_id);
        }

        let _ = TEAM_SIZE;
        panel_resp.response.rect
    }
}
