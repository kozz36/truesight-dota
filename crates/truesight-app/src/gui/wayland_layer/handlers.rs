use super::{InteractionMode, LayerEvent, MAX_OVERLAY_H, MAX_OVERLAY_W, WaylandLayerState};
use smithay_client_toolkit::compositor::CompositorHandler;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_seat, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::pointer::ThemeSpec;
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, registry_handlers,
};
use std::num::NonZeroU32;

impl CompositorHandler for WaylandLayerState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let scale = new_factor.max(1);
        let configured_info = if let Some(active) = self.active_surface.as_mut()
            && active.layer.wl_surface() == surface
        {
            active.scale = scale;
            active
                .configured
                .then_some((active.width, active.height, active.scale))
        } else {
            None
        };
        if let Some((width, height, scale)) = configured_info {
            self.emit(LayerEvent::Configured {
                width,
                height,
                scale,
            });
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if let Some(active) = self.active_surface.as_mut()
            && active.layer.wl_surface() == surface
            && let InteractionMode::Dragging {
                ref mut frame_pending,
                ..
            } = active.interaction
        {
            *frame_pending = false;
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        let info = self.output_state.info(output);
        let configured_info = if let Some(active) = self.active_surface.as_mut()
            && active.layer.wl_surface() == surface
            && let Some(info) = info
        {
            active.output_id = Some(info.id);
            active.output_origin = info.logical_position.unwrap_or(active.output_origin);
            if let Some(sz) = info.logical_size.filter(|&(w, h)| w > 0 && h > 0) {
                active.output_size = Some(sz);
            }
            let sf = info.scale_factor.max(1);
            let scale_changed = sf != active.scale;
            active.scale = sf;
            (active.configured && scale_changed).then_some((
                active.width,
                active.height,
                active.scale,
            ))
        } else {
            None
        };
        if let Some((width, height, scale)) = configured_info {
            self.emit(LayerEvent::Configured {
                width,
                height,
                scale,
            });
        }
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for WaylandLayerState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        self.emit_monitors();
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        self.emit_monitors();
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.emit_monitors_excluding(Some(&output));
    }
}

impl LayerShellHandler for WaylandLayerState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        if self.drag_anchor.as_ref().is_some_and(|a| &a.layer == layer) {
            self.finish_anchored_drag();
            return;
        }
        if self
            .active_surface
            .as_ref()
            .is_some_and(|a| &a.layer == layer)
        {
            self.pending_set_geometry.clear();
            self.drag_anchor = None;
            self.active_surface = None;
            self.emit(LayerEvent::Closed);
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let configured_info = if let Some(active) = self.active_surface.as_mut()
            && &active.layer == layer
        {
            active.width = NonZeroU32::new(configure.new_size.0)
                .map_or(active.width, NonZeroU32::get)
                .max(active.min_w)
                .min(MAX_OVERLAY_W);
            active.height = NonZeroU32::new(configure.new_size.1)
                .map_or(active.height, NonZeroU32::get)
                .max(active.min_h)
                .min(MAX_OVERLAY_H);
            active.configured = true;
            if let InteractionMode::Resizing { ref mut pacer, .. } = active.interaction {
                pacer.on_configured((active.width, active.height));
            }
            Some((active.width, active.height, active.scale))
        } else {
            None
        };

        if let Some((width, height, scale)) = configured_info {
            self.emit(LayerEvent::Configured {
                width,
                height,
                scale,
            });
            if self.commit_pending_frame() {
                self.advance_queued_resize();
            }
        }
    }
}

impl SeatHandler for WaylandLayerState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && self.themed_pointer.is_none()
            && self.plain_pointer.is_none()
        {
            let cursor_surface = self.compositor_state.create_surface(qh);
            if let Ok(themed) = self.seat_state.get_pointer_with_theme(
                qh,
                &seat,
                self.shm.wl_shm(),
                cursor_surface,
                ThemeSpec::default(),
            ) {
                self.themed_pointer = Some(themed);
            } else if let Ok(ptr) = self.seat_state.get_pointer(qh, &seat) {
                self.plain_pointer = Some(ptr);
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(themed) = self.themed_pointer.take() {
                themed.pointer().release();
            }
            if let Some(ptr) = self.plain_pointer.take() {
                ptr.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl ShmHandler for WaylandLayerState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(WaylandLayerState);
delegate_output!(WaylandLayerState);
delegate_shm!(WaylandLayerState);
delegate_seat!(WaylandLayerState);
delegate_pointer!(WaylandLayerState);
delegate_layer!(WaylandLayerState);
delegate_registry!(WaylandLayerState);

impl ProvidesRegistryState for WaylandLayerState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}
