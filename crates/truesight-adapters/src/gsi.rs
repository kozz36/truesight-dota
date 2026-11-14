use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use truesight_core::domain::{Faction, HeroId, Role, Side};
use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

/// Maximum accepted GSI HTTP payload size (64 KiB) to enforce memory bounds.
pub const MAX_GSI_PAYLOAD_BYTES: usize = 64 * 1024;
/// Bounded channel capacity for backpressure (`rust-architect` admission control).
pub const GSI_QUEUE_CAPACITY: usize = 16;

/// Minimal zero-dependency parser for Dota 2 GSI JSON payloads and custom draft commands.
pub fn parse_gsi_json_events(payload: &str) -> Vec<DraftEvent> {
    let mut events = Vec::new();

    if let Some(state) = extract_json_string(payload, "game_state") {
        let clock = extract_json_int(payload, "clock_time").unwrap_or(0);
        if state == "DOTA_GAMERULES_STATE_HERO_SELECTION"
            || state == "DOTA_GAMERULES_STATE_STRATEGY_TIME"
            || state == "DOTA_GAMERULES_STATE_PRE_GAME"
            || state == "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS"
        {
            events.push(DraftEvent::GameRulesStateChanged {
                state_name: state,
                game_time_secs: clock,
            });
        }
    }

    // Extract local player faction (`"player": { ... "team_name": "radiant" | "dire" }`) and Steam32 ID
    let player_slice = extract_json_object_slice(payload, "player").unwrap_or(payload);
    if let Some(raw_steamid) = extract_json_u64(player_slice, "steamid") {
        if let Some(steam32) = crate::sync::normalize_steam32_id(raw_steamid) {
            events.push(DraftEvent::PlayerSteamIdDetected { steam32 });
        }
    }
    if let Some(team_name) = extract_json_string(player_slice, "team_name") {
        let faction = match team_name.trim().to_ascii_lowercase().as_str() {
            "radiant" | "team2" | "goodguys" => Some(Faction::Radiant),
            "dire" | "team3" | "badguys" => Some(Faction::Dire),
            _ => None,
        };
        if let Some(f) = faction {
            events.push(DraftEvent::FactionDetected { faction: f });
        }
    }

    // Extract hero name from `"hero": { ... "name": "npc_dota_hero_..." }`
    if let Some(hero_slice) = extract_json_object_slice(payload, "hero") {
        if let Some(hero_slug) = extract_json_string(hero_slice, "name") {
            if hero_slug.starts_with("npc_dota_hero_") {
                events.push(DraftEvent::LocalPlayerHeroDetected { hero_slug });
            }
        }
    }

    // Support spectator/custom draft pick injection: `{"truesight_pick":{"side":"ally","slot":0,"hero_id":1}}`
    if let Some(slice) = extract_json_object_slice(payload, "truesight_pick") {
        let side = match extract_json_string(slice, "side").as_deref() {
            Some("enemy") | Some("enemies") | Some("dire") => Side::Enemies,
            _ => Side::Allies,
        };
        if let (Some(slot), Some(hero_id)) = (
            extract_json_int(slice, "slot"),
            extract_json_int(slice, "hero_id"),
        ) {
            if (0..5).contains(&slot) && (1..160).contains(&hero_id) {
                events.push(DraftEvent::PickDetected {
                    side,
                    slot: slot as usize,
                    hero_id: HeroId(hero_id as u16),
                    role: Role::ALL.get(slot as usize).copied(),
                });
            }
        }
    }

    events
}

fn extract_json_object_slice<'a>(input: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{key}\"");
    let start = input.find(&pattern)? + pattern.len();
    let rest = input[start..].trim_start();
    let after_colon = rest.strip_prefix(':')?.trim_start();
    if !after_colon.starts_with('{') {
        return None;
    }
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in after_colon.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&after_colon[..=idx]);
                }
            }
            _ => {}
        }
    }
    Some(after_colon)
}

fn extract_json_string(input: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\"");
    let start = input.find(&pattern)? + pattern.len();
    let rest = input[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn extract_json_int(input: &str, key: &str) -> Option<i32> {
    let pattern = format!("\"{key}\"");
    let start = input.find(&pattern)? + pattern.len();
    let rest = input[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let token_len = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .count();
    if token_len == 0 {
        return None;
    }
    rest[..token_len].parse::<i32>().ok()
}

fn extract_json_u64(input: &str, key: &str) -> Option<u64> {
    let pattern = format!("\"{key}\"");
    let start = input.find(&pattern)? + pattern.len();
    let rest = input[start..].trim_start();
    let after_colon = rest.strip_prefix(':')?.trim_start();
    let digits: String = after_colon
        .chars()
        .skip_while(|c| *c == '"')
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// Bounded local HTTP listener for Dota 2 Game State Integration (`127.0.0.1:<port>`).
pub struct BoundedGsiListener {
    rx: Receiver<DraftEvent>,
    shutdown: Arc<AtomicBool>,
    local_addr: SocketAddr,
    worker: Option<JoinHandle<()>>,
}

impl BoundedGsiListener {
    pub fn bind(addr: &str) -> Result<Self, PortError> {
        let listener = TcpListener::bind(addr).map_err(|e| PortError(e.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| PortError(e.to_string()))?;
        let local_addr = listener.local_addr().map_err(|e| PortError(e.to_string()))?;

        let (tx, rx) = mpsc::sync_channel(GSI_QUEUE_CAPACITY);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_worker = Arc::clone(&shutdown);

        let worker = thread::Builder::new()
            .name("truesight-gsi-listener".into())
            .spawn(move || run_accept_loop(listener, tx, shutdown_worker))
            .map_err(|e| PortError(e.to_string()))?;

        Ok(Self {
            rx,
            shutdown,
            local_addr,
            worker: Some(worker),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl DraftEventSource for BoundedGsiListener {
    type Error = PortError;

    fn poll_events(&mut self) -> Result<Vec<DraftEvent>, Self::Error> {
        let mut batch = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            batch.push(ev);
        }
        Ok(batch)
    }
}

impl Drop for BoundedGsiListener {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn run_accept_loop(
    listener: TcpListener,
    tx: SyncSender<DraftEvent>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                handle_gsi_connection(stream, &tx);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn handle_gsi_connection(mut stream: TcpStream, tx: &SyncSender<DraftEvent>) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));

    let mut buf = vec![0_u8; MAX_GSI_PAYLOAD_BYTES];
    let mut total_read = 0_usize;

    loop {
        if total_read >= MAX_GSI_PAYLOAD_BYTES {
            break;
        }
        match stream.read(&mut buf[total_read..]) {
            Ok(0) => break,
            Ok(n) => {
                total_read += n;
                if let Some(header_end) = find_double_crlf(&buf[..total_read]) {
                    let headers = String::from_utf8_lossy(&buf[..header_end]);
                    let content_len = parse_content_length(&headers).unwrap_or(0);
                    if header_end + content_len <= total_read {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }

    if total_read > 0 {
        let raw = String::from_utf8_lossy(&buf[..total_read]);
        let body = raw.split("\r\n\r\n").nth(1).unwrap_or(&raw);
        for ev in parse_gsi_json_events(body) {
            match tx.try_send(ev) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }

    let _ = std::io::Write::write_all(
        &mut stream,
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
    );
}

fn find_double_crlf(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            return rest.trim().parse::<usize>().ok();
        }
    }
    None
}
