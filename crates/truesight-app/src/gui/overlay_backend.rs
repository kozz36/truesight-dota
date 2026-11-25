use eframe::egui;

/// Native or fallback overlay presentation backend selected for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) enum OverlayBackendKind {
    EframeViewport,
    WaylandLayerShell,
}

/// Host context mode passed to the shared overlay UI renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) enum OverlayHostMode {
    Viewport,
    LayerShell,
}

/// Axis-aligned logical-point rectangle serialized across thread boundaries for pointer hit-testing.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) struct OverlayRect {
    pub(super) min_x: f32,
    pub(super) min_y: f32,
    pub(super) max_x: f32,
    pub(super) max_y: f32,
}

impl OverlayRect {
    /// Returns `true` when logical point `(x, y)` lies inside this rectangle (inclusive).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    #[inline]
    pub(super) fn contains(self, x: f32, y: f32) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }
}

impl From<egui::Rect> for OverlayRect {
    fn from(r: egui::Rect) -> Self {
        Self {
            min_x: r.min.x,
            min_y: r.min.y,
            max_x: r.max.x,
            max_y: r.max.y,
        }
    }
}

/// Commands dispatched from the `eframe` UI thread to the Wayland `layer-shell` presenter thread.
#[derive(Debug, Clone)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) enum LayerCommand {
    Show {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        min_w: u32,
        min_h: u32,
        click_through: bool,
    },
    Hide,
    SetGeometry {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        min_w: u32,
        min_h: u32,
    },
    SetClickThrough(bool),
    SetHitRegions {
        drag_rect: Option<OverlayRect>,
        resize_rect: Option<OverlayRect>,
    },
    SetCursor(egui::CursorIcon),
    PresentFrame {
        width_px: u32,
        height_px: u32,
        scale: i32,
        argb8888: Vec<u8>,
    },
    Shutdown,
}

/// Input, output-bounds, and lifecycle events reported back to the `eframe` UI thread.
#[derive(Debug, Clone)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) enum LayerEvent {
    Configured {
        width: u32,
        height: u32,
        scale: i32,
    },
    GeometryChanged {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        persist: bool,
    },
    MonitorsUpdated(Vec<crate::session::MonitorRect>),
    PointerMoved {
        x: f32,
        y: f32,
    },
    PointerLeft,
    PointerButton {
        x: f32,
        y: f32,
        button: egui::PointerButton,
        pressed: bool,
    },
    Scroll {
        dx: f32,
        dy: f32,
    },
    Closed,
}

/// Resolves which overlay backend to activate given optional `TRUESIGHT_OVERLAY_BACKEND`
/// override, session type, and whether the Wayland compositor exposes `zwlr_layer_shell_v1`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn resolve_overlay_backend(
    override_env: Option<&str>,
    is_wayland: bool,
    layer_shell_supported: bool,
) -> OverlayBackendKind {
    match override_env
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("viewport" | "eframe") => OverlayBackendKind::EframeViewport,
        Some("layer-shell" | "layershell" | "wayland") => {
            if layer_shell_supported {
                OverlayBackendKind::WaylandLayerShell
            } else {
                OverlayBackendKind::EframeViewport
            }
        }
        _ => {
            if is_wayland && layer_shell_supported {
                OverlayBackendKind::WaylandLayerShell
            } else {
                OverlayBackendKind::EframeViewport
            }
        }
    }
}

/// Probes the runtime environment and compositor protocols to select the active overlay backend.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn detect_overlay_backend() -> OverlayBackendKind {
    #[cfg(target_os = "linux")]
    {
        let override_env = std::env::var("TRUESIGHT_OVERLAY_BACKEND").ok();
        let is_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let layer_shell_supported = if is_wayland {
            super::wayland_layer::probe_layer_shell_support()
        } else {
            false
        };
        resolve_overlay_backend(override_env.as_deref(), is_wayland, layer_shell_supported)
    }
    #[cfg(not(target_os = "linux"))]
    {
        OverlayBackendKind::EframeViewport
    }
}

/// Applies an interactive pointer drag delta to top-left layer-shell margins and clamps within
/// the active monitor bounds when known.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn compute_drag_margin_step(
    start_margin: (i32, i32),
    delta: (f32, f32),
    overlay_size: (u32, u32),
    monitor_bounds: Option<(i32, i32)>,
) -> (i32, i32) {
    let raw_x = start_margin.0.saturating_add(delta.0.round() as i32);
    let raw_y = start_margin.1.saturating_add(delta.1.round() as i32);
    if let Some((mon_w, mon_h)) = monitor_bounds {
        let max_x = mon_w.saturating_sub(overlay_size.0 as i32).max(0);
        let max_y = mon_h.saturating_sub(overlay_size.1 as i32).max(0);
        (raw_x.clamp(0, max_x), raw_y.clamp(0, max_y))
    } else {
        (raw_x.max(0), raw_y.max(0))
    }
}

/// Applies a south-east corner resize delta and clamps dimensions to overlay min/max bounds.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn compute_resize_step(
    start_size: (u32, u32),
    delta: (f32, f32),
    min_size: (u32, u32),
    max_size: (u32, u32),
) -> (u32, u32) {
    let min_w = min_size.0.min(max_size.0);
    let min_h = min_size.1.min(max_size.1);
    let w = (start_size.0 as f32 + delta.0)
        .round()
        .clamp(min_w as f32, max_size.0 as f32) as u32;
    let h = (start_size.1 as f32 + delta.1)
        .round()
        .clamp(min_h as f32, max_size.1 as f32) as u32;
    (w, h)
}

#[cfg(target_os = "linux")]
impl super::TrueSightGuiApp {
    /// Drives the dedicated Wayland `zwlr_layer_shell_v1` overlay `egui::Context`,
    /// translating presenter events into `RawInput` and pushing dirty `ARGB8888` frames.
    pub(super) fn step_wayland_layer_overlay(&mut self, main_ctx: &egui::Context) {
        let Some(handle) = self.wayland_layer.as_ref() else {
            self.overlay_backend = OverlayBackendKind::EframeViewport;
            return;
        };
        let events = handle.poll_events();

        let mut egui_events = Vec::new();
        for ev in events {
            match ev {
                LayerEvent::Configured {
                    width,
                    height,
                    scale,
                } => {
                    self.wayland_layer_size = (width.max(1), height.max(1));
                    self.wayland_layer_scale = scale.max(1);
                }
                LayerEvent::GeometryChanged {
                    x,
                    y,
                    width,
                    height,
                    persist,
                } => {
                    self.wayland_layer_size = (width.max(1), height.max(1));
                    let geom = crate::session::OverlayGeometry {
                        x: x as f32,
                        y: y as f32,
                        width: width as f32,
                        height: height as f32,
                        click_through: self.session.overlay_geometry.click_through,
                    };
                    if persist {
                        self.session.set_active_overlay_geometry(geom);
                    } else if self.session.is_post_draft_build_view() {
                        self.session.build_overlay_geometry = geom;
                    } else {
                        self.session.overlay_geometry = geom;
                    }
                }
                LayerEvent::MonitorsUpdated(monitors) => {
                    self.session.set_known_monitors(monitors);
                }
                LayerEvent::PointerMoved { x, y } => {
                    egui_events.push(egui::Event::PointerMoved(egui::pos2(x, y)));
                }
                LayerEvent::PointerLeft => {
                    egui_events.push(egui::Event::PointerGone);
                }
                LayerEvent::PointerButton {
                    x,
                    y,
                    button,
                    pressed,
                } => {
                    egui_events.push(egui::Event::PointerButton {
                        pos: egui::pos2(x, y),
                        button,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                LayerEvent::Scroll { dx, dy } => {
                    egui_events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(dx, dy),
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                LayerEvent::Closed => {
                    self.session.set_overlay_mode(false);
                    self.reset_wayland_layer_surface_state();
                    main_ctx.request_repaint();
                }
            }
        }

        if !self.session.overlay_mode {
            if self.wayland_layer_visible {
                if let Some(handle) = self.wayland_layer.as_ref() {
                    handle.send(LayerCommand::Hide);
                }
                self.reset_wayland_layer_surface_state();
            }
            return;
        }

        let is_post_draft = self.session.is_post_draft_build_view();
        let target_geom = self.session.active_overlay_geometry();
        let (min_w, min_h) = if is_post_draft {
            (220_u32, 104_u32)
        } else {
            (280_u32, 160_u32)
        };
        let target_w = (target_geom.width.round() as u32).clamp(min_w, 840);
        let target_h = (target_geom.height.round() as u32).clamp(min_h, 720);
        let target_x = target_geom.x.round() as i32;
        let target_y = target_geom.y.round() as i32;

        if !self.wayland_layer_visible {
            self.wayland_layer_visible = true;
            self.wayland_layer_size = (target_w, target_h);
            self.last_overlay_is_post_draft = Some(is_post_draft);
            self.wayland_last_hit_regions = (None, None);
            self.wayland_last_cursor = None;
            self.overlay_rasterizer.invalidate();
            if let Some(handle) = self.wayland_layer.as_ref() {
                handle.send(LayerCommand::Show {
                    x: target_x,
                    y: target_y,
                    width: target_w,
                    height: target_h,
                    min_w,
                    min_h,
                    click_through: target_geom.click_through,
                });
            }
        } else if self
            .last_overlay_is_post_draft
            .is_some_and(|prev| prev != is_post_draft)
        {
            self.last_overlay_is_post_draft = Some(is_post_draft);
            self.wayland_layer_size = (target_w, target_h);
            if let Some(handle) = self.wayland_layer.as_ref() {
                handle.send(LayerCommand::SetGeometry {
                    x: target_x,
                    y: target_y,
                    width: target_w,
                    height: target_h,
                    min_w,
                    min_h,
                });
            }
        }

        let (logical_w, logical_h) = self.wayland_layer_size;
        let scale = self.wayland_layer_scale.max(1);
        let ppp = scale as f32;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(logical_w as f32, logical_h as f32),
            )),
            time: main_ctx.input(|i| Some(i.time)),
            events: egui_events,
            focused: !self.session.overlay_geometry.click_through,
            ..Default::default()
        };
        raw.viewports
            .entry(raw.viewport_id)
            .or_default()
            .native_pixels_per_point = Some(ppp);

        let overlay_ctx = self.overlay_ctx.clone();
        overlay_ctx.set_pixels_per_point(ppp);
        let mut panel_out = None;
        let full_output = overlay_ctx.run(raw, |ctx| {
            if !self.overlay_ctx_style_configured {
                Self::configure_visuals_for(ctx);
                self.overlay_ctx_style_configured = true;
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    panel_out =
                        Some(self.render_overlay_panel_with_host(ui, OverlayHostMode::LayerShell));
                });
        });

        // Clicking the close button inside the overlay turns off overlay mode immediately.
        if !self.session.overlay_mode {
            if self.wayland_layer_visible {
                if let Some(handle) = self.wayland_layer.as_ref() {
                    handle.send(LayerCommand::Hide);
                }
                self.reset_wayland_layer_surface_state();
            }
            main_ctx.request_repaint();
            return;
        }

        let hit_regions = if self.session.overlay_geometry.click_through {
            (None, None)
        } else if let Some(out) = panel_out {
            (
                Some(OverlayRect::from(out.drag_hit_rect)),
                Some(OverlayRect::from(out.resize_handle)),
            )
        } else {
            (None, None)
        };
        if hit_regions != self.wayland_last_hit_regions {
            self.wayland_last_hit_regions = hit_regions;
            if let Some(handle) = self.wayland_layer.as_ref() {
                handle.send(LayerCommand::SetHitRegions {
                    drag_rect: hit_regions.0,
                    resize_rect: hit_regions.1,
                });
            }
        }

        let cursor = full_output.platform_output.cursor_icon;
        if self.wayland_last_cursor != Some(cursor) {
            self.wayland_last_cursor = Some(cursor);
            if let Some(handle) = self.wayland_layer.as_ref() {
                handle.send(LayerCommand::SetCursor(cursor));
            }
        }

        self.overlay_rasterizer
            .apply_textures_delta(&full_output.textures_delta);
        let prims = overlay_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let width_px = logical_w.saturating_mul(scale as u32);
        let height_px = logical_h.saturating_mul(scale as u32);
        if let Some(argb) = self
            .overlay_rasterizer
            .rasterize_if_dirty(width_px, height_px, ppp, &prims)
            && let Some(handle) = self.wayland_layer.as_ref()
        {
            handle.send(LayerCommand::PresentFrame {
                width_px,
                height_px,
                scale,
                argb8888: argb.to_vec(),
            });
        }

        if let Some(vout) = full_output.viewport_output.get(&overlay_ctx.viewport_id())
            && vout.repaint_delay <= std::time::Duration::from_millis(80)
        {
            main_ctx.request_repaint_after(vout.repaint_delay);
        }
    }

    fn reset_wayland_layer_surface_state(&mut self) {
        self.wayland_layer_visible = false;
        self.last_overlay_is_post_draft = None;
        self.wayland_last_hit_regions = (None, None);
        self.wayland_last_cursor = None;
        self.overlay_rasterizer.invalidate();
    }
}

impl super::TrueSightGuiApp {
    /// Renders the secondary `eframe` overlay viewport used on GNOME/Mutter, X11, Windows, and macOS.
    pub(super) fn render_viewport_overlay(
        &mut self,
        ctx: &egui::Context,
        f8_root: bool,
        pass_toggled_this_frame: bool,
    ) {
        let overlay_id = egui::ViewportId::from_hash_of("truesight_compact_overlay");
        let is_post_draft = self.session.is_post_draft_build_view();
        let target_geom = self.session.active_overlay_geometry();
        let builder = super::overlay::build_overlay_viewport_builder(target_geom, is_post_draft);
        ctx.show_viewport_immediate(overlay_id, builder, |vctx, _class| {
            if self
                .last_overlay_is_post_draft
                .is_some_and(|prev| prev != is_post_draft)
            {
                self.overlay_wm_gesture_cooldown = 2;
                let min_sz = if is_post_draft {
                    egui::vec2(220.0, 104.0)
                } else {
                    egui::vec2(280.0, 160.0)
                };
                vctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min_sz));
                vctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    target_geom.width,
                    target_geom.height,
                )));
                let has_pos = vctx.input(|i| {
                    let vp = i.viewport();
                    vp.outer_rect.is_some() || vp.inner_rect.is_some()
                });
                if has_pos {
                    vctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                        target_geom.x,
                        target_geom.y,
                    )));
                }
            }
            self.last_overlay_is_post_draft = Some(is_post_draft);

            if !f8_root && Self::is_non_repeating_key_pressed(vctx, egui::Key::F8) {
                self.session.set_overlay_mode(false);
                ctx.request_repaint();
            }
            if !pass_toggled_this_frame && Self::is_pass_shortcut_pressed(vctx) {
                let next = !self.session.overlay_geometry.click_through;
                self.set_overlay_click_through_live(ctx, next);
            }

            self.sync_live_overlay_viewport_geometry(vctx);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(vctx, |ui| {
                    let _ = self.render_overlay_panel(ui);
                });

            let click_through = self.session.overlay_geometry.click_through;
            if self.overlay_applied_click_through != Some(click_through) {
                vctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(click_through));
                self.overlay_applied_click_through = Some(click_through);
                self.overlay_passthrough_repaint_frames = 2;
                ctx.request_repaint();
                ctx.request_repaint_of(overlay_id);
            } else if self.overlay_passthrough_repaint_frames > 0 {
                self.overlay_passthrough_repaint_frames -= 1;
                if !click_through {
                    vctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(false));
                }
                ctx.request_repaint();
                ctx.request_repaint_of(overlay_id);
            }
        });
    }
}
