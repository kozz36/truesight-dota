use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use truesight_core::domain::{Faction, HeroId, Role, Side};
use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

/// Maximum accepted GSI HTTP payload size (64 KiB) to enforce memory bounds.
pub const MAX_GSI_PAYLOAD_BYTES: usize = 64 * 1024;
/// Maximum accepted GSI HTTP header section size (8 KiB).
const MAX_GSI_HEADER_BYTES: usize = 8 * 1024;
/// Maximum JSON nesting depth accepted from GSI payloads.
const MAX_JSON_DEPTH: usize = 16;
/// Bounded channel capacity for backpressure (`rust-architect` admission control).
pub const GSI_QUEUE_CAPACITY: usize = 16;

/// Parses structural Dota 2 GSI JSON payloads and custom draft commands without regex/substring scans.
pub fn parse_gsi_json_events(payload: &str) -> Vec<DraftEvent> {
    parse_json_root(payload)
        .filter(|root| matches!(root, JsonVal::Obj(_)))
        .map(|root| parse_gsi_events_from_root(&root))
        .unwrap_or_default()
}

fn parse_gsi_events_from_root(root: &JsonVal) -> Vec<DraftEvent> {
    let mut events = Vec::new();

    let map_node = root.get("map").unwrap_or(root);
    if let Some(state) = map_node.get("game_state").and_then(JsonVal::as_str) {
        let clock = map_node.get("clock_time").and_then(JsonVal::as_i32).unwrap_or(0);
        if matches!(
            state,
            "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD"
                | "DOTA_GAMERULES_STATE_HERO_SELECTION"
                | "DOTA_GAMERULES_STATE_STRATEGY_TIME"
                | "DOTA_GAMERULES_STATE_PRE_GAME"
                | "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS"
                | "DOTA_GAMERULES_STATE_POST_GAME"
                | "DOTA_GAMERULES_STATE_DISCONNECT"
        ) {
            events.push(DraftEvent::GameRulesStateChanged {
                state_name: state.to_string(),
                game_time_secs: clock,
            });
        }
    }

    let player_node = root.get("player").unwrap_or(root);
    let hero_node_opt = root.get("hero");
    let is_replay = (0..10_usize).any(|idx| find_slot(player_node, &format!("player{idx}")).is_some());
    if is_replay {
        for idx in 0..10_usize {
            let key = format!("player{idx}");
            let Some(p_sub) = find_slot(player_node, &key) else {
                continue;
            };
            let Some(steam32) = p_sub
                .get("steamid")
                .and_then(JsonVal::as_u64)
                .and_then(crate::sync::normalize_steam32_id)
            else {
                continue;
            };
            let faction = p_sub
                .get("team_name")
                .and_then(JsonVal::as_str)
                .and_then(parse_faction_name)
                .unwrap_or(if idx < 5 { Faction::Radiant } else { Faction::Dire });
            let hero_slug = hero_node_opt
                .and_then(|hn| find_slot(hn, &key))
                .and_then(|hs| hs.get("name"))
                .and_then(JsonVal::as_str)
                .unwrap_or("")
                .to_string();
            events.push(DraftEvent::PlayerHeroMapped {
                steam32,
                faction,
                hero_slug,
            });
        }
    } else {
        if let Some(steam32) = player_node
            .get("steamid")
            .and_then(JsonVal::as_u64)
            .and_then(crate::sync::normalize_steam32_id)
        {
            events.push(DraftEvent::PlayerSteamIdDetected { steam32 });
        }
        if let Some(f) = player_node
            .get("team_name")
            .and_then(JsonVal::as_str)
            .and_then(parse_faction_name)
        {
            events.push(DraftEvent::FactionDetected { faction: f });
        }
        if let Some(slug) = hero_node_opt
            .and_then(|hs| hs.get("name"))
            .and_then(JsonVal::as_str)
            .filter(|s| s.starts_with("npc_dota_hero_"))
        {
            events.push(DraftEvent::LocalPlayerHeroDetected {
                hero_slug: slug.to_string(),
            });
        }
    }

    if let Some(pick) = root.get("truesight_pick") {
        let side = match pick.get("side").and_then(JsonVal::as_str) {
            Some("enemy") | Some("enemies") | Some("dire") => Side::Enemies,
            _ => Side::Allies,
        };
        if let (Some(slot), Some(hero_id)) = (
            pick.get("slot").and_then(JsonVal::as_i32),
            pick.get("hero_id").and_then(JsonVal::as_i32),
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

fn find_slot<'a>(node: &'a JsonVal, key: &str) -> Option<&'a JsonVal> {
    node.get(key)
        .or_else(|| node.get("team2").and_then(|t| t.get(key)))
        .or_else(|| node.get("team3").and_then(|t| t.get(key)))
}

fn parse_faction_name(raw: &str) -> Option<Faction> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "radiant" | "team2" | "goodguys" => Some(Faction::Radiant),
        "dire" | "team3" | "badguys" => Some(Faction::Dire),
        _ => None,
    }
}

enum JsonVal {
    Obj(Vec<(String, JsonVal)>),
    Str(String),
    Num(String),
    Other,
}

impl JsonVal {
    fn get(&self, key: &str) -> Option<&JsonVal> {
        let JsonVal::Obj(entries) = self else { return None };
        entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn as_str(&self) -> Option<&str> {
        if let JsonVal::Str(s) = self { Some(s.as_str()) } else { None }
    }

    fn as_i32(&self) -> Option<i32> {
        if let JsonVal::Num(s) | JsonVal::Str(s) = self { s.trim().parse().ok() } else { None }
    }

    fn as_u64(&self) -> Option<u64> {
        if let JsonVal::Num(s) | JsonVal::Str(s) = self { s.trim().parse().ok() } else { None }
    }
}

struct JsonParser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    fn take_digits(&mut self) -> bool {
        let start = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        self.pos > start
    }

    fn parse_value(&mut self, depth: usize) -> Option<JsonVal> {
        if depth > MAX_JSON_DEPTH {
            return None;
        }
        self.skip_ws();
        match self.peek()? {
            b'{' => self.parse_object(depth + 1),
            b'[' => self.parse_array(depth + 1),
            b'"' => self.parse_string().map(JsonVal::Str),
            b't' | b'f' | b'n' => ["true", "false", "null"].iter().find_map(|kw| {
                self.src[self.pos..]
                    .starts_with(kw)
                    .then(|| { self.pos += kw.len(); JsonVal::Other })
            }),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => None,
        }
    }

    fn parse_object(&mut self, depth: usize) -> Option<JsonVal> {
        self.pos += 1;
        self.skip_ws();
        let mut entries = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Some(JsonVal::Obj(entries));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') { return None; }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') { return None; }
            self.pos += 1;
            entries.push((key, self.parse_value(depth)?));
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => { self.pos += 1; return Some(JsonVal::Obj(entries)); }
                _ => return None,
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Option<JsonVal> {
        self.pos += 1;
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Some(JsonVal::Other);
        }
        loop {
            self.parse_value(depth)?;
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b']' => { self.pos += 1; return Some(JsonVal::Other); }
                _ => return None,
            }
        }
    }

    fn parse_string(&mut self) -> Option<String> {
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.src.len() {
            let b = self.src.as_bytes()[self.pos];
            match b {
                b'"' => { self.pos += 1; return Some(out); }
                b'\\' => {
                    self.pos += 1;
                    let esc = self.peek()?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\x08'),
                        b'f' => out.push('\x0c'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self.src.get(self.pos..self.pos + 4)?;
                            let cp = u32::from_str_radix(hex, 16).ok()?;
                            out.push(char::from_u32(cp)?);
                            self.pos += 4;
                        }
                        _ => return None,
                    }
                }
                0x00..=0x1F => return None,
                0x20..=0x7F => { out.push(b as char); self.pos += 1; }
                _ => {
                    let ch = self.src[self.pos..].chars().next()?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        None
    }

    fn parse_number(&mut self) -> Option<JsonVal> {
        let start = self.pos;
        if self.peek() == Some(b'-') { self.pos += 1; }
        match self.peek()? {
            b'0' => self.pos += 1,
            b'1'..=b'9' => { self.take_digits(); }
            _ => return None,
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !self.take_digits() { return None; }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) { self.pos += 1; }
            if !self.take_digits() { return None; }
        }
        Some(JsonVal::Num(self.src[start..self.pos].to_string()))
    }
}

fn parse_json_root(input: &str) -> Option<JsonVal> {
    let mut parser = JsonParser { src: input, pos: 0 };
    let val = parser.parse_value(0)?;
    parser.skip_ws();
    (parser.pos == input.len()).then_some(val)
}

/// Validates that a configured GSI bind address resolves strictly to loopback (`127.0.0.0/8` or `::1`).
pub fn validate_gsi_bind_addr(addr: &str) -> Result<SocketAddr, PortError> {
    let addrs: Vec<SocketAddr> = addr
        .to_socket_addrs()
        .map_err(|e| PortError(format!("Invalid GSI bind address '{addr}': {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(PortError(format!(
            "Invalid GSI bind address '{addr}': resolved to no addresses"
        )));
    }
    if addrs.iter().any(|sa| !sa.ip().is_loopback()) {
        return Err(PortError(format!(
            "Refusing non-loopback GSI bind address '{addr}'; only 127.0.0.1 / ::1 loopback is permitted"
        )));
    }
    Ok(addrs[0])
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
        let sock_addr = validate_gsi_bind_addr(addr)?;
        let listener = TcpListener::bind(sock_addr).map_err(|e| PortError(e.to_string()))?;
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

    let mut raw_buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let mut header_end: Option<usize> = None;
    let mut expected_total: Option<usize> = None;

    loop {
        let cap = expected_total.unwrap_or(MAX_GSI_HEADER_BYTES + MAX_GSI_PAYLOAD_BYTES);
        if raw_buf.len() >= cap {
            break;
        }
        let max_read = (cap - raw_buf.len()).min(chunk.len());
        match stream.read(&mut chunk[..max_read]) {
            Ok(0) => break,
            Ok(n) => {
                raw_buf.extend_from_slice(&chunk[..n]);
                if header_end.is_none() {
                    if let Some(end) = find_double_crlf(&raw_buf) {
                        if end > MAX_GSI_HEADER_BYTES {
                            write_http_response(&mut stream, 400, "Bad Request");
                            return;
                        }
                        match validate_http_headers(&raw_buf[..end]) {
                            Ok(content_len) => {
                                header_end = Some(end);
                                expected_total = end.checked_add(content_len);
                                if expected_total.is_none() {
                                    write_http_response(&mut stream, 413, "Payload Too Large");
                                    return;
                                }
                            }
                            Err((status, reason)) => {
                                write_http_response(&mut stream, status, reason);
                                return;
                            }
                        }
                    } else if raw_buf.len() > MAX_GSI_HEADER_BYTES {
                        write_http_response(&mut stream, 400, "Bad Request");
                        return;
                    }
                }
                if let Some(need) = expected_total {
                    if raw_buf.len() >= need {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }

    let Some(end) = header_end else {
        if !raw_buf.is_empty() {
            write_http_response(&mut stream, 400, "Bad Request");
        }
        return;
    };
    let need = expected_total.unwrap_or(end);
    if raw_buf.len() < need {
        write_http_response(&mut stream, 400, "Bad Request");
        return;
    }

    let Ok(body) = std::str::from_utf8(&raw_buf[end..need]) else {
        write_http_response(&mut stream, 400, "Bad Request");
        return;
    };
    let Some(root) = parse_json_root(body).filter(|r| matches!(r, JsonVal::Obj(_))) else {
        write_http_response(&mut stream, 400, "Bad Request");
        return;
    };

    for ev in parse_gsi_events_from_root(&root) {
        match tx.try_send(ev) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => return,
        }
    }
    write_http_response(&mut stream, 200, "OK");
}

fn validate_http_headers(header_bytes: &[u8]) -> Result<usize, (u16, &'static str)> {
    let headers = std::str::from_utf8(header_bytes).map_err(|_| (400, "Bad Request"))?;
    let mut lines = headers.split("\r\n");
    let req_line = lines.next().ok_or((400, "Bad Request"))?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next().ok_or((400, "Bad Request"))?;
    let target = parts.next().ok_or((400, "Bad Request"))?;
    let version = parts.next().ok_or((400, "Bad Request"))?;
    if parts.next().is_some()
        || target.is_empty()
        || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
    {
        return Err((400, "Bad Request"));
    }
    if method != "POST" {
        return Err((405, "Method Not Allowed"));
    }

    let mut content_len: Option<usize> = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, val) = line.split_once(':').ok_or((400, "Bad Request"))?;
        let lower = name.trim().to_ascii_lowercase();
        if lower == "transfer-encoding" {
            return Err((400, "Bad Request"));
        }
        if lower == "content-length" {
            let parsed = val.trim().parse::<usize>().map_err(|_| (400, "Bad Request"))?;
            if parsed > MAX_GSI_PAYLOAD_BYTES {
                return Err((413, "Payload Too Large"));
            }
            if content_len.is_some_and(|prev| prev != parsed) {
                return Err((400, "Bad Request"));
            }
            content_len = Some(parsed);
        }
    }
    content_len.ok_or((411, "Length Required"))
}

fn write_http_response(stream: &mut TcpStream, status: u16, reason: &str) {
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reason}",
        reason.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn find_double_crlf(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4)
}
