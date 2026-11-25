mod handlers;
mod output;
mod pointer;

use super::overlay_backend::{
    CrossMonitorTarget, LayerCommand, LayerEvent, OverlayRect, compute_drag_margin_step,
};
use eframe::egui;
use smithay_client_toolkit::compositor::{CompositorState, Region};
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::reexports::calloop::channel::Event as ChannelEvent;
use smithay_client_toolkit::reexports::calloop::{self, EventLoop, LoopSignal};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{
    wl_output, wl_pointer, wl_registry, wl_shm, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, QueueHandle};
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::seat::pointer::ThemedPointer;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface,
};
use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::shm::slot::SlotPool;
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

struct ActiveLayerSurface {
    layer: LayerSurface,
    empty_region: Option<Region>,
    configured: bool,
    output_id: Option<u32>,
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

pub(super) struct DragAnchor {
    pub(super) layer: LayerSurface,
    pub(super) desktop_origin: (i32, i32),
    pub(super) grab_local: (f32, f32),
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
    last_frame: Option<PendingShmFrame>,
    drag_anchor: Option<DragAnchor>,
    active_surface: Option<ActiveLayerSurface>,
}

impl WaylandLayerState {
    fn emit(&self, ev: LayerEvent) {
        if self.event_tx.send(ev).is_ok() {
            self.main_ctx.request_repaint();
        }
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
        let expected_w = active.width.saturating_mul(active.scale.max(1) as u32);
        let expected_h = active.height.saturating_mul(active.scale.max(1) as u32);
        if frame.width_px == 0
            || frame.height_px == 0
            || frame.width_px != expected_w
            || frame.height_px != expected_h
        {
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
            self.last_frame = Some(frame);
        }
    }

    fn attach_transparent_buffer(
        &mut self,
        wl_surface: &wl_surface::WlSurface,
        width: u32,
        height: u32,
        scale: i32,
    ) {
        let s = scale.max(1) as u32;
        let w_px = width.max(1).saturating_mul(s);
        let h_px = height.max(1).saturating_mul(s);
        let stride = (w_px as i32).saturating_mul(4);
        let Ok((buffer, canvas)) =
            self.pool
                .create_buffer(w_px as i32, h_px as i32, stride, wl_shm::Format::Argb8888)
        else {
            return;
        };
        canvas.fill(0);
        wl_surface.set_buffer_scale(scale.max(1));
        wl_surface.damage_buffer(0, 0, w_px as i32, h_px as i32);
        if buffer.attach_to(wl_surface).is_ok() {
            wl_surface.commit();
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn create_active_surface_on_output(
        &self,
        wl_output: Option<&wl_output::WlOutput>,
        output_id: Option<u32>,
        origin: (i32, i32),
        size: Option<(i32, i32)>,
        scale: i32,
        margin: (i32, i32),
        dims: (u32, u32, u32, u32),
        click_through: bool,
        hit_regions: (Option<OverlayRect>, Option<OverlayRect>),
        seed_last_frame: bool,
    ) -> ActiveLayerSurface {
        let (width, height, min_w, min_h) = dims;
        let surface = self.compositor_state.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Overlay,
            Some("truesight-overlay"),
            wl_output,
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(width, height);
        layer.set_margin(margin.1, 0, 0, margin.0);

        let empty_region = Region::new(&self.compositor_state).ok();
        let pending_frame = if seed_last_frame {
            self.last_frame.clone().filter(|f| {
                f.width_px == width.saturating_mul(scale.max(1) as u32)
                    && f.height_px == height.saturating_mul(scale.max(1) as u32)
                    && f.scale == scale.max(1)
            })
        } else {
            None
        };
        let active = ActiveLayerSurface {
            layer,
            empty_region,
            configured: false,
            output_id,
            scale: scale.max(1),
            output_origin: origin,
            output_size: size,
            margin_x: margin.0,
            margin_y: margin.1,
            width,
            height,
            min_w,
            min_h,
            click_through,
            drag_rect: hit_regions.0,
            resize_rect: hit_regions.1,
            interaction: InteractionMode::None,
            pending_frame,
        };
        Self::apply_input_region(&active);
        active.layer.commit();
        active
    }

    pub(super) fn migrate_to_output(
        &mut self,
        target: CrossMonitorTarget,
        resize: Option<(u32, u32, u32, u32)>,
        persist: bool,
    ) {
        let Some((wl_output, scale)) = self.output_by_id(target.output_id) else {
            return;
        };
        let Some(prev) = self.active_surface.take() else {
            return;
        };
        let dims = resize.unwrap_or((prev.width, prev.height, prev.min_w, prev.min_h));
        let active = self.create_active_surface_on_output(
            Some(&wl_output),
            Some(target.output_id),
            target.origin,
            Some(target.size),
            scale,
            target.margin,
            dims,
            prev.click_through,
            (prev.drag_rect, prev.resize_rect),
            true,
        );
        self.active_surface = Some(active);

        self.emit(LayerEvent::GeometryChanged {
            x: target.origin.0 + target.margin.0,
            y: target.origin.1 + target.margin.1,
            width: dims.0,
            height: dims.1,
            persist,
        });
        self.emit(LayerEvent::Configured {
            width: dims.0,
            height: dims.1,
            scale,
        });
    }

    pub(super) fn begin_anchored_cross_monitor_drag(
        &mut self,
        target: CrossMonitorTarget,
        grab_local: (f32, f32),
    ) {
        if self.drag_anchor.is_some() {
            self.migrate_to_output(target, None, false);
            return;
        }
        let Some((wl_output, scale)) = self.output_by_id(target.output_id) else {
            return;
        };
        let Some(prev) = self.active_surface.take() else {
            return;
        };
        let anchor_origin = (
            prev.output_origin.0 + prev.margin_x,
            prev.output_origin.1 + prev.margin_y,
        );
        self.attach_transparent_buffer(
            prev.layer.wl_surface(),
            prev.width,
            prev.height,
            prev.scale,
        );
        let dims = (prev.width, prev.height, prev.min_w, prev.min_h);
        let hit_regions = (prev.drag_rect, prev.resize_rect);
        let click_through = prev.click_through;
        self.drag_anchor = Some(DragAnchor {
            layer: prev.layer,
            desktop_origin: anchor_origin,
            grab_local,
        });
        let active = self.create_active_surface_on_output(
            Some(&wl_output),
            Some(target.output_id),
            target.origin,
            Some(target.size),
            scale,
            target.margin,
            dims,
            click_through,
            hit_regions,
            true,
        );
        self.active_surface = Some(active);
        if let Some(pointer) = &self.themed_pointer {
            let _ = pointer.set_cursor(
                &self.conn,
                pointer::map_egui_cursor(egui::CursorIcon::Grabbing),
            );
        }

        self.emit(LayerEvent::GeometryChanged {
            x: target.origin.0 + target.margin.0,
            y: target.origin.1 + target.margin.1,
            width: dims.0,
            height: dims.1,
            persist: false,
        });
        self.emit(LayerEvent::Configured {
            width: dims.0,
            height: dims.1,
            scale,
        });
    }

    pub(super) fn finish_anchored_drag(&mut self) {
        if self.drag_anchor.take().is_none() {
            return;
        }
        if let Some(pointer) = &self.themed_pointer {
            let _ = pointer.set_cursor(&self.conn, pointer::map_egui_cursor(self.current_cursor));
        }
        if let Some(active) = &self.active_surface {
            self.emit(LayerEvent::GeometryChanged {
                x: active.output_origin.0 + active.margin_x,
                y: active.output_origin.1 + active.margin_y,
                width: active.width,
                height: active.height,
                persist: true,
            });
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
                self.drag_anchor = None;
                self.active_surface = None;
                let target = self.resolve_target_output(x, y, width, height);
                let init_w = width.max(min_w).min(MAX_OVERLAY_W);
                let init_h = height.max(min_h).min(MAX_OVERLAY_H);
                let raw_margin_x = x.saturating_sub(target.origin.0).max(0);
                let raw_margin_y = y.saturating_sub(target.origin.1).max(0);
                let margin = compute_drag_margin_step(
                    (raw_margin_x, raw_margin_y),
                    (0.0, 0.0),
                    (init_w, init_h),
                    target.size,
                );
                let active = self.create_active_surface_on_output(
                    target.output.as_ref(),
                    target.id,
                    target.origin,
                    target.size,
                    target.scale,
                    margin,
                    (init_w, init_h, min_w, min_h),
                    click_through,
                    (None, None),
                    false,
                );
                self.active_surface = Some(active);
            }
            LayerCommand::Hide => {
                self.drag_anchor = None;
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
                let w = width.max(min_w).min(MAX_OVERLAY_W);
                let h = height.max(min_h).min(MAX_OVERLAY_H);
                let target = self.resolve_target_output(x, y, w, h);
                let current_output_id = self.active_surface.as_ref().and_then(|a| a.output_id);
                let interacting = self
                    .active_surface
                    .as_ref()
                    .is_some_and(|a| !matches!(a.interaction, InteractionMode::None));
                if !interacting
                    && let Some(target_id) = target.id
                    && Some(target_id) != current_output_id
                {
                    let raw_mx = x.saturating_sub(target.origin.0).max(0);
                    let raw_my = y.saturating_sub(target.origin.1).max(0);
                    let margin =
                        compute_drag_margin_step((raw_mx, raw_my), (0.0, 0.0), (w, h), target.size);
                    self.migrate_to_output(
                        CrossMonitorTarget {
                            output_id: target_id,
                            origin: target.origin,
                            size: target.size.unwrap_or((w as i32, h as i32)),
                            margin,
                        },
                        Some((w, h, min_w, min_h)),
                        false,
                    );
                } else if let Some(active) = self.active_surface.as_mut() {
                    active.min_w = min_w;
                    active.min_h = min_h;
                    if matches!(active.interaction, InteractionMode::None) {
                        let raw_mx = x.saturating_sub(active.output_origin.0).max(0);
                        let raw_my = y.saturating_sub(active.output_origin.1).max(0);
                        let (mx, my) = compute_drag_margin_step(
                            (raw_mx, raw_my),
                            (0.0, 0.0),
                            (w, h),
                            active.output_size,
                        );
                        active.margin_x = mx;
                        active.margin_y = my;
                        active.width = w;
                        active.height = h;
                        active
                            .layer
                            .set_margin(active.margin_y, 0, 0, active.margin_x);
                        active.layer.set_size(active.width, active.height);
                        active.layer.commit();
                    }
                }
            }
            LayerCommand::SetClickThrough(click_through) => {
                if click_through {
                    self.drag_anchor = None;
                }
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
                    let _ = pointer.set_cursor(&self.conn, pointer::map_egui_cursor(icon));
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
                self.drag_anchor = None;
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
        last_frame: None,
        drag_anchor: None,
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
