use super::hotkey::{GlobalHotkeyAction, PASS_HOTKEY_LABEL, init_desktop_manager, set_status};
use crate::session::PassHotkey;
use eframe::egui;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};

/// Default XDG GlobalShortcuts `preferred_trigger` syntax (`https://specifications.freedesktop.org/shortcuts/latest/`).
pub(super) const PORTAL_PREFERRED_TRIGGER: &str = "CTRL+SHIFT+t";

#[cfg(target_os = "linux")]
pub(super) type PortalSessionSlot =
    Arc<Mutex<Option<(zbus::blocking::Connection, zbus::zvariant::OwnedObjectPath)>>>;

/// Formats the hotkey backend status label from the portal-reported `trigger_description`.
/// Falls back to `(In-App)` with `fallback_label` whenever no trigger is bound by the compositor.
pub(super) fn portal_status_label(assigned_trigger: Option<&str>, fallback_label: &str) -> String {
    match assigned_trigger.map(str::trim).filter(|s| !s.is_empty()) {
        Some(trigger) => format!("{trigger} (Global · Wayland Portal)"),
        None => format!("{fallback_label} (In-App)"),
    }
}

/// Result of running the Wayland XDG GlobalShortcuts portal session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PortalLoopOutcome {
    Finished,
    Cancelled,
    Unavailable(String),
}

/// Returns `true` only when the portal is unavailable (never after explicit user cancellation)
/// and an X11/XWayland `DISPLAY` is available for `XGrabKey` fallback.
pub(super) fn should_fallback_to_xwayland(outcome: &PortalLoopOutcome, has_display: bool) -> bool {
    matches!(outcome, PortalLoopOutcome::Unavailable(_)) && has_display
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
fn active_fallback_label(active_hotkey: &Arc<Mutex<PassHotkey>>) -> String {
    active_hotkey
        .lock()
        .map(|h| h.format_label())
        .unwrap_or_else(|_| PASS_HOTKEY_LABEL.to_string())
}

#[cfg(target_os = "linux")]
const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
#[cfg(target_os = "linux")]
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
#[cfg(target_os = "linux")]
const PORTAL_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
#[cfg(target_os = "linux")]
const REQ_IFACE: &str = "org.freedesktop.portal.Request";
#[cfg(target_os = "linux")]
const SHORTCUT_ID: &str = "toggle_overlay_pass";

#[cfg(target_os = "linux")]
static REBIND_TOKEN_SEQ: AtomicU32 = AtomicU32::new(1);

#[cfg(target_os = "linux")]
fn portal_wait_response(
    conn: &zbus::blocking::Connection,
    unique: &str,
    req_token: &str,
    send_call: &dyn Fn(&str) -> zbus::Result<zbus::zvariant::OwnedObjectPath>,
) -> Result<
    (
        u32,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    ),
    PortalLoopOutcome,
> {
    use std::collections::HashMap;
    use zbus::blocking::Proxy;
    use zbus::zvariant::OwnedValue;

    let unavailable = |err: zbus::Error| PortalLoopOutcome::Unavailable(err.to_string());
    let predicted_path = format!("/org/freedesktop/portal/desktop/request/{unique}/{req_token}");
    let req_proxy =
        Proxy::new(conn, PORTAL_DEST, predicted_path.as_str(), REQ_IFACE).map_err(unavailable)?;
    let mut sigs = req_proxy.receive_signal("Response").map_err(unavailable)?;
    let actual_path = send_call(req_token).map_err(unavailable)?;
    if actual_path.as_str() != predicted_path {
        let actual_proxy =
            Proxy::new(conn, PORTAL_DEST, actual_path.as_str(), REQ_IFACE).map_err(unavailable)?;
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
}

#[cfg(target_os = "linux")]
pub(super) fn rebind_existing_portal_session(
    conn: &zbus::blocking::Connection,
    session_path: &zbus::zvariant::OwnedObjectPath,
    preferred: PassHotkey,
    status: &Arc<Mutex<String>>,
) -> Result<(), PortalLoopOutcome> {
    use std::collections::HashMap;
    use zbus::blocking::Proxy;
    use zbus::zvariant::Value;

    let unavailable = |err: zbus::Error| PortalLoopOutcome::Unavailable(err.to_string());
    let unique = conn
        .unique_name()
        .ok_or_else(|| PortalLoopOutcome::Unavailable("missing D-Bus unique name".into()))?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let seq = REBIND_TOKEN_SEQ.fetch_add(1, Ordering::Relaxed);
    let req_tok = format!("truesight_rebind_{}_{seq}", std::process::id());
    let portal = Proxy::new(conn, PORTAL_DEST, PORTAL_PATH, PORTAL_IFACE).map_err(unavailable)?;
    let trigger = preferred.to_portal_trigger();
    let (code, res) = portal_wait_response(conn, &unique, &req_tok, &|tok| {
        let mut shortcut_props: HashMap<&str, Value<'_>> = HashMap::new();
        shortcut_props.insert(
            "description",
            Value::from("Toggle TrueSight Overlay Click-Through"),
        );
        shortcut_props.insert("preferred_trigger", Value::from(trigger.as_str()));
        let shortcuts = vec![(SHORTCUT_ID, shortcut_props)];
        let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
        opts.insert("handle_token", Value::from(tok));
        portal.call(
            "BindShortcuts",
            &(session_path.as_ref(), shortcuts, "", opts),
        )
    })?;

    if code == 0 {
        let assigned = res
            .get("shortcuts")
            .and_then(|val| inspect_portal_shortcuts_value(val, SHORTCUT_ID).1);
        set_status(
            status,
            portal_status_label(assigned.as_deref(), &preferred.format_label()),
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[expect(clippy::too_many_arguments)]
pub(super) fn spawn_wayland_portal_listener(
    tx: mpsc::SyncSender<GlobalHotkeyAction>,
    ctx: egui::Context,
    status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    portal_session: PortalSessionSlot,
    active_hotkey: Arc<Mutex<PassHotkey>>,
    force_bind: bool,
    rebind_in_flight: Arc<AtomicBool>,
) {
    let _ = std::thread::Builder::new()
        .name("truesight-global-hotkey".into())
        .spawn(move || {
            let outcome = run_wayland_portal_loop(
                &tx,
                &ctx,
                &status,
                &stop,
                &portal_session,
                &active_hotkey,
                force_bind,
                &rebind_in_flight,
            );
            rebind_in_flight.store(false, Ordering::Release);
            let fallback_lbl = active_fallback_label(&active_hotkey);
            if should_fallback_to_xwayland(&outcome, std::env::var_os("DISPLAY").is_some()) {
                let hk = active_hotkey.lock().map(|h| *h).unwrap_or_default();
                let x11_expected = Arc::new(AtomicU32::new(0));
                let (_x11_mgr, _x11_hk) =
                    init_desktop_manager(tx, ctx, &status, "XWayland", hk, x11_expected);
                while !stop.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            } else if matches!(
                outcome,
                PortalLoopOutcome::Cancelled | PortalLoopOutcome::Unavailable(_)
            ) {
                set_status(&status, format!("{fallback_lbl} (In-App)"));
                ctx.request_repaint();
            }
        });
}

#[cfg(target_os = "linux")]
#[expect(clippy::too_many_arguments)]
fn run_wayland_portal_loop(
    tx: &mpsc::SyncSender<GlobalHotkeyAction>,
    ctx: &egui::Context,
    status: &Arc<Mutex<String>>,
    stop: &Arc<AtomicBool>,
    portal_session: &PortalSessionSlot,
    active_hotkey: &Arc<Mutex<PassHotkey>>,
    force_bind: bool,
    rebind_in_flight: &Arc<AtomicBool>,
) -> PortalLoopOutcome {
    match run_wayland_portal_loop_inner(
        tx,
        ctx,
        status,
        stop,
        portal_session,
        active_hotkey,
        force_bind,
        rebind_in_flight,
    ) {
        Ok(()) => PortalLoopOutcome::Finished,
        Err(outcome) => outcome,
    }
}

#[cfg(target_os = "linux")]
#[expect(clippy::too_many_arguments)]
fn run_wayland_portal_loop_inner(
    tx: &mpsc::SyncSender<GlobalHotkeyAction>,
    ctx: &egui::Context,
    status: &Arc<Mutex<String>>,
    stop: &Arc<AtomicBool>,
    portal_session: &PortalSessionSlot,
    active_hotkey: &Arc<Mutex<PassHotkey>>,
    force_bind: bool,
    rebind_in_flight: &Arc<AtomicBool>,
) -> Result<(), PortalLoopOutcome> {
    use std::collections::HashMap;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

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

    let create_req_tok = next_token("req");
    let sess_tok = next_token("sess");
    let (create_code, create_res) =
        portal_wait_response(&conn, &unique, &create_req_tok, &|tok| {
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

    let mut portal_sigs = portal.receive_all_signals().map_err(unavailable)?;

    let list_req_tok = next_token("list");
    let mut shortcut_listed = false;
    let mut assigned_trigger = None;
    if let Ok((0, list_res)) = portal_wait_response(&conn, &unique, &list_req_tok, &|tok| {
        let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
        opts.insert("handle_token", Value::from(tok));
        portal.call("ListShortcuts", &(session_path.as_ref(), opts))
    }) && let Some(val) = list_res.get("shortcuts")
    {
        (shortcut_listed, assigned_trigger) = inspect_portal_shortcuts_value(val, SHORTCUT_ID);
    }

    if force_bind || !shortcut_listed {
        let preferred_trigger = active_hotkey
            .lock()
            .map(|h| h.to_portal_trigger())
            .unwrap_or_else(|_| PORTAL_PREFERRED_TRIGGER.to_string());
        let bind_req_tok = next_token("bind");
        let (bind_code, bind_res) = portal_wait_response(&conn, &unique, &bind_req_tok, &|tok| {
            let mut shortcut_props: HashMap<&str, Value<'_>> = HashMap::new();
            shortcut_props.insert(
                "description",
                Value::from("Toggle TrueSight Overlay Click-Through"),
            );
            shortcut_props.insert("preferred_trigger", Value::from(preferred_trigger.as_str()));
            let shortcuts = vec![(SHORTCUT_ID, shortcut_props)];
            let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
            opts.insert("handle_token", Value::from(tok));
            portal.call(
                "BindShortcuts",
                &(session_path.as_ref(), shortcuts, "", opts),
            )
        })?;
        rebind_in_flight.store(false, Ordering::Release);
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
    } else {
        rebind_in_flight.store(false, Ordering::Release);
    }

    let fallback_lbl = active_fallback_label(active_hotkey);
    set_status(
        status,
        portal_status_label(assigned_trigger.as_deref(), &fallback_lbl),
    );
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
                    let fallback_lbl = active_fallback_label(active_hotkey);
                    set_status(
                        status,
                        portal_status_label(next_trigger.as_deref(), &fallback_lbl),
                    );
                    ctx.request_repaint();
                }
            }
            _ => {}
        }
    }

    Ok(())
}
