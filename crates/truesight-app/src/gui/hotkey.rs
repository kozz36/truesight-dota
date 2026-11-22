use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

/// Human-readable label for the global click-through hotkey (`Ctrl+Shift+P`, leaving `F9` free for Dota 2 Pause).
pub(super) const PASS_HOTKEY_LABEL: &str = "Ctrl+Shift+P";

/// XDG GlobalShortcuts `preferred_trigger` syntax (`https://specifications.freedesktop.org/shortcuts/latest/`).
#[cfg(any(target_os = "linux", test))]
pub(super) const PORTAL_PREFERRED_TRIGGER: &str = "CTRL+SHIFT+p";

/// Formats the hotkey backend status label from the portal-reported `trigger_description`.
/// Falls back to `(In-App)` whenever no trigger is actually bound by the compositor.
#[cfg(any(target_os = "linux", test))]
pub(super) fn portal_status_label(assigned_trigger: Option<&str>) -> String {
    match assigned_trigger.map(str::trim).filter(|s| !s.is_empty()) {
        Some(trigger) => format!("{trigger} (Global · Wayland Portal)"),
        None => format!("{PASS_HOTKEY_LABEL} (In-App)"),
    }
}

/// Result of running the Wayland XDG GlobalShortcuts portal session.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PortalLoopOutcome {
    Finished,
    Cancelled,
    Unavailable(String),
}

/// Returns `true` only when the portal is unavailable (never after explicit user cancellation)
/// and an X11/XWayland `DISPLAY` is available for `XGrabKey` fallback.
#[cfg(any(target_os = "linux", test))]
pub(super) fn should_fallback_to_xwayland(outcome: &PortalLoopOutcome, has_display: bool) -> bool {
    matches!(outcome, PortalLoopOutcome::Unavailable(_)) && has_display
}

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
fn unwrap_value<'v, 'a>(mut val: &'v zbus::zvariant::Value<'a>) -> &'v zbus::zvariant::Value<'a> {
    while let zbus::zvariant::Value::Value(inner) = val {
        val = inner.as_ref();
    }
    val
}

#[cfg(target_os = "linux")]
fn value_as_str<'v, 'a>(val: &'v zbus::zvariant::Value<'a>) -> Option<&'v str> {
    match unwrap_value(val) {
        zbus::zvariant::Value::Str(s) => Some(s.as_str()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn value_as_non_empty_string(val: &zbus::zvariant::Value<'_>) -> Option<String> {
    value_as_str(val)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Inspects an XDG `shortcuts` `a(sa{sv})` variant payload for `target_id` and extracts
/// whether the entry exists plus any non-empty `trigger_description`.
#[cfg(target_os = "linux")]
pub(super) fn inspect_portal_shortcuts_value(
    val: &zbus::zvariant::Value<'_>,
    target_id: &str,
) -> (bool, Option<String>) {
    let mut listed = false;
    let mut assigned_trigger = None;
    let zbus::zvariant::Value::Array(entries) = unwrap_value(val) else {
        return (false, None);
    };
    for item in entries.iter() {
        let zbus::zvariant::Value::Structure(pair) = unwrap_value(item) else {
            continue;
        };
        let Some(id_val) = pair.fields().first() else {
            continue;
        };
        if value_as_str(id_val) != Some(target_id) {
            continue;
        }
        listed = true;
        if let Some(props_val) = pair.fields().get(1)
            && let zbus::zvariant::Value::Dict(dict) = unwrap_value(props_val)
        {
            for (k, v) in dict.iter() {
                if value_as_str(k) == Some("trigger_description")
                    && let Some(desc) = value_as_non_empty_string(v)
                {
                    assigned_trigger = Some(desc);
                }
            }
        }
    }
    (listed, assigned_trigger)
}

/// Extracts the non-empty `trigger_description` for `target_id` from a deserialized
/// `ShortcutsChanged` (`Vec<(String, HashMap<String, OwnedValue>)>`) payload.
#[cfg(target_os = "linux")]
pub(super) fn extract_assigned_trigger_from_pairs(
    pairs: &[(
        String,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    )],
    target_id: &str,
) -> Option<String> {
    pairs
        .iter()
        .find(|(id, _)| id == target_id)
        .and_then(|(_, props)| props.get("trigger_description"))
        .and_then(|v| value_as_non_empty_string(v))
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
            let outcome = run_wayland_portal_loop(&tx, &ctx, &status, &stop, &portal_session);
            if should_fallback_to_xwayland(&outcome, std::env::var_os("DISPLAY").is_some()) {
                // XWayland fallback covers XWayland games (e.g. Dota 2 via Proton or X11) if portal is absent.
                let _x11_mgr = init_desktop_manager(tx, ctx, &status, "XWayland");
                while !stop.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            } else if matches!(
                outcome,
                PortalLoopOutcome::Cancelled | PortalLoopOutcome::Unavailable(_)
            ) {
                set_status(&status, format!("{PASS_HOTKEY_LABEL} (In-App)"));
                ctx.request_repaint();
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
) -> PortalLoopOutcome {
    match run_wayland_portal_loop_inner(tx, ctx, status, stop, portal_session) {
        Ok(()) => PortalLoopOutcome::Finished,
        Err(outcome) => outcome,
    }
}

#[cfg(target_os = "linux")]
fn run_wayland_portal_loop_inner(
    tx: &mpsc::SyncSender<GlobalHotkeyAction>,
    ctx: &egui::Context,
    status: &Arc<Mutex<String>>,
    stop: &Arc<AtomicBool>,
    portal_session: &Arc<
        Mutex<Option<(zbus::blocking::Connection, zbus::zvariant::OwnedObjectPath)>>,
    >,
) -> Result<(), PortalLoopOutcome> {
    use std::collections::HashMap;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
    const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
    const PORTAL_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
    const REQ_IFACE: &str = "org.freedesktop.portal.Request";
    const SHORTCUT_ID: &str = "toggle_overlay_pass";

    let unavailable = |err: zbus::Error| PortalLoopOutcome::Unavailable(err.to_string());

    let conn = Connection::session().map_err(unavailable)?;
    let unique = conn
        .unique_name()
        .ok_or_else(|| PortalLoopOutcome::Unavailable("missing D-Bus unique name".into()))?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let pid = std::process::id();

    let portal = Proxy::new(&conn, PORTAL_DEST, PORTAL_PATH, PORTAL_IFACE).map_err(unavailable)?;

    let mut token_seq = 0_u32;
    let mut next_token = |prefix: &str| {
        token_seq += 1;
        format!("truesight_{prefix}_{pid}_{token_seq}")
    };

    let wait_response = |req_token: &str,
                         send_call: &dyn Fn(&str) -> zbus::Result<OwnedObjectPath>|
     -> Result<(u32, HashMap<String, OwnedValue>), PortalLoopOutcome> {
        let predicted_path =
            format!("/org/freedesktop/portal/desktop/request/{unique}/{req_token}");
        let req_proxy = Proxy::new(&conn, PORTAL_DEST, predicted_path.as_str(), REQ_IFACE)
            .map_err(unavailable)?;
        let mut sigs = req_proxy.receive_signal("Response").map_err(unavailable)?;
        let actual_path = send_call(req_token).map_err(unavailable)?;
        if actual_path.as_str() != predicted_path {
            let actual_proxy = Proxy::new(&conn, PORTAL_DEST, actual_path.as_str(), REQ_IFACE)
                .map_err(unavailable)?;
            sigs = actual_proxy
                .receive_signal("Response")
                .map_err(unavailable)?;
        }
        let msg = sigs.next().ok_or_else(|| {
            PortalLoopOutcome::Unavailable("portal Request::Response stream closed".into())
        })?;
        msg.body()
            .deserialize::<(u32, HashMap<String, OwnedValue>)>()
            .map_err(|e| PortalLoopOutcome::Unavailable(e.to_string()))
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
    if create_code == 1 {
        return Err(PortalLoopOutcome::Cancelled);
    }
    if create_code != 0 {
        return Err(PortalLoopOutcome::Unavailable(format!(
            "CreateSession portal response {create_code}"
        )));
    }

    let session_handle_str = create_res
        .get("session_handle")
        .and_then(|v| {
            v.downcast_ref::<zbus::zvariant::ObjectPath<'_>>()
                .ok()
                .map(|p| p.as_str().to_string())
                .or_else(|| v.downcast_ref::<String>().ok())
        })
        .ok_or_else(|| {
            PortalLoopOutcome::Unavailable(
                "missing session_handle in CreateSession response".into(),
            )
        })?;
    let session_path = OwnedObjectPath::try_from(session_handle_str)
        .map_err(|e| PortalLoopOutcome::Unavailable(e.to_string()))?;

    if let Ok(mut guard) = portal_session.lock() {
        *guard = Some((conn.clone(), session_path.clone()));
    }

    // Subscribe to both Activated and ShortcutsChanged before List/Bind to avoid signal races.
    let mut portal_sigs = portal.receive_all_signals().map_err(unavailable)?;

    // 2. Check existing shortcuts so approved bindings do not re-prompt every launch.
    let list_req_tok = next_token("list");
    let mut shortcut_listed = false;
    let mut assigned_trigger = None;
    if let Ok((0, list_res)) = wait_response(&list_req_tok, &|tok| {
        let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
        opts.insert("handle_token", Value::from(tok));
        portal.call("ListShortcuts", &(session_path.as_ref(), opts))
    }) && let Some(val) = list_res.get("shortcuts")
    {
        (shortcut_listed, assigned_trigger) = inspect_portal_shortcuts_value(val, SHORTCUT_ID);
    }

    // 3. Bind Ctrl+Shift+P using XDG shortcut spec (`CTRL+SHIFT+p`) if not already listed.
    if !shortcut_listed {
        let bind_req_tok = next_token("bind");
        let (bind_code, bind_res) = wait_response(&bind_req_tok, &|tok| {
            let mut shortcut_props: HashMap<&str, Value<'_>> = HashMap::new();
            shortcut_props.insert(
                "description",
                Value::from("Toggle TrueSight Overlay Click-Through"),
            );
            shortcut_props.insert("preferred_trigger", Value::from(PORTAL_PREFERRED_TRIGGER));
            let shortcuts = vec![(SHORTCUT_ID, shortcut_props)];
            let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
            opts.insert("handle_token", Value::from(tok));
            portal.call(
                "BindShortcuts",
                &(session_path.as_ref(), shortcuts, "", opts),
            )
        })?;
        if bind_code == 1 {
            return Err(PortalLoopOutcome::Cancelled);
        }
        if bind_code != 0 {
            return Err(PortalLoopOutcome::Unavailable(format!(
                "BindShortcuts portal response {bind_code}"
            )));
        }
        if let Some(val) = bind_res.get("shortcuts") {
            let (_, bind_trigger) = inspect_portal_shortcuts_value(val, SHORTCUT_ID);
            assigned_trigger = bind_trigger;
        }
    }

    // 4. Reflect only truly assigned triggers as Global; keep In-App status if unbound.
    set_status(status, portal_status_label(assigned_trigger.as_deref()));
    ctx.request_repaint();

    for msg in &mut portal_sigs {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let header = msg.header();
        let Some(member) = header.member() else {
            continue;
        };
        match member.as_str() {
            "Activated" => {
                if let Ok((sig_session, shortcut_id, _ts, _opts)) =
                    msg.body()
                        .deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>(
                        )
                    && sig_session == session_path
                    && shortcut_id == SHORTCUT_ID
                {
                    let _ = tx.try_send(GlobalHotkeyAction::ToggleOverlayPass);
                    ctx.request_repaint();
                }
            }
            "ShortcutsChanged" => {
                if let Ok((sig_session, shortcuts)) = msg
                    .body()
                    .deserialize::<(OwnedObjectPath, Vec<(String, HashMap<String, OwnedValue>)>)>()
                    && sig_session == session_path
                {
                    let next_trigger = extract_assigned_trigger_from_pairs(&shortcuts, SHORTCUT_ID);
                    set_status(status, portal_status_label(next_trigger.as_deref()));
                    ctx.request_repaint();
                }
            }
            _ => {}
        }
    }

    Ok(())
}
