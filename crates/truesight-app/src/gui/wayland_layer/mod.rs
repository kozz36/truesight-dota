use super::overlay_backend::{LayerCommand, LayerEvent, OverlayRect, compute_drag_margin_step};
use crate::session::MonitorRect;
use eframe::egui;
mod pointer;

use pointer::map_egui_cursor;
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::channel::Event as ChannelEvent;
use smithay_client_toolkit::reexports::calloop::{self, EventLoop, LoopSignal};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{
    wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::pointer::{ThemeSpec, ThemedPointer};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, registry_handlers,
};
use std::num::NonZeroU32;
use std::sync::mpsc;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const MAX_OVERLAY_W: u32 = 840;
const MAX_OVERLAY_H: u32 = 720;

struct ProbeState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ProbeState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

/// Probes the active Wayland display for `zwlr_layer_shell_v1` support without opening a window.
pub(super) fn probe_layer_shell_support() -> bool {
    let Ok(conn) = Connection::connect_to_env() else {
        return false;
    };
    let Ok((globals, _)) = registry_queue_init::<ProbeState>(&conn) else {
        return false;
    };
    globals
        .contents()
        .clone_list()
        .iter()
        .any(|g| g.interface == "zwlr_layer_shell_v1")
}

/// Non-blocking handle to the background Wayland `zwlr_layer_shell_v1` presenter thread.
pub(super) struct WaylandLayerHandle {
    cmd_tx: calloop::channel::Sender<LayerCommand>,
    event_rx: mpsc::Receiver<LayerEvent>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for WaylandLayerHandle {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(LayerCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl WaylandLayerHandle {
    /// Connects to Wayland, binds `zwlr_layer_shell_v1` + `wl_shm`, and spawns the presenter loop.
    pub(super) fn start(main_ctx: egui::Context) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = calloop::channel::channel::<LayerCommand>();
        let (event_tx, event_rx) = mpsc::channel::<LayerEvent>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let worker = std::thread::Builder::new()
            .name("truesight-wayland-overlay".into())
            .spawn(move || run_presenter_thread(main_ctx, cmd_rx, event_tx, ready_tx))
            .map_err(|e| format!("failed to spawn Wayland overlay thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                cmd_tx,
                event_rx,
                worker: Some(worker),
            }),
            Ok(Err(err)) => {
                let _ = worker.join();
                Err(err)
            }
            Err(_) => {
                let _ = worker.join();
                Err("Wayland overlay presenter exited during startup".into())
            }
        }
    }

    /// Sends a command to the presenter thread without blocking the `eframe` UI thread.
    pub(super) fn send(&self, cmd: LayerCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Drains pending pointer, geometry, configure, and output events from the presenter thread.
    pub(super) fn poll_events(&self) -> Vec<LayerEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

struct TargetOutputLayout {
    output: Option<wl_output::WlOutput>,
    origin: (i32, i32),
    size: Option<(i32, i32)>,
    scale: i32,
}

#[derive(Debug, Clone)]
struct PendingShmFrame {
    width_px: u32,
    height_px: u32,
    scale: i32,
    argb8888: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
enum InteractionMode {
    None,
    Dragging {
        grab_local: (f32, f32),
        frame_pending: bool,
    },
    Resizing {
        start_local: (f32, f32),
        start_size: (u32, u32),
    },
}

struct ActiveLayerSurface {
    layer: LayerSurface,
    empty_region: Option<Region>,
    configured: bool,
    scale: i32,
    output_origin: (i32, i32),
    output_size: Option<(i32, i32)>,
    margin_x: i32,
    margin_y: i32,
    width: u32,
    height: u32,
    min_w: u32,
    min_h: u32,
    click_through: bool,
    drag_rect: Option<OverlayRect>,
    resize_rect: Option<OverlayRect>,
    interaction: InteractionMode,
    pending_frame: Option<PendingShmFrame>,
}

struct WaylandLayerState {
    conn: Connection,
    qh: QueueHandle<Self>,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    layer_shell: LayerShell,
    pool: SlotPool,
    main_ctx: egui::Context,
    event_tx: mpsc::Sender<LayerEvent>,
    loop_signal: LoopSignal,
    themed_pointer: Option<ThemedPointer>,
    plain_pointer: Option<wl_pointer::WlPointer>,
    current_cursor: egui::CursorIcon,
    active_surface: Option<ActiveLayerSurface>,
}

impl WaylandLayerState {
    fn emit(&self, ev: LayerEvent) {
        if self.event_tx.send(ev).is_ok() {
            self.main_ctx.request_repaint();
        }
    }

    fn emit_monitors(&self) {
        let monitors: Vec<MonitorRect> = self
            .output_state
            .outputs()
            .filter_map(|o| {
                let info = self.output_state.info(&o)?;
                let (lx, ly) = info.logical_position.unwrap_or((0, 0));
                let (lw, lh) = info.logical_size?;
                if lw <= 0 || lh <= 0 {
                    return None;
                }
                let sf = info.scale_factor.max(1) as f32;
                Some(MonitorRect::new(
                    lx as f32 * sf,
                    ly as f32 * sf,
                    lw as f32 * sf,
                    lh as f32 * sf,
                ))
            })
            .collect();
        if !monitors.is_empty() {
            self.emit(LayerEvent::MonitorsUpdated(monitors));
        }
    }

    fn resolve_target_output(
        &self,
        desktop_x: i32,
        desktop_y: i32,
        width: u32,
        height: u32,
    ) -> TargetOutputLayout {
        let cx = desktop_x + (width as i32 / 2);
        let cy = desktop_y + (height as i32 / 2);
        let mut fallback = None;

        for out in self.output_state.outputs() {
            let Some(info) = self.output_state.info(&out) else {
                continue;
            };
            let origin = info.logical_position.unwrap_or((0, 0));
            let size = info.logical_size.filter(|&(w, h)| w > 0 && h > 0);
            let scale = info.scale_factor.max(1);
            if fallback.is_none() {
                fallback = Some(TargetOutputLayout {
                    output: Some(out.clone()),
                    origin,
                    size,
                    scale,
                });
            }
            if let Some((lw, lh)) = size
                && cx >= origin.0
                && cx < origin.0 + lw
                && cy >= origin.1
                && cy < origin.1 + lh
            {
                return TargetOutputLayout {
                    output: Some(out),
                    origin,
                    size: Some((lw, lh)),
                    scale,
                };
            }
        }

        fallback.unwrap_or(TargetOutputLayout {
            output: None,
            origin: (0, 0),
            size: None,
            scale: 1,
        })
    }

    fn apply_input_region(active: &ActiveLayerSurface) {
        let surface = active.layer.wl_surface();
        if active.click_through {
            if let Some(region) = &active.empty_region {
                surface.set_input_region(Some(region.wl_region()));
            }
        } else {
            surface.set_input_region(None);
        }
    }

    fn commit_pending_frame(&mut self) {
        let Some(active) = self.active_surface.as_mut() else {
            return;
        };
        if !active.configured {
            return;
        }
        let Some(frame) = active.pending_frame.take() else {
            return;
        };
        if frame.width_px == 0 || frame.height_px == 0 {
            return;
        }
        let expected_len = (frame.width_px as usize)
            .saturating_mul(frame.height_px as usize)
            .saturating_mul(4);
        if frame.argb8888.len() < expected_len {
            return;
        }

        let stride = (frame.width_px as i32).saturating_mul(4);
        let Ok((buffer, canvas)) = self.pool.create_buffer(
            frame.width_px as i32,
            frame.height_px as i32,
            stride,
            wl_shm::Format::Argb8888,
        ) else {
            return;
        };
        if canvas.len() >= expected_len {
            canvas[..expected_len].copy_from_slice(&frame.argb8888[..expected_len]);
        }

        let wl_surface = active.layer.wl_surface();
        wl_surface.set_buffer_scale(frame.scale.max(1));
        wl_surface.damage_buffer(0, 0, frame.width_px as i32, frame.height_px as i32);
        if buffer.attach_to(wl_surface).is_ok() {
            active.layer.commit();
        }
    }

    fn handle_command(&mut self, cmd: LayerCommand) {
        match cmd {
            LayerCommand::Show {
                x,
                y,
                width,
                height,
                min_w,
                min_h,
                click_through,
            } => {
                self.active_surface = None;
                let target = self.resolve_target_output(x, y, width, height);
                let init_w = width.max(min_w).min(MAX_OVERLAY_W);
                let init_h = height.max(min_h).min(MAX_OVERLAY_H);
                let raw_margin_x = x.saturating_sub(target.origin.0).max(0);
                let raw_margin_y = y.saturating_sub(target.origin.1).max(0);
                let (margin_x, margin_y) = compute_drag_margin_step(
                    (raw_margin_x, raw_margin_y),
                    (0.0, 0.0),
                    (init_w, init_h),
                    target.size,
                );
                let surface = self.compositor_state.create_surface(&self.qh);
                let layer = self.layer_shell.create_layer_surface(
                    &self.qh,
                    surface,
                    Layer::Overlay,
                    Some("truesight-overlay"),
                    target.output.as_ref(),
                );
                layer.set_anchor(Anchor::TOP | Anchor::LEFT);
                layer.set_exclusive_zone(0);
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
                layer.set_size(init_w, init_h);
                layer.set_margin(margin_y, 0, 0, margin_x);

                let empty_region = Region::new(&self.compositor_state).ok();
                let active = ActiveLayerSurface {
                    layer,
                    empty_region,
                    configured: false,
                    scale: target.scale,
                    output_origin: target.origin,
                    output_size: target.size,
                    margin_x,
                    margin_y,
                    width: init_w,
                    height: init_h,
                    min_w,
                    min_h,
                    click_through,
                    drag_rect: None,
                    resize_rect: None,
                    interaction: InteractionMode::None,
                    pending_frame: None,
                };
                Self::apply_input_region(&active);
                active.layer.commit();
                self.active_surface = Some(active);
            }
            LayerCommand::Hide => {
                self.active_surface = None;
            }
            LayerCommand::SetGeometry {
                x,
                y,
                width,
                height,
                min_w,
                min_h,
            } => {
                if let Some(active) = self.active_surface.as_mut() {
                    active.min_w = min_w;
                    active.min_h = min_h;
                    if matches!(active.interaction, InteractionMode::None) {
                        active.margin_x = x.saturating_sub(active.output_origin.0).max(0);
                        active.margin_y = y.saturating_sub(active.output_origin.1).max(0);
                        active.width = width.max(min_w).min(MAX_OVERLAY_W);
                        active.height = height.max(min_h).min(MAX_OVERLAY_H);
                        active
                            .layer
                            .set_margin(active.margin_y, 0, 0, active.margin_x);
                        active.layer.set_size(active.width, active.height);
                        active.layer.commit();
                    }
                }
            }
            LayerCommand::SetClickThrough(click_through) => {
                if let Some(active) = self.active_surface.as_mut()
                    && active.click_through != click_through
                {
                    active.click_through = click_through;
                    if click_through {
                        active.interaction = InteractionMode::None;
                    }
                    Self::apply_input_region(active);
                    active.layer.commit();
                }
            }
            LayerCommand::SetHitRegions {
                drag_rect,
                resize_rect,
            } => {
                if let Some(active) = self.active_surface.as_mut() {
                    active.drag_rect = drag_rect;
                    active.resize_rect = resize_rect;
                }
            }
            LayerCommand::SetCursor(icon) => {
                self.current_cursor = icon;
                if let Some(pointer) = &self.themed_pointer {
                    let _ = pointer.set_cursor(&self.conn, map_egui_cursor(icon));
                }
            }
            LayerCommand::PresentFrame {
                width_px,
                height_px,
                scale,
                argb8888,
            } => {
                if let Some(active) = self.active_surface.as_mut() {
                    active.pending_frame = Some(PendingShmFrame {
                        width_px,
                        height_px,
                        scale,
                        argb8888,
                    });
                    self.commit_pending_frame();
                }
            }
            LayerCommand::Shutdown => {
                self.active_surface = None;
                self.loop_signal.stop();
            }
        }
        let _ = self.conn.flush();
    }
}

fn run_presenter_thread(
    main_ctx: egui::Context,
    cmd_rx: calloop::channel::Channel<LayerCommand>,
    event_tx: mpsc::Sender<LayerEvent>,
    ready_tx: mpsc::SyncSender<Result<(), String>>,
) {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("Wayland connect failed: {e}")));
            return;
        }
    };
    let (globals, event_queue) = match registry_queue_init(&conn) {
        Ok(pair) => pair,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("Wayland registry init failed: {e}")));
            return;
        }
    };
    let qh = event_queue.handle();

    let compositor_state = match CompositorState::bind(&globals, &qh) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("wl_compositor unavailable: {e}")));
            return;
        }
    };
    let layer_shell = match LayerShell::bind(&globals, &qh) {
        Ok(ls) => ls,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("zwlr_layer_shell_v1 unavailable: {e}")));
            return;
        }
    };
    let shm = match Shm::bind(&globals, &qh) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("wl_shm unavailable: {e}")));
            return;
        }
    };
    let pool = match SlotPool::new(512 * 320 * 4, &shm) {
        Ok(p) => p,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("wl_shm SlotPool init failed: {e}")));
            return;
        }
    };
    let mut event_loop: EventLoop<WaylandLayerState> = match EventLoop::try_new() {
        Ok(el) => el,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("calloop EventLoop init failed: {e}")));
            return;
        }
    };

    let mut state = WaylandLayerState {
        conn: conn.clone(),
        qh: qh.clone(),
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor_state,
        shm,
        layer_shell,
        pool,
        main_ctx,
        event_tx,
        loop_signal: event_loop.get_signal(),
        themed_pointer: None,
        plain_pointer: None,
        current_cursor: egui::CursorIcon::Default,
        active_surface: None,
    };

    let handle = event_loop.handle();
    if WaylandSource::new(conn, event_queue)
        .insert(handle.clone())
        .is_err()
    {
        let _ = ready_tx.send(Err("Failed to register WaylandSource in calloop".into()));
        return;
    }
    if handle
        .insert_source(cmd_rx, |evt, _, state| match evt {
            ChannelEvent::Msg(cmd) => state.handle_command(cmd),
            ChannelEvent::Closed => {
                state.active_surface = None;
                state.loop_signal.stop();
            }
        })
        .is_err()
    {
        let _ = ready_tx.send(Err(
            "Failed to register LayerCommand channel in calloop".into()
        ));
        return;
    }

    let _ = ready_tx.send(Ok(()));
    let _ = event_loop.run(None, &mut state, |_| {});
}

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
        _output: wl_output::WlOutput,
    ) {
        self.emit_monitors();
    }
}

impl LayerShellHandler for WaylandLayerState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        if self
            .active_surface
            .as_ref()
            .is_some_and(|a| &a.layer == layer)
        {
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
            self.commit_pending_frame();
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
