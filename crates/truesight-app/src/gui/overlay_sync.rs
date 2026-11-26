use super::TrueSightGuiApp;
use crate::session::OverlayGeometry;
use eframe::egui;

const OVERLAY_WM_GESTURE_SETTLE_FRAMES: u8 = 4;
const OVERLAY_LIVE_MOTION_SETTLE_FRAMES: u8 = 3;

/// Tracks active OS window-manager drag/resize gestures and fractional-DPI quantization
/// across frames so viewport clamping settles cleanly instead of oscillating with Win32
/// `WM_SIZING` / `WM_MOVING` loops.
#[derive(Default)]
pub(crate) struct OverlayViewportSyncState {
    wm_gesture_active: bool,
    wm_gesture_awaiting_end: bool,
    wm_settle_frames: u8,
    motion_settle_frames: u8,
    consecutive_motion_frames: u8,
    last_live_pts: Option<OverlayGeometry>,
    last_sent_pos: Option<((f32, f32), (f32, f32))>,
    last_sent_size: Option<((f32, f32), (f32, f32))>,
}

impl OverlayViewportSyncState {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn begin_wm_gesture(&mut self) {
        self.wm_gesture_active = true;
        self.wm_gesture_awaiting_end = true;
        self.wm_settle_frames = OVERLAY_WM_GESTURE_SETTLE_FRAMES;
        self.last_sent_pos = None;
        self.last_sent_size = None;
    }

    /// Transitions an active OS drag/resize gesture into its post-release settle countdown.
    pub(super) fn end_wm_gesture(&mut self) {
        if self.wm_gesture_awaiting_end {
            self.wm_gesture_awaiting_end = false;
            self.wm_settle_frames = OVERLAY_WM_GESTURE_SETTLE_FRAMES;
        }
    }

    pub(super) fn begin_programmatic_move(&mut self) {
        self.wm_gesture_active = false;
        self.wm_gesture_awaiting_end = false;
        self.wm_settle_frames = 2;
        self.motion_settle_frames = 0;
        self.consecutive_motion_frames = 0;
        self.last_live_pts = None;
        self.last_sent_pos = None;
        self.last_sent_size = None;
    }

    fn is_settling(&self) -> bool {
        self.wm_gesture_active || self.wm_settle_frames > 0 || self.motion_settle_frames > 0
    }

    fn needs_settle_repaint(&self) -> bool {
        !self.wm_gesture_awaiting_end
            && (self.wm_settle_frames > 0 || self.motion_settle_frames > 0)
    }

    fn observe_candidate(&mut self, candidate: OverlayGeometry, pos_tol: f32) {
        let motion_eps = 0.5_f32;
        let matches_self_pos = self.last_sent_pos.is_some_and(|(target, _)| {
            (candidate.x - target.0).abs() <= pos_tol && (candidate.y - target.1).abs() <= pos_tol
        });
        let matches_self_size = self.last_sent_size.is_some_and(|(target, _)| {
            (candidate.width - target.0).abs() <= pos_tol
                && (candidate.height - target.1).abs() <= pos_tol
        });

        let moved = self.last_live_pts.is_some_and(|prev| {
            let pos_moved = ((candidate.x - prev.x).abs() > motion_eps
                || (candidate.y - prev.y).abs() > motion_eps)
                && !matches_self_pos;
            let size_moved = ((candidate.width - prev.width).abs() > motion_eps
                || (candidate.height - prev.height).abs() > motion_eps)
                && !matches_self_size;
            pos_moved || size_moved
        });

        if moved {
            if self.wm_gesture_active {
                self.wm_settle_frames = OVERLAY_WM_GESTURE_SETTLE_FRAMES;
            } else {
                self.consecutive_motion_frames = self.consecutive_motion_frames.saturating_add(1);
                if self.consecutive_motion_frames >= 2 {
                    self.motion_settle_frames = OVERLAY_LIVE_MOTION_SETTLE_FRAMES;
                }
            }
        } else {
            self.consecutive_motion_frames = 0;
            // Keep clamping suppressed indefinitely while the user pauses mid-grab before releasing.
            if !self.wm_gesture_awaiting_end {
                if self.wm_settle_frames > 0 {
                    self.wm_settle_frames -= 1;
                }
                if self.wm_settle_frames == 0 {
                    self.wm_gesture_active = false;
                }
            }
            if self.motion_settle_frames > 0 {
                self.motion_settle_frames -= 1;
            }
        }
        self.last_live_pts = Some(candidate);
    }
}

fn should_suppress_repeated_cmd(
    last: Option<((f32, f32), (f32, f32))>,
    target: (f32, f32),
    candidate: (f32, f32),
    pos_tol: f32,
) -> bool {
    let Some((prev_target, prev_candidate)) = last else {
        return false;
    };
    let same_target =
        (target.0 - prev_target.0).abs() <= pos_tol && (target.1 - prev_target.1).abs() <= pos_tol;
    let same_candidate = (candidate.0 - prev_candidate.0).abs() <= pos_tol * 2.0
        && (candidate.1 - prev_candidate.1).abs() <= pos_tol * 2.0;
    same_target && same_candidate
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
        // One physical-pixel rounding slack prevents Win32 SetWindowPos / DWM fractional-DPI
        // quantization (e.g. 1.25x, 1.5x, 1.75x) from triggering repeated 1px ping-pong commands.
        let pos_tol = (1.0 / ppp).max(0.5) + 0.05;

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
        self.overlay_viewport_sync
            .observe_candidate(candidate_pts, pos_tol);

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

        // Avoid sending corrective position/size commands while a drag/resize or settle window is active.
        let (primary_down, wm_gesture_ended) = vctx.input(|i| {
            let ended = i.pointer.primary_released()
                || i.pointer.any_released()
                || i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::PointerMoved(_)
                            | egui::Event::PointerGone
                            | egui::Event::WindowFocused(false)
                            | egui::Event::PointerButton { pressed: false, .. }
                    )
                });
            (i.pointer.primary_down(), ended)
        });
        if !primary_down && wm_gesture_ended {
            self.overlay_viewport_sync.end_wm_gesture();
        }
        let actively_interacting = self.overlay_drag_armed.is_some()
            || self.overlay_resize_armed.is_some()
            || self.overlay_viewport_sync.is_settling()
            || primary_down;
        if actively_interacting {
            if self.overlay_viewport_sync.needs_settle_repaint() {
                vctx.request_repaint_after(std::time::Duration::from_millis(32));
            }
            return;
        }

        let need_pos = has_live_position
            && ((clamped_pts.x - candidate_pts.x).abs() > pos_tol
                || (clamped_pts.y - candidate_pts.y).abs() > pos_tol);
        if need_pos {
            let target = (clamped_pts.x, clamped_pts.y);
            let candidate = (candidate_pts.x, candidate_pts.y);
            if !should_suppress_repeated_cmd(
                self.overlay_viewport_sync.last_sent_pos,
                target,
                candidate,
                pos_tol,
            ) {
                self.overlay_viewport_sync.last_sent_pos = Some((target, candidate));
                vctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                    clamped_pts.x,
                    clamped_pts.y,
                )));
            }
        } else {
            self.overlay_viewport_sync.last_sent_pos = None;
        }

        let need_size = (clamped_pts.width - candidate_pts.width).abs() > pos_tol
            || (clamped_pts.height - candidate_pts.height).abs() > pos_tol;
        if need_size {
            let target = (clamped_pts.width, clamped_pts.height);
            let candidate = (candidate_pts.width, candidate_pts.height);
            if !should_suppress_repeated_cmd(
                self.overlay_viewport_sync.last_sent_size,
                target,
                candidate,
                pos_tol,
            ) {
                self.overlay_viewport_sync.last_sent_size = Some((target, candidate));
                vctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    clamped_pts.width,
                    clamped_pts.height,
                )));
            }
        } else {
            self.overlay_viewport_sync.last_sent_size = None;
        }

        if (clamped_pts.x - active_geom.x).abs() > pos_tol
            || (clamped_pts.y - active_geom.y).abs() > pos_tol
            || (clamped_pts.width - active_geom.width).abs() > pos_tol
            || (clamped_pts.height - active_geom.height).abs() > pos_tol
        {
            self.session.set_active_overlay_geometry(clamped_pts);
        }
    }
}
