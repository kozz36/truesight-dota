use super::super::overlay_backend::{
    LayerEvent, compute_drag_margin_step, compute_resize_step, resolve_cross_monitor_drag_target,
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

impl PointerHandler for WaylandLayerState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let outputs = self.logical_outputs();
        let mut pending_events = Vec::new();
        let mut enter_seen = false;
        let mut migrate_target = None;
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
                                            migrate_target = Some(target);
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
                            } => {
                                let delta = (px - start_local.0, py - start_local.1);
                                let next = compute_resize_step(
                                    start_size,
                                    delta,
                                    (active.min_w, active.min_h),
                                    (MAX_OVERLAY_W, MAX_OVERLAY_H),
                                );
                                if next != (active.width, active.height) {
                                    active.width = next.0;
                                    active.height = next.1;
                                    active.layer.set_size(active.width, active.height);
                                    active.layer.commit();
                                    pending_events.push(LayerEvent::GeometryChanged {
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
                            active.interaction = InteractionMode::None;
                            pending_events.push(LayerEvent::GeometryChanged {
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
                            active.interaction = InteractionMode::None;
                            pending_events.push(LayerEvent::GeometryChanged {
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

        if let Some(target) = migrate_target {
            self.migrate_to_output(target, None, true);
            let _ = self.conn.flush();
            return;
        }

        if !pending_events.is_empty() {
            for ev in pending_events {
                self.emit(ev);
            }
            let _ = self.conn.flush();
        }
    }
}
