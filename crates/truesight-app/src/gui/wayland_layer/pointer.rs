use super::super::overlay_backend::{
    LayerEvent, compute_anchored_drag_desktop_pos, compute_drag_margin_step, compute_resize_step,
    resolve_cross_monitor_drag_target,
};
use super::{
    BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, InteractionMode, MAX_OVERLAY_H, MAX_OVERLAY_W,
    WaylandLayerState,
};
use eframe::egui;
use smithay_client_toolkit::reexports::client::protocol::wl_pointer;
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::seat::pointer::{
    CursorIcon as SctkCursorIcon, PointerEvent, PointerEventKind, PointerHandler,
};
use smithay_client_toolkit::shell::WaylandSurface;

pub(super) fn map_egui_cursor(icon: egui::CursorIcon) -> SctkCursorIcon {
    match icon {
        egui::CursorIcon::PointingHand => SctkCursorIcon::Pointer,
        egui::CursorIcon::Grab => SctkCursorIcon::Grab,
        egui::CursorIcon::Grabbing => SctkCursorIcon::Grabbing,
        egui::CursorIcon::ResizeSouthEast => SctkCursorIcon::NwseResize,
        egui::CursorIcon::Text => SctkCursorIcon::Text,
        egui::CursorIcon::NoDrop | egui::CursorIcon::NotAllowed => SctkCursorIcon::NotAllowed,
        _ => SctkCursorIcon::Default,
    }
}

pub(super) fn map_wayland_button(button: u32) -> Option<egui::PointerButton> {
    match button {
        BTN_LEFT => Some(egui::PointerButton::Primary),
        BTN_RIGHT => Some(egui::PointerButton::Secondary),
        BTN_MIDDLE => Some(egui::PointerButton::Middle),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) struct PendingSetGeometry {
    pub(in crate::gui) is_post_draft: bool,
    pub(in crate::gui) x: i32,
    pub(in crate::gui) y: i32,
    pub(in crate::gui) width: u32,
    pub(in crate::gui) height: u32,
    pub(in crate::gui) min_w: u32,
    pub(in crate::gui) min_h: u32,
}

/// Buffers view transitions (`LayerCommand::SetGeometry`) sent while a native pointer drag or
/// resize is active so the gesture finishes cleanly under the view that started it and the target
/// view geometry applies immediately on release.
#[derive(Debug, Default, Clone, Copy)]
pub(in crate::gui) struct DeferredGeometryQueue {
    pending: Option<PendingSetGeometry>,
}

impl DeferredGeometryQueue {
    pub(in crate::gui) fn clear(&mut self) {
        self.pending = None;
    }

    pub(in crate::gui) fn on_set_geometry(
        &mut self,
        gesture_active: bool,
        req: PendingSetGeometry,
    ) -> Option<PendingSetGeometry> {
        if gesture_active {
            self.pending = Some(req);
            None
        } else {
            self.pending = None;
            Some(req)
        }
    }

    pub(in crate::gui) fn on_gesture_finished(
        &mut self,
        gesture_active: bool,
    ) -> Option<PendingSetGeometry> {
        if gesture_active {
            None
        } else {
            self.pending.take()
        }
    }
}

/// Coalesces high-rate Wayland pointer resize motions so at most one un-presented size transition
/// is in flight at a time, preventing stale-dimension `PresentFrame` drops while animating smoothly.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) struct ResizePacer {
    frame_in_flight: Option<(u32, u32)>,
    queued_size: Option<(u32, u32)>,
}

impl ResizePacer {
    pub(in crate::gui) fn new() -> Self {
        Self::default()
    }

    pub(in crate::gui) fn on_motion(
        &mut self,
        current: (u32, u32),
        next: (u32, u32),
    ) -> Option<(u32, u32)> {
        if self.frame_in_flight.is_some() {
            self.queued_size = (next != current).then_some(next);
            return None;
        }
        if next == current {
            return None;
        }
        self.frame_in_flight = Some(next);
        self.queued_size = None;
        Some(next)
    }

    pub(in crate::gui) fn on_configured(&mut self, configured: (u32, u32)) {
        if self.frame_in_flight.is_some() {
            self.frame_in_flight = Some(configured);
        }
    }

    pub(in crate::gui) fn on_frame_committed(
        &mut self,
        committed: (u32, u32),
    ) -> Option<(u32, u32)> {
        if let Some(inflight) = self.frame_in_flight
            && inflight != committed
        {
            return None;
        }
        if let Some(queued) = self.queued_size.take()
            && queued != committed
        {
            self.frame_in_flight = Some(queued);
            Some(queued)
        } else {
            self.frame_in_flight = None;
            None
        }
    }

    pub(in crate::gui) fn on_finish(&mut self, current: (u32, u32)) -> Option<(u32, u32)> {
        self.frame_in_flight = None;
        self.queued_size.take().filter(|&sz| sz != current)
    }
}

impl WaylandLayerState {
    fn handle_anchored_pointer_events(
        &mut self,
        events: &[PointerEvent],
        outputs: &[super::super::overlay_backend::LogicalOutputBounds],
    ) {
        let Some((anchor_surface, anchor_origin, grab_local)) = self
            .drag_anchor
            .as_ref()
            .map(|a| (a.layer.wl_surface().clone(), a.desktop_origin, a.grab_local))
        else {
            return;
        };

        for event in events {
            if event.surface != anchor_surface {
                continue;
            }
            let px = event.position.0 as f32;
            let py = event.position.1 as f32;
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    let (ptr_desktop, desired_desktop) =
                        compute_anchored_drag_desktop_pos(anchor_origin, grab_local, (px, py));
                    let migrate_target = self.active_surface.as_ref().and_then(|active| {
                        let cur_size = active.output_size?;
                        resolve_cross_monitor_drag_target(
                            active.output_id,
                            active.output_origin,
                            cur_size,
                            desired_desktop,
                            ptr_desktop,
                            (active.width, active.height),
                            outputs,
                        )
                    });
                    if let Some(target) = migrate_target {
                        self.migrate_to_output(target, None, None, false);
                        continue;
                    }
                    let moved_geom = if let Some(active) = self.active_surface.as_mut() {
                        let raw_mx = desired_desktop.0.saturating_sub(active.output_origin.0);
                        let raw_my = desired_desktop.1.saturating_sub(active.output_origin.1);
                        let (mx, my) = compute_drag_margin_step(
                            (raw_mx, raw_my),
                            (0.0, 0.0),
                            (active.width, active.height),
                            active.output_size,
                        );
                        if (mx, my) != (active.margin_x, active.margin_y) {
                            active.margin_x = mx;
                            active.margin_y = my;
                            active.layer.set_margin(my, 0, 0, mx);
                            active.layer.commit();
                            Some((
                                active.is_post_draft,
                                active.output_origin.0 + mx,
                                active.output_origin.1 + my,
                                active.width,
                                active.height,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some((is_post_draft, x, y, width, height)) = moved_geom {
                        self.emit(LayerEvent::GeometryChanged {
                            is_post_draft,
                            x,
                            y,
                            width,
                            height,
                            persist: false,
                        });
                    }
                }
                PointerEventKind::Release {
                    button: BTN_LEFT, ..
                }
                | PointerEventKind::Leave { .. } => {
                    self.finish_anchored_drag();
                    let _ = self.conn.flush();
                    return;
                }
                _ => {}
            }
        }
        let _ = self.conn.flush();
    }
    pub(super) fn advance_queued_resize(&mut self) {
        let next_event = if let Some(active) = self.active_surface.as_mut()
            && let InteractionMode::Resizing { ref mut pacer, .. } = active.interaction
            && let Some((w, h)) = pacer.on_frame_committed((active.width, active.height))
        {
            active.width = w;
            active.height = h;
            active.layer.set_size(active.width, active.height);
            active.layer.commit();
            Some(LayerEvent::GeometryChanged {
                is_post_draft: active.is_post_draft,
                x: active.output_origin.0 + active.margin_x,
                y: active.output_origin.1 + active.margin_y,
                width: active.width,
                height: active.height,
                persist: false,
            })
        } else {
            None
        };
        if let Some(ev) = next_event {
            self.emit(ev);
        }
    }
}

impl PointerHandler for WaylandLayerState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let outputs = self.logical_outputs();
        if self.drag_anchor.is_some() {
            self.handle_anchored_pointer_events(events, &outputs);
            return;
        }

        let mut pending_events = Vec::new();
        let mut enter_seen = false;
        let mut gesture_finished = false;
        let mut migrate_target: Option<(
            super::super::overlay_backend::CrossMonitorTarget,
            (f32, f32),
        )> = None;
        {
            let Some(active) = self.active_surface.as_mut() else {
                return;
            };
            let target_surface = active.layer.wl_surface().clone();
            for event in events {
                if event.surface != target_surface {
                    continue;
                }
                let px = event.position.0 as f32;
                let py = event.position.1 as f32;
                match event.kind {
                    PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                        if matches!(event.kind, PointerEventKind::Enter { .. }) {
                            enter_seen = true;
                        }
                        match active.interaction {
                            InteractionMode::Dragging {
                                grab_local,
                                ref mut frame_pending,
                            } => {
                                if !*frame_pending {
                                    let delta = (px - grab_local.0, py - grab_local.1);
                                    let raw_dx = delta.0.round() as i32;
                                    let raw_dy = delta.1.round() as i32;
                                    if let Some(cur_size) = active.output_size {
                                        let desired_desktop = (
                                            active.output_origin.0 + active.margin_x + raw_dx,
                                            active.output_origin.1 + active.margin_y + raw_dy,
                                        );
                                        let pointer_desktop = (
                                            active.output_origin.0
                                                + active.margin_x
                                                + px.round() as i32,
                                            active.output_origin.1
                                                + active.margin_y
                                                + py.round() as i32,
                                        );
                                        if let Some(target) = resolve_cross_monitor_drag_target(
                                            active.output_id,
                                            active.output_origin,
                                            cur_size,
                                            desired_desktop,
                                            pointer_desktop,
                                            (active.width, active.height),
                                            &outputs,
                                        ) {
                                            migrate_target = Some((target, grab_local));
                                            break;
                                        }
                                    }
                                    let next = compute_drag_margin_step(
                                        (active.margin_x, active.margin_y),
                                        delta,
                                        (active.width, active.height),
                                        active.output_size,
                                    );
                                    if next != (active.margin_x, active.margin_y) {
                                        active.margin_x = next.0;
                                        active.margin_y = next.1;
                                        active.layer.set_margin(
                                            active.margin_y,
                                            0,
                                            0,
                                            active.margin_x,
                                        );
                                        active.layer.wl_surface().frame(qh, target_surface.clone());
                                        active.layer.commit();
                                        *frame_pending = true;
                                        pending_events.push(LayerEvent::GeometryChanged {
                                            is_post_draft: active.is_post_draft,
                                            x: active.output_origin.0 + active.margin_x,
                                            y: active.output_origin.1 + active.margin_y,
                                            width: active.width,
                                            height: active.height,
                                            persist: false,
                                        });
                                    }
                                }
                            }
                            InteractionMode::Resizing {
                                start_local,
                                start_size,
                                ref mut pacer,
                            } => {
                                let delta = (px - start_local.0, py - start_local.1);
                                let next = compute_resize_step(
                                    start_size,
                                    delta,
                                    (active.min_w, active.min_h),
                                    (MAX_OVERLAY_W, MAX_OVERLAY_H),
                                );
                                if let Some((w, h)) =
                                    pacer.on_motion((active.width, active.height), next)
                                {
                                    active.width = w;
                                    active.height = h;
                                    active.layer.set_size(active.width, active.height);
                                    active.layer.commit();
                                    pending_events.push(LayerEvent::GeometryChanged {
                                        is_post_draft: active.is_post_draft,
                                        x: active.output_origin.0 + active.margin_x,
                                        y: active.output_origin.1 + active.margin_y,
                                        width: active.width,
                                        height: active.height,
                                        persist: false,
                                    });
                                }
                            }
                            InteractionMode::None => {
                                pending_events.push(LayerEvent::PointerMoved { x: px, y: py });
                            }
                        }
                    }
                    PointerEventKind::Leave { .. } => {
                        if !matches!(active.interaction, InteractionMode::None) {
                            if let InteractionMode::Resizing { ref mut pacer, .. } =
                                active.interaction
                                && let Some((w, h)) = pacer.on_finish((active.width, active.height))
                            {
                                active.width = w;
                                active.height = h;
                                active.layer.set_size(active.width, active.height);
                                active.layer.commit();
                            }
                            active.interaction = InteractionMode::None;
                            gesture_finished = true;
                            pending_events.push(LayerEvent::GeometryChanged {
                                is_post_draft: active.is_post_draft,
                                x: active.output_origin.0 + active.margin_x,
                                y: active.output_origin.1 + active.margin_y,
                                width: active.width,
                                height: active.height,
                                persist: true,
                            });
                        }
                        pending_events.push(LayerEvent::PointerLeft);
                    }
                    PointerEventKind::Press { button, .. } => {
                        if active.click_through {
                            continue;
                        }
                        if button == BTN_LEFT {
                            if active.resize_rect.is_some_and(|r| r.contains(px, py)) {
                                active.interaction = InteractionMode::Resizing {
                                    start_local: (px, py),
                                    start_size: (active.width, active.height),
                                    pacer: ResizePacer::new(),
                                };
                                continue;
                            }
                            if active.drag_rect.is_some_and(|r| r.contains(px, py)) {
                                active.interaction = InteractionMode::Dragging {
                                    grab_local: (px, py),
                                    frame_pending: false,
                                };
                                continue;
                            }
                        }
                        if let Some(btn) = map_wayland_button(button) {
                            pending_events.push(LayerEvent::PointerButton {
                                x: px,
                                y: py,
                                button: btn,
                                pressed: true,
                            });
                        }
                    }
                    PointerEventKind::Release { button, .. } => {
                        if button == BTN_LEFT
                            && !matches!(active.interaction, InteractionMode::None)
                        {
                            if let InteractionMode::Resizing { ref mut pacer, .. } =
                                active.interaction
                                && let Some((w, h)) = pacer.on_finish((active.width, active.height))
                            {
                                active.width = w;
                                active.height = h;
                                active.layer.set_size(active.width, active.height);
                                active.layer.commit();
                            }
                            active.interaction = InteractionMode::None;
                            gesture_finished = true;
                            pending_events.push(LayerEvent::GeometryChanged {
                                is_post_draft: active.is_post_draft,
                                x: active.output_origin.0 + active.margin_x,
                                y: active.output_origin.1 + active.margin_y,
                                width: active.width,
                                height: active.height,
                                persist: true,
                            });
                            continue;
                        }
                        if let Some(btn) = map_wayland_button(button) {
                            pending_events.push(LayerEvent::PointerButton {
                                x: px,
                                y: py,
                                button: btn,
                                pressed: false,
                            });
                        }
                    }
                    PointerEventKind::Axis {
                        horizontal,
                        vertical,
                        ..
                    } => {
                        let dy = if vertical.absolute.abs() > f64::EPSILON {
                            -vertical.absolute as f32
                        } else {
                            -(vertical.discrete as f32) * 18.0
                        };
                        let dx = if horizontal.absolute.abs() > f64::EPSILON {
                            -horizontal.absolute as f32
                        } else {
                            -(horizontal.discrete as f32) * 18.0
                        };
                        if dx.abs() > 0.01 || dy.abs() > 0.01 {
                            pending_events.push(LayerEvent::Scroll { dx, dy });
                        }
                    }
                }
            }
        }

        if enter_seen && let Some(pointer) = &self.themed_pointer {
            let _ = pointer.set_cursor(&self.conn, map_egui_cursor(self.current_cursor));
        }

        if let Some((target, grab_local)) = migrate_target {
            self.begin_anchored_cross_monitor_drag(target, grab_local);
            let _ = self.conn.flush();
            return;
        }

        if !pending_events.is_empty() {
            for ev in pending_events {
                self.emit(ev);
            }
            if gesture_finished {
                self.flush_pending_set_geometry();
            }
            let _ = self.conn.flush();
        } else if gesture_finished {
            self.flush_pending_set_geometry();
            let _ = self.conn.flush();
        }
    }
}
