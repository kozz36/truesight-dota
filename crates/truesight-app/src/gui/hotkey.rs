use super::TrueSightGuiApp;
use super::theme::*;
use crate::session::{PassHotkey, PassHotkeyKey};
use eframe::egui::{self, RichText, Stroke};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};

#[cfg(test)]
pub(super) use super::hotkey_portal::{
    PORTAL_PREFERRED_TRIGGER, PortalLoopOutcome, portal_status_label, should_bind_on_startup,
    should_fallback_to_xwayland, should_use_configure_shortcuts,
};
#[cfg(all(test, target_os = "linux"))]
pub(super) use super::hotkey_portal::{
    extract_assigned_trigger_from_pairs, inspect_portal_shortcuts_value,
};

/// Human-readable default label for the global click-through hotkey (`Ctrl+Shift+T`).
pub(super) const PASS_HOTKEY_LABEL: &str = "Ctrl+Shift+T";

/// Actions dispatched from OS-level global hotkeys into the GUI frame loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GlobalHotkeyAction {
    ToggleOverlayPass,
}

macro_rules! map_pass_keys {
    ($($pk:ident => ($ek:ident, $gc:ident)),* $(,)?) => {
        fn egui_key_to_pass_key(key: egui::Key) -> Option<PassHotkeyKey> {
            match key {
                $(egui::Key::$ek => Some(PassHotkeyKey::$pk),)*
                _ => None,
            }
        }

        fn to_global_code(key: PassHotkeyKey) -> global_hotkey::hotkey::Code {
            use global_hotkey::hotkey::Code;
            match key {
                $(PassHotkeyKey::$pk => Code::$gc,)*
            }
        }
    };
}

map_pass_keys!(
    A => (A, KeyA), B => (B, KeyB), C => (C, KeyC), D => (D, KeyD), E => (E, KeyE),
    F => (F, KeyF), G => (G, KeyG), H => (H, KeyH), I => (I, KeyI), J => (J, KeyJ),
    K => (K, KeyK), L => (L, KeyL), M => (M, KeyM), N => (N, KeyN), O => (O, KeyO),
    P => (P, KeyP), Q => (Q, KeyQ), R => (R, KeyR), S => (S, KeyS), T => (T, KeyT),
    U => (U, KeyU), V => (V, KeyV), W => (W, KeyW), X => (X, KeyX), Y => (Y, KeyY),
    Z => (Z, KeyZ),
    Digit0 => (Num0, Digit0), Digit1 => (Num1, Digit1), Digit2 => (Num2, Digit2),
    Digit3 => (Num3, Digit3), Digit4 => (Num4, Digit4), Digit5 => (Num5, Digit5),
    Digit6 => (Num6, Digit6), Digit7 => (Num7, Digit7), Digit8 => (Num8, Digit8),
    Digit9 => (Num9, Digit9),
    F1 => (F1, F1), F2 => (F2, F2), F3 => (F3, F3), F4 => (F4, F4),
    F5 => (F5, F5), F6 => (F6, F6), F7 => (F7, F7), F8 => (F8, F8),
    F9 => (F9, F9), F10 => (F10, F10), F11 => (F11, F11), F12 => (F12, F12),
);

fn to_global_hotkey(hotkey: PassHotkey) -> global_hotkey::hotkey::HotKey {
    use global_hotkey::hotkey::{HotKey, Modifiers};
    let mut mods = Modifiers::empty();
    if hotkey.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if hotkey.alt {
        mods |= Modifiers::ALT;
    }
    if hotkey.shift {
        mods |= Modifiers::SHIFT;
    }
    HotKey::new(
        (!mods.is_empty()).then_some(mods),
        to_global_code(hotkey.key),
    )
}

/// Converts a pressed `egui::Key` + modifiers into a [`PassHotkey`] (requiring at least one modifier).
pub(super) fn from_egui_key(key: egui::Key, modifiers: egui::Modifiers) -> Option<PassHotkey> {
    let ctrl = modifiers.ctrl;
    let alt = modifiers.alt;
    let shift = modifiers.shift;
    if !(ctrl || alt || shift) {
        return None;
    }
    let pass_key = egui_key_to_pass_key(key)?;
    Some(PassHotkey {
        ctrl,
        alt,
        shift,
        key: pass_key,
    })
}

/// Returns `true` when the active pass-through hotkey was just pressed (non-repeating) in `ctx`.
pub(super) fn is_pass_shortcut_pressed(ctx: &egui::Context, target: PassHotkey) -> bool {
    ctx.input(|i| {
        i.events.iter().any(|ev| {
            matches!(
                ev,
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if egui_key_to_pass_key(*key) == Some(target.key)
                    && modifiers.ctrl == target.ctrl
                    && modifiers.alt == target.alt
                    && modifiers.shift == target.shift
            )
        })
    })
}

fn capture_pass_hotkey_from_ctx(ctx: &egui::Context) -> Option<Result<PassHotkey, bool>> {
    ctx.input(|i| {
        for ev in &i.events {
            if let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = ev
            {
                if *key == egui::Key::Escape {
                    return Some(Err(true));
                }
                if let Some(hk) = from_egui_key(*key, *modifiers) {
                    return Some(Ok(hk));
                }
            }
        }
        None
    })
}

/// Cross-platform global hotkey listener supporting custom shortcuts (`Ctrl+Shift+T` default)
/// on Windows, X11, and macOS, plus XDG Desktop Portal `GlobalShortcuts` re-binding on Linux Wayland.
pub(super) struct GlobalHotkeyListener {
    rx: mpsc::Receiver<GlobalHotkeyAction>,
    tx: mpsc::SyncSender<GlobalHotkeyAction>,
    status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    manager: Option<global_hotkey::GlobalHotKeyManager>,
    registered_hotkey: Option<global_hotkey::hotkey::HotKey>,
    expected_id: Arc<AtomicU32>,
    platform_name: &'static str,
    is_wayland_portal: bool,
    active_hotkey: Arc<Mutex<PassHotkey>>,
    rebind_in_flight: Arc<AtomicBool>,
    #[cfg(target_os = "linux")]
    portal_session: super::hotkey_portal::PortalSessionSlot,
    #[cfg(target_os = "linux")]
    portal_bound_in_session: Arc<AtomicBool>,
}

impl Drop for GlobalHotkeyListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(mgr) = &self.manager
            && let Some(prev) = self.registered_hotkey.take()
        {
            let _ = mgr.unregister(prev);
        }
        #[cfg(target_os = "linux")]
        if let Ok(mut guard) = self.portal_session.lock()
            && let Some((conn, session_path)) = guard.take()
        {
            super::hotkey_portal::close_portal_session(&conn, &session_path);
        }
    }
}

impl GlobalHotkeyListener {
    #[cfg(test)]
    fn make_test_listener(
        rx: mpsc::Receiver<GlobalHotkeyAction>,
        status: &str,
        is_wayland_portal: bool,
    ) -> Self {
        let (tx, _unused_rx) = mpsc::sync_channel(1);
        Self {
            rx,
            tx,
            status: Arc::new(Mutex::new(status.to_string())),
            stop: Arc::new(AtomicBool::new(false)),
            manager: None,
            registered_hotkey: None,
            expected_id: Arc::new(AtomicU32::new(0)),
            platform_name: if is_wayland_portal {
                "Wayland Portal"
            } else {
                "Test"
            },
            is_wayland_portal,
            active_hotkey: Arc::new(Mutex::new(PassHotkey::default())),
            rebind_in_flight: Arc::new(AtomicBool::new(false)),
            #[cfg(target_os = "linux")]
            portal_session: Arc::new(Mutex::new(None)),
            #[cfg(target_os = "linux")]
            portal_bound_in_session: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates an inactive listener backed by a caller-supplied channel for deterministic unit tests.
    #[cfg(test)]
    pub(super) fn from_channel_for_test(
        rx: mpsc::Receiver<GlobalHotkeyAction>,
        status: &str,
    ) -> Self {
        Self::make_test_listener(rx, status, false)
    }

    /// Creates a test listener with configurable Wayland Portal backend flag for Options UI tests.
    #[cfg(test)]
    pub(super) fn from_channel_for_test_with_wayland(
        rx: mpsc::Receiver<GlobalHotkeyAction>,
        status: &str,
        is_wayland_portal: bool,
    ) -> Self {
        Self::make_test_listener(rx, status, is_wayland_portal)
    }

    /// Starts OS-level global shortcut registration for `hotkey` and wakes `ctx` on trigger.
    pub(super) fn start(ctx: egui::Context, hotkey: PassHotkey) -> Self {
        let (tx, rx) = mpsc::sync_channel::<GlobalHotkeyAction>(8);
        let status = Arc::new(Mutex::new(format!("{} (In-App)", hotkey.format_label())));
        let stop = Arc::new(AtomicBool::new(false));
        let expected_id = Arc::new(AtomicU32::new(0));
        let active_hotkey = Arc::new(Mutex::new(hotkey));
        let rebind_in_flight = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "linux")]
        let portal_session = Arc::new(Mutex::new(None));
        #[cfg(target_os = "linux")]
        let portal_bound_in_session = Arc::new(AtomicBool::new(false));

        #[cfg(target_os = "windows")]
        let (platform_name, is_wayland_portal, manager, registered_hotkey) = {
            let (m, hk) = init_desktop_manager(
                tx.clone(),
                ctx,
                &status,
                "Windows",
                hotkey,
                Arc::clone(&expected_id),
            );
            ("Windows", false, m, hk)
        };

        #[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
        let (platform_name, is_wayland_portal, manager, registered_hotkey) = {
            let (m, hk) = init_desktop_manager(
                tx.clone(),
                ctx,
                &status,
                "Desktop",
                hotkey,
                Arc::clone(&expected_id),
            );
            ("Desktop", false, m, hk)
        };

        #[cfg(target_os = "linux")]
        let (platform_name, is_wayland_portal, manager, registered_hotkey) = {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                super::hotkey_portal::spawn_wayland_portal_listener(
                    tx.clone(),
                    ctx,
                    Arc::clone(&status),
                    Arc::clone(&stop),
                    Arc::clone(&portal_session),
                    Arc::clone(&active_hotkey),
                    false,
                    Arc::clone(&rebind_in_flight),
                    Arc::clone(&portal_bound_in_session),
                );
                ("Wayland Portal", true, None, None)
            } else if std::env::var_os("DISPLAY").is_some() {
                let (m, hk) = init_desktop_manager(
                    tx.clone(),
                    ctx,
                    &status,
                    "X11",
                    hotkey,
                    Arc::clone(&expected_id),
                );
                ("X11", false, m, hk)
            } else {
                ("Linux", false, None, None)
            }
        };

        Self {
            rx,
            tx,
            status,
            stop,
            manager,
            registered_hotkey,
            expected_id,
            platform_name,
            is_wayland_portal,
            active_hotkey,
            rebind_in_flight,
            #[cfg(target_os = "linux")]
            portal_session,
            #[cfg(target_os = "linux")]
            portal_bound_in_session,
        }
    }

    /// Re-registers the pass-through global shortcut dynamically on Windows/X11/Desktop,
    /// or updates the active in-app/preferred trigger label on Wayland.
    pub(super) fn update_hotkey(&mut self, _ctx: &egui::Context, hotkey: PassHotkey) -> bool {
        if let Ok(mut guard) = self.active_hotkey.lock() {
            *guard = hotkey;
        }
        let label = hotkey.format_label();
        if self.is_wayland_portal {
            let cur = self.status_label();
            if cur.ends_with("(In-App)") {
                set_status(&self.status, format!("{label} (In-App)"));
            }
            return true;
        }

        let Some(mgr) = &self.manager else {
            set_status(&self.status, format!("{label} (In-App)"));
            return false;
        };

        let prev = self.registered_hotkey.take();
        if let Some(old_hk) = prev {
            let _ = mgr.unregister(old_hk);
        }
        let new_hk = to_global_hotkey(hotkey);
        if mgr.register(new_hk).is_ok() {
            self.expected_id.store(new_hk.id(), Ordering::Release);
            self.registered_hotkey = Some(new_hk);
            set_status(
                &self.status,
                format!("{label} (Global · {})", self.platform_name),
            );
            true
        } else {
            self.expected_id.store(0, Ordering::Release);
            set_status(&self.status, format!("{label} (In-App)"));
            false
        }
    }

    pub(super) fn is_wayland_portal_backend(&self) -> bool {
        self.is_wayland_portal
    }

    pub(super) fn rebind_in_flight(&self) -> bool {
        self.rebind_in_flight.load(Ordering::Acquire)
    }

    /// Opens the compositor's XDG `BindShortcuts` prompt on demand without restarting the app.
    pub(super) fn rebind_wayland_portal(
        &mut self,
        ctx: egui::Context,
        preferred: PassHotkey,
    ) -> bool {
        if !self.is_wayland_portal {
            return false;
        }
        if let Ok(mut guard) = self.active_hotkey.lock() {
            *guard = preferred;
        }
        if self
            .rebind_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }

        #[cfg(target_os = "linux")]
        {
            let existing = self.portal_session.lock().ok().and_then(|g| g.clone());
            if let Some((conn, session_path)) = existing {
                let tx = self.tx.clone();
                let stop = Arc::clone(&self.stop);
                let portal_session = Arc::clone(&self.portal_session);
                let active_hotkey = Arc::clone(&self.active_hotkey);
                let status = Arc::clone(&self.status);
                let in_flight = Arc::clone(&self.rebind_in_flight);
                let bound_in_session = Arc::clone(&self.portal_bound_in_session);
                let _ = std::thread::Builder::new()
                    .name("truesight-portal-rebind".into())
                    .spawn(move || {
                        let res = super::hotkey_portal::rebind_existing_portal_session(
                            &conn,
                            &session_path,
                            preferred,
                            &status,
                            &bound_in_session,
                        );
                        if matches!(
                            res,
                            Ok(super::hotkey_portal::RebindSessionResult::NeedsFreshSession)
                        ) {
                            super::hotkey_portal::close_portal_session(&conn, &session_path);
                            if let Ok(mut guard) = portal_session.lock() {
                                *guard = None;
                            }
                            bound_in_session.store(false, Ordering::Release);
                            super::hotkey_portal::spawn_wayland_portal_listener(
                                tx,
                                ctx,
                                status,
                                stop,
                                portal_session,
                                active_hotkey,
                                true,
                                in_flight,
                                bound_in_session,
                            );
                        } else {
                            in_flight.store(false, Ordering::Release);
                            ctx.request_repaint();
                        }
                    });
            } else {
                super::hotkey_portal::spawn_wayland_portal_listener(
                    self.tx.clone(),
                    ctx,
                    Arc::clone(&self.status),
                    Arc::clone(&self.stop),
                    Arc::clone(&self.portal_session),
                    Arc::clone(&self.active_hotkey),
                    true,
                    Arc::clone(&self.rebind_in_flight),
                    Arc::clone(&self.portal_bound_in_session),
                );
            }
            true
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = &self.tx;
            self.rebind_in_flight.store(false, Ordering::Release);
            ctx.request_repaint();
            false
        }
    }

    /// Drains all pending global hotkey actions without blocking the UI thread.
    pub(super) fn poll_actions(&self) -> Vec<GlobalHotkeyAction> {
        let _ = &self.tx;
        let mut out = Vec::new();
        while let Ok(action) = self.rx.try_recv() {
            out.push(action);
        }
        out
    }

    /// Returns a concise human-readable description of the active global hotkey backend.
    pub(super) fn status_label(&self) -> String {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| format!("{PASS_HOTKEY_LABEL} (In-App)"))
    }
}

pub(super) fn set_status(status: &Arc<Mutex<String>>, text: String) {
    if let Ok(mut guard) = status.lock() {
        *guard = text;
    }
}

pub(super) fn init_desktop_manager(
    tx: mpsc::SyncSender<GlobalHotkeyAction>,
    ctx: egui::Context,
    status: &Arc<Mutex<String>>,
    platform_name: &str,
    hotkey: PassHotkey,
    expected_id: Arc<AtomicU32>,
) -> (
    Option<global_hotkey::GlobalHotKeyManager>,
    Option<global_hotkey::hotkey::HotKey>,
) {
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    let label = hotkey.format_label();
    let Ok(manager) = GlobalHotKeyManager::new() else {
        set_status(status, format!("{label} (In-App)"));
        return (None, None);
    };

    let handler_expected = Arc::clone(&expected_id);
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        let cur_id = handler_expected.load(Ordering::Acquire);
        if cur_id != 0 && event.id == cur_id && event.state == HotKeyState::Pressed {
            let _ = tx.try_send(GlobalHotkeyAction::ToggleOverlayPass);
            ctx.request_repaint();
        }
    }));

    let hk = to_global_hotkey(hotkey);
    if manager.register(hk).is_err() {
        set_status(status, format!("{label} (In-App)"));
        return (Some(manager), None);
    }

    expected_id.store(hk.id(), Ordering::Release);
    set_status(status, format!("{label} (Global · {platform_name})"));
    (Some(manager), Some(hk))
}

impl TrueSightGuiApp {
    pub(super) fn handle_pass_hotkey_frame(&mut self, ctx: &egui::Context) -> (bool, bool) {
        let mut pass_toggled_this_frame = false;
        let mut captured_this_frame = false;
        if !self.options_open {
            self.hotkey_recording = false;
        }
        if self.hotkey_recording {
            match capture_pass_hotkey_from_ctx(ctx) {
                Some(Ok(captured)) => {
                    self.session.set_pass_hotkey(captured);
                    if let Some(listener) = &mut self.hotkey_listener {
                        let _ = listener.update_hotkey(ctx, captured);
                    }
                    self.hotkey_recording = false;
                    captured_this_frame = true;
                    self.status_banner =
                        format!("Pass-through hotkey: {}", captured.format_label());
                }
                Some(Err(_escaped)) => {
                    self.hotkey_recording = false;
                    captured_this_frame = true;
                }
                None => {}
            }
        }
        // Always drain queued global actions so a shortcut pressed during capture cannot fire PASS on this or the next frame.
        if let Some(actions) = self
            .hotkey_listener
            .as_ref()
            .map(GlobalHotkeyListener::poll_actions)
            && !captured_this_frame
            && !self.hotkey_recording
        {
            for action in actions {
                if action == GlobalHotkeyAction::ToggleOverlayPass && self.session.overlay_mode {
                    let next = !self.session.overlay_geometry.click_through;
                    self.set_overlay_click_through_live(ctx, next);
                    pass_toggled_this_frame = true;
                }
            }
        }
        if captured_this_frame {
            return (true, true);
        }
        let f8_root =
            !self.hotkey_recording && Self::is_non_repeating_key_pressed(ctx, egui::Key::F8);
        if f8_root {
            let next = !self.session.overlay_mode;
            self.session.set_overlay_mode(next);
        }
        if !pass_toggled_this_frame
            && !self.hotkey_recording
            && self.session.overlay_mode
            && is_pass_shortcut_pressed(ctx, self.session.pass_hotkey)
        {
            let next = !self.session.overlay_geometry.click_through;
            self.set_overlay_click_through_live(ctx, next);
            pass_toggled_this_frame = true;
        }
        (f8_root, pass_toggled_this_frame)
    }

    pub(super) fn render_options_hotkey_card(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        tr: &super::i18n::UiStrings,
    ) {
        let hk_label = self.session.pass_hotkey.format_label();
        let hotkey_status = self
            .hotkey_listener
            .as_ref()
            .map(GlobalHotkeyListener::status_label)
            .unwrap_or_else(|| format!("{hk_label} (In-App)"));
        let is_wayland = self
            .hotkey_listener
            .as_ref()
            .is_some_and(GlobalHotkeyListener::is_wayland_portal_backend);
        let is_default = self.session.pass_hotkey == PassHotkey::default();

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                RichText::new(tr.pass_hotkey_label)
                    .size(9.5)
                    .strong()
                    .color(P_MUTED),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(&hotkey_status)
                        .size(9.5)
                        .strong()
                        .color(P_CYAN),
                )
                .truncate(),
            )
            .on_hover_text(&hotkey_status);

            let rec_lbl = if self.hotkey_recording {
                tr.hotkey_recording_btn
            } else {
                tr.hotkey_change_btn
            };
            let rec_col = if self.hotkey_recording {
                P_VOID
            } else {
                P_GLINT
            };
            let rec_fill = if self.hotkey_recording {
                P_GEM
            } else {
                P_INDIGO_2
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(rec_lbl).size(9.0).strong().color(rec_col))
                        .fill(rec_fill)
                        .stroke(Stroke::new(
                            1.0_f32,
                            if self.hotkey_recording { P_GEM } else { P_LINE },
                        )),
                )
                .on_hover_text(tr.hotkey_record_hover)
                .clicked()
            {
                self.hotkey_recording = !self.hotkey_recording;
            }

            if !is_default
                && ui
                    .add(
                        egui::Button::new(
                            RichText::new(tr.hotkey_reset_btn).size(9.0).color(P_SILVER),
                        )
                        .fill(P_INDIGO_2),
                    )
                    .clicked()
            {
                let def_hk = PassHotkey::default();
                self.session.set_pass_hotkey(def_hk);
                self.hotkey_recording = false;
                if let Some(listener) = &mut self.hotkey_listener {
                    let _ = listener.update_hotkey(ctx, def_hk);
                }
            }
        });

        if is_wayland {
            let in_flight = self
                .hotkey_listener
                .as_ref()
                .is_some_and(GlobalHotkeyListener::rebind_in_flight);
            let cfg_lbl = if in_flight {
                tr.configure_shortcut_busy
            } else {
                tr.configure_shortcut_btn
            };
            if ui
                .add_enabled(
                    !in_flight,
                    egui::Button::new(RichText::new(cfg_lbl).size(9.5).strong().color(P_GEM))
                        .fill(P_INDIGO_2)
                        .stroke(Stroke::new(1.0_f32, P_GEM_DEEP)),
                )
                .on_hover_text(tr.configure_shortcut_hover)
                .clicked()
                && let Some(listener) = &mut self.hotkey_listener
            {
                let _ = listener.rebind_wayland_portal(ctx.clone(), self.session.pass_hotkey);
            }
        }
    }
}
