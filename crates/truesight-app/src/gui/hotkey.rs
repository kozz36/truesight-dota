use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

/// Human-readable label for the global click-through hotkey (`Ctrl+Shift+P`, leaving `F9` free for Dota 2 Pause).
pub(super) const PASS_HOTKEY_LABEL: &str = "Ctrl+Shift+P";

/// Actions dispatched from OS-level global hotkeys into the GUI frame loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GlobalHotkeyAction {
    ToggleOverlayPass,
}

/// Cross-platform global hotkey listener (`Ctrl+Shift+P`) backed by Win32 `RegisterHotKey` on
/// Windows, XDG Desktop Portal `GlobalShortcuts` on Linux Wayland, and X11 `XGrabKey` fallback on
/// Linux X11. Fails closed to in-app shortcut handling without affecting normal draft rendering.
pub(super) struct GlobalHotkeyListener {
    rx: mpsc::Receiver<GlobalHotkeyAction>,
    status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    _manager: Option<global_hotkey::GlobalHotKeyManager>,
    #[cfg(target_os = "linux")]
    portal_session:
        Arc<Mutex<Option<(zbus::blocking::Connection, zbus::zvariant::OwnedObjectPath)>>>,
}

impl Drop for GlobalHotkeyListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        #[cfg(target_os = "linux")]
        if let Ok(mut guard) = self.portal_session.lock()
            && let Some((conn, session_path)) = guard.take()
        {
            if let Ok(proxy) = zbus::blocking::Proxy::new(
                &conn,
                "org.freedesktop.portal.Desktop",
                session_path,
                "org.freedesktop.portal.Session",
            ) {
                let _ = proxy.call::<_, _, ()>("Close", &());
            }
        }
    }
}

impl GlobalHotkeyListener {
    /// Creates an inactive listener backed by a caller-supplied channel for deterministic unit tests.
    #[cfg(test)]
    pub(super) fn from_channel_for_test(
        rx: mpsc::Receiver<GlobalHotkeyAction>,
        status: &str,
    ) -> Self {
        Self {
            rx,
            status: Arc::new(Mutex::new(status.to_string())),
            stop: Arc::new(AtomicBool::new(false)),
            _manager: None,
            #[cfg(target_os = "linux")]
            portal_session: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts OS-level `Ctrl+Shift+P` global shortcut registration and wakes `ctx` on trigger.
    pub(super) fn start(ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::sync_channel::<GlobalHotkeyAction>(8);
        let status = Arc::new(Mutex::new(format!("{PASS_HOTKEY_LABEL} (In-App)")));
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "linux")]
        let portal_session = Arc::new(Mutex::new(None));

        #[cfg(target_os = "windows")]
        let manager = init_desktop_manager(tx.clone(), ctx.clone(), &status, "Windows");

        #[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
        let manager = init_desktop_manager(tx.clone(), ctx.clone(), &status, "Desktop");

        #[cfg(target_os = "linux")]
        let manager = {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                spawn_wayland_portal_listener(
                    tx,
                    ctx,
                    Arc::clone(&status),
                    Arc::clone(&stop),
                    Arc::clone(&portal_session),
                );
                None
            } else if std::env::var_os("DISPLAY").is_some() {
                init_desktop_manager(tx, ctx, &status, "X11")
            } else {
                None
            }
        };

        Self {
            rx,
            status,
            stop,
            _manager: manager,
            #[cfg(target_os = "linux")]
            portal_session,
        }
    }

    /// Drains all pending global hotkey actions without blocking the UI thread.
    pub(super) fn poll_actions(&self) -> Vec<GlobalHotkeyAction> {
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

fn set_status(status: &Arc<Mutex<String>>, text: String) {
    if let Ok(mut guard) = status.lock() {
        *guard = text;
    }
}

fn init_desktop_manager(
    tx: mpsc::SyncSender<GlobalHotkeyAction>,
    ctx: egui::Context,
    status: &Arc<Mutex<String>>,
    platform_name: &str,
) -> Option<global_hotkey::GlobalHotKeyManager> {
    use global_hotkey::{
        GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
        hotkey::{Code, HotKey, Modifiers},
    };

    let Ok(manager) = GlobalHotKeyManager::new() else {
        set_status(status, format!("{PASS_HOTKEY_LABEL} (In-App)"));
        return None;
    };
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyP);
    if manager.register(hotkey).is_err() {
        set_status(status, format!("{PASS_HOTKEY_LABEL} (In-App)"));
        return None;
    }

    let expected_id = hotkey.id();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == expected_id && event.state == HotKeyState::Pressed {
            let _ = tx.try_send(GlobalHotkeyAction::ToggleOverlayPass);
            ctx.request_repaint();
        }
    }));

    set_status(
        status,
        format!("{PASS_HOTKEY_LABEL} (Global · {platform_name})"),
    );
    Some(manager)
}

#[cfg(target_os = "linux")]
fn spawn_wayland_portal_listener(
    tx: mpsc::SyncSender<GlobalHotkeyAction>,
    ctx: egui::Context,
    status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    portal_session: Arc<
        Mutex<Option<(zbus::blocking::Connection, zbus::zvariant::OwnedObjectPath)>>,
    >,
) {
    let _ = std::thread::Builder::new()
        .name("truesight-global-hotkey".into())
        .spawn(move || {
            if let Err(_err) = run_wayland_portal_loop(&tx, &ctx, &status, &stop, &portal_session) {
                if std::env::var_os("DISPLAY").is_some() {
                    // XWayland fallback covers XWayland games (e.g. Dota 2 via Proton or X11) if portal is absent.
                    let _x11_mgr = init_desktop_manager(tx, ctx, &status, "XWayland");
                    while !stop.load(Ordering::Acquire) {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                } else {
                    set_status(&status, format!("{PASS_HOTKEY_LABEL} (In-App)"));
                }
            }
        });
}

#[cfg(target_os = "linux")]
fn run_wayland_portal_loop(
    tx: &mpsc::SyncSender<GlobalHotkeyAction>,
    ctx: &egui::Context,
    status: &Arc<Mutex<String>>,
    stop: &Arc<AtomicBool>,
    portal_session: &Arc<
        Mutex<Option<(zbus::blocking::Connection, zbus::zvariant::OwnedObjectPath)>>,
    >,
) -> Result<(), String> {
    use std::collections::HashMap;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
    const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
    const PORTAL_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
    const REQ_IFACE: &str = "org.freedesktop.portal.Request";
    const SHORTCUT_ID: &str = "toggle_overlay_pass";

    let conn = Connection::session().map_err(|e| e.to_string())?;
    let unique = conn
        .unique_name()
        .ok_or_else(|| "missing D-Bus unique name".to_string())?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let pid = std::process::id();

    let portal =
        Proxy::new(&conn, PORTAL_DEST, PORTAL_PATH, PORTAL_IFACE).map_err(|e| e.to_string())?;

    let mut token_seq = 0_u32;
    let mut next_token = |prefix: &str| {
        token_seq += 1;
        format!("truesight_{prefix}_{pid}_{token_seq}")
    };

    let wait_response = |req_token: &str,
                         send_call: &dyn Fn(&str) -> zbus::Result<OwnedObjectPath>|
     -> Result<(u32, HashMap<String, OwnedValue>), String> {
        let predicted_path =
            format!("/org/freedesktop/portal/desktop/request/{unique}/{req_token}");
        let req_proxy = Proxy::new(&conn, PORTAL_DEST, predicted_path.as_str(), REQ_IFACE)
            .map_err(|e| e.to_string())?;
        let mut sigs = req_proxy
            .receive_signal("Response")
            .map_err(|e| e.to_string())?;
        let actual_path = send_call(req_token).map_err(|e| e.to_string())?;
        if actual_path.as_str() != predicted_path {
            let actual_proxy = Proxy::new(&conn, PORTAL_DEST, actual_path.as_str(), REQ_IFACE)
                .map_err(|e| e.to_string())?;
            sigs = actual_proxy
                .receive_signal("Response")
                .map_err(|e| e.to_string())?;
        }
        let msg = sigs
            .next()
            .ok_or_else(|| "portal Request::Response stream closed".to_string())?;
        msg.body()
            .deserialize::<(u32, HashMap<String, OwnedValue>)>()
            .map_err(|e| e.to_string())
    };

    // 1. Create GlobalShortcuts session
    let create_req_tok = next_token("req");
    let sess_tok = next_token("sess");
    let (create_code, create_res) = wait_response(&create_req_tok, &|tok| {
        let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
        opts.insert("handle_token", Value::from(tok));
        opts.insert("session_handle_token", Value::from(sess_tok.as_str()));
        portal.call("CreateSession", &(opts,))
    })?;
    if create_code != 0 {
        return Err(format!("CreateSession portal response {create_code}"));
    }

    let session_handle_str = create_res
        .get("session_handle")
        .and_then(|v| {
            v.downcast_ref::<zbus::zvariant::ObjectPath<'_>>()
                .ok()
                .map(|p| p.as_str().to_string())
                .or_else(|| v.downcast_ref::<String>().ok())
        })
        .ok_or_else(|| "missing session_handle in CreateSession response".to_string())?;
    let session_path = OwnedObjectPath::try_from(session_handle_str).map_err(|e| e.to_string())?;

    if let Ok(mut guard) = portal_session.lock() {
        *guard = Some((conn.clone(), session_path.clone()));
    }

    // 2. Check existing shortcuts so approved bindings do not re-prompt every launch.
    let list_req_tok = next_token("list");
    let mut already_bound = false;
    if let Ok((0, list_res)) = wait_response(&list_req_tok, &|tok| {
        let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
        opts.insert("handle_token", Value::from(tok));
        portal.call("ListShortcuts", &(session_path.as_ref(), opts))
    }) {
        if let Some(val) = list_res.get("shortcuts")
            && let Ok(entries) = val.downcast_ref::<zbus::zvariant::Array<'_>>()
        {
            for item in entries.iter() {
                if let Ok(pair) = item.downcast_ref::<zbus::zvariant::Structure<'_>>()
                    && let Some(first) = pair.fields().first()
                    && let Ok(id_str) = first.downcast_ref::<String>()
                    && id_str == SHORTCUT_ID
                {
                    already_bound = true;
                }
            }
        }
    }

    // 3. Bind Ctrl+Shift+P if not already registered for this session/app.
    if !already_bound {
        let bind_req_tok = next_token("bind");
        let (bind_code, _) = wait_response(&bind_req_tok, &|tok| {
            let mut shortcut_props: HashMap<&str, Value<'_>> = HashMap::new();
            shortcut_props.insert(
                "description",
                Value::from("Toggle TrueSight Overlay Click-Through"),
            );
            shortcut_props.insert("preferred_trigger", Value::from("Control+Shift+p"));
            let shortcuts = vec![(SHORTCUT_ID, shortcut_props)];
            let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
            opts.insert("handle_token", Value::from(tok));
            portal.call(
                "BindShortcuts",
                &(session_path.as_ref(), shortcuts, "", opts),
            )
        })?;
        if bind_code != 0 {
            return Err(format!("BindShortcuts portal response {bind_code}"));
        }
    }

    // 4. Listen for Activated signals while app runs.
    let mut activated_sigs = portal
        .receive_signal("Activated")
        .map_err(|e| e.to_string())?;
    set_status(
        status,
        format!("{PASS_HOTKEY_LABEL} (Global · Wayland Portal)"),
    );

    for msg in &mut activated_sigs {
        if stop.load(Ordering::Acquire) {
            break;
        }
        if let Ok((sig_session, shortcut_id, _ts, _opts)) =
            msg.body()
                .deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>()
            && sig_session == session_path
            && shortcut_id == SHORTCUT_ID
        {
            let _ = tx.try_send(GlobalHotkeyAction::ToggleOverlayPass);
            ctx.request_repaint();
        }
    }

    Ok(())
}
