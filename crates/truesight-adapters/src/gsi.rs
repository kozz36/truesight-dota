use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
        let clock = map_node
            .get("clock_time")
            .and_then(JsonVal::as_i32)
            .unwrap_or(0);
        if matches!(
            state,
            "DOTA_GAMERULES_STATE_INIT"
                | "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD"
                | "DOTA_GAMERULES_STATE_CUSTOM_GAME_SETUP"
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
    let is_replay =
        (0..10_usize).any(|idx| find_slot(player_node, &format!("player{idx}")).is_some());
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
                .unwrap_or(if idx < 5 {
                    Faction::Radiant
                } else {
                    Faction::Dire
                });
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
        let local_hero_slug = hero_node_opt
            .and_then(|hs| hs.get("name"))
            .and_then(JsonVal::as_str)
            .filter(|s| s.starts_with("npc_dota_hero_"))
            .map(ToOwned::to_owned);
        if let Some(ref slug) = local_hero_slug {
            events.push(DraftEvent::LocalPlayerHeroDetected {
                hero_slug: slug.clone(),
            });
        }
        if let Some(items_node) = root.get("items") {
            if matches!(items_node, JsonVal::Obj(_))
                && items_node.get("team2").is_none()
                && items_node.get("team3").is_none()
                && items_node.get("player0").is_none()
            {
                const ITEM_SLOT_KEYS: &[&str] = &[
                    "slot0",
                    "slot1",
                    "slot2",
                    "slot3",
                    "slot4",
                    "slot5",
                    "slot6",
                    "slot7",
                    "slot8",
                    "stash0",
                    "stash1",
                    "stash2",
                    "stash3",
                    "stash4",
                    "stash5",
                    "teleport0",
                    "teleport",
                    "neutral0",
                    "neutral",
                ];
                let mut saw_recognized_slot = false;
                let mut item_slugs = Vec::new();
                for &key in ITEM_SLOT_KEYS {
                    if let Some(slot_obj) = items_node.get(key) {
                        saw_recognized_slot = true;
                        if let Some(raw_name) = slot_obj.get("name").and_then(JsonVal::as_str) {
                            let trimmed = raw_name.trim();
                            if !trimmed.is_empty() && trimmed != "item_empty" {
                                let canonical = trimmed.strip_prefix("item_").unwrap_or(trimmed);
                                if !canonical.is_empty() {
                                    item_slugs.push(canonical.to_string());
                                }
                            }
                        }
                    }
                }
                if saw_recognized_slot {
                    events.push(DraftEvent::InventoryUpdated {
                        hero_slug: local_hero_slug,
                        item_slugs,
                    });
                }
            }
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

    if let Some(draft_node) = root.get("draft") {
        for sub in [
            Some(draft_node),
            draft_node.get("team2"),
            draft_node.get("team3"),
        ]
        .into_iter()
        .flatten()
        {
            for idx in 0..16_usize {
                let key = format!("ban{idx}_id");
                if let Some(hid) = sub.get(&key).and_then(JsonVal::as_i32) {
                    if (1..160).contains(&hid) {
                        let ev = DraftEvent::BanDetected {
                            hero_id: HeroId(hid as u16),
                        };
                        if !events.contains(&ev) {
                            events.push(ev);
                        }
                    }
                }
            }
        }
    }
    if let Some(ban_node) = root.get("truesight_ban") {
        if let Some(hid) = ban_node.get("hero_id").and_then(JsonVal::as_i32) {
            if (1..160).contains(&hid) {
                let ev = DraftEvent::BanDetected {
                    hero_id: HeroId(hid as u16),
                };
                if !events.contains(&ev) {
                    events.push(ev);
                }
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
        let JsonVal::Obj(entries) = self else {
            return None;
        };
        entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn as_str(&self) -> Option<&str> {
        if let JsonVal::Str(s) = self {
            Some(s.as_str())
        } else {
            None
        }
    }

    fn as_i32(&self) -> Option<i32> {
        if let JsonVal::Num(s) | JsonVal::Str(s) = self {
            s.trim().parse().ok()
        } else {
            None
        }
    }

    fn as_u64(&self) -> Option<u64> {
        if let JsonVal::Num(s) | JsonVal::Str(s) = self {
            s.trim().parse().ok()
        } else {
            None
        }
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
                self.src[self.pos..].starts_with(kw).then(|| {
                    self.pos += kw.len();
                    JsonVal::Other
                })
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
            if self.peek() != Some(b'"') {
                return None;
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return None;
            }
            self.pos += 1;
            entries.push((key, self.parse_value(depth)?));
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Some(JsonVal::Obj(entries));
                }
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
                b']' => {
                    self.pos += 1;
                    return Some(JsonVal::Other);
                }
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
                b'"' => {
                    self.pos += 1;
                    return Some(out);
                }
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
                0x20..=0x7F => {
                    out.push(b as char);
                    self.pos += 1;
                }
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
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek()? {
            b'0' => self.pos += 1,
            b'1'..=b'9' => {
                self.take_digits();
            }
            _ => return None,
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !self.take_digits() {
                return None;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !self.take_digits() {
                return None;
            }
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

/// Queue admission, coalescing, and overflow-recovery diagnostics (no payload data).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventQueueStats {
    pub enqueued: u64,
    pub coalesced: u64,
    pub dropped: u64,
    pub recovered_resyncs: u64,
}

type SideSlotPicks = [Option<(HeroId, Option<Role>)>; 5];

#[derive(Default)]
struct GsiStateSnapshot {
    pending_reset_state: Option<(String, i32)>,
    latest_game_state: Option<(String, i32)>,
    latest_steam32: Option<u64>,
    latest_faction: Option<Faction>,
    latest_local_hero: Option<String>,
    latest_player_mappings: Vec<(u64, Faction, String)>,
    latest_picks: [SideSlotPicks; 2],
    latest_bans: Vec<HeroId>,
    latest_inventory: Option<(Option<String>, Vec<String>)>,
}

impl GsiStateSnapshot {
    fn apply_event(&mut self, ev: &DraftEvent) -> bool {
        match ev {
            DraftEvent::GameRulesStateChanged {
                state_name,
                game_time_secs,
            } => {
                let prev = self.latest_game_state.as_ref().map(|(s, _)| s.as_str());
                let reset = (state_name == "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD"
                    && prev != Some(state_name.as_str()))
                    || (state_name == "DOTA_GAMERULES_STATE_HERO_SELECTION"
                        && matches!(prev, Some(p) if !matches!(p, "DOTA_GAMERULES_STATE_HERO_SELECTION" | "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD")));
                if reset {
                    self.latest_local_hero = None;
                    self.latest_player_mappings.clear();
                    self.latest_picks = Default::default();
                    self.latest_bans.clear();
                    self.latest_inventory = None;
                    self.pending_reset_state = Some((state_name.clone(), *game_time_secs));
                }
                self.latest_game_state = Some((state_name.clone(), *game_time_secs));
                reset
            }
            DraftEvent::PlayerSteamIdDetected { steam32 } => {
                self.latest_steam32 = Some(*steam32);
                false
            }
            DraftEvent::FactionDetected { faction } => {
                self.latest_faction = Some(*faction);
                false
            }
            DraftEvent::LocalPlayerHeroDetected { hero_slug } => {
                if self
                    .latest_inventory
                    .as_ref()
                    .is_some_and(|(owner, _)| owner.as_deref() != Some(hero_slug.as_str()))
                {
                    self.latest_inventory = None;
                }
                self.latest_local_hero = Some(hero_slug.clone());
                false
            }
            DraftEvent::PlayerHeroMapped {
                steam32,
                faction,
                hero_slug,
            } => {
                if let Some(entry) = self
                    .latest_player_mappings
                    .iter_mut()
                    .find(|(id, _, _)| id == steam32)
                {
                    *entry = (*steam32, *faction, hero_slug.clone());
                } else if self.latest_player_mappings.len() < 10 {
                    self.latest_player_mappings
                        .push((*steam32, *faction, hero_slug.clone()));
                }
                false
            }
            DraftEvent::PickDetected {
                side,
                slot,
                hero_id,
                role,
            } if *slot < 5 => {
                let side_idx = if matches!(side, Side::Allies) { 0 } else { 1 };
                self.latest_picks[side_idx][*slot] = Some((*hero_id, *role));
                false
            }
            DraftEvent::BanDetected { hero_id }
                if !self.latest_bans.contains(hero_id) && self.latest_bans.len() < 16 =>
            {
                self.latest_bans.push(*hero_id);
                false
            }
            DraftEvent::InventoryUpdated {
                hero_slug,
                item_slugs,
            } => {
                if hero_slug.is_some()
                    && self.latest_local_hero.is_some()
                    && hero_slug.as_deref() != self.latest_local_hero.as_deref()
                {
                    return false;
                }
                let owner = hero_slug.clone().or_else(|| self.latest_local_hero.clone());
                self.latest_inventory = Some((owner, item_slugs.clone()));
                false
            }
            _ => false,
        }
    }

    fn push_reconcile_events(
        &self,
        out: &mut Vec<DraftEvent>,
        pending_reset: Option<(String, i32)>,
    ) {
        if let Some((s, t)) = pending_reset {
            if let Some(pos) = out.iter().position(|e| matches!(e, DraftEvent::GameRulesStateChanged { state_name, .. } if state_name == &s)) {
                let existing = out.remove(pos);
                out.insert(0, existing);
            } else {
                out.insert(0, DraftEvent::GameRulesStateChanged { state_name: s, game_time_secs: t });
            }
        }
        let upsert = |list: &mut Vec<DraftEvent>, ev: DraftEvent| {
            if let Some(pos) = list.iter().position(|old| should_coalesce(old, &ev)) {
                list[pos] = ev;
            } else {
                list.push(ev);
            }
        };
        if let Some(steam32) = self.latest_steam32 {
            upsert(out, DraftEvent::PlayerSteamIdDetected { steam32 });
        }
        if let Some(faction) = self.latest_faction {
            upsert(out, DraftEvent::FactionDetected { faction });
        }
        if let Some(ref hero_slug) = self.latest_local_hero {
            upsert(
                out,
                DraftEvent::LocalPlayerHeroDetected {
                    hero_slug: hero_slug.clone(),
                },
            );
        }
        for (steam32, faction, hero_slug) in &self.latest_player_mappings {
            upsert(
                out,
                DraftEvent::PlayerHeroMapped {
                    steam32: *steam32,
                    faction: *faction,
                    hero_slug: hero_slug.clone(),
                },
            );
        }
        for hero_id in &self.latest_bans {
            upsert(out, DraftEvent::BanDetected { hero_id: *hero_id });
        }
        for (side, slots) in [Side::Allies, Side::Enemies]
            .into_iter()
            .zip(self.latest_picks.iter())
        {
            for (slot, entry) in slots.iter().enumerate() {
                if let Some((hero_id, role)) = entry {
                    upsert(
                        out,
                        DraftEvent::PickDetected {
                            side,
                            slot,
                            hero_id: *hero_id,
                            role: *role,
                        },
                    );
                }
            }
        }
        // Remove any prior queued InventoryUpdated before replaying the snapshot inventory after
        // LocalPlayerHeroDetected so fresh items always land under the reconciled hero identity.
        out.retain(|e| !matches!(e, DraftEvent::InventoryUpdated { .. }));
        if let Some((ref inv_hero, ref item_slugs)) = self.latest_inventory {
            let matches_local = match (&self.latest_local_hero, inv_hero) {
                (Some(local), Some(inv)) => local == inv,
                (Some(_), None) => false,
                (None, _) => true,
            };
            if matches_local {
                out.push(DraftEvent::InventoryUpdated {
                    hero_slug: inv_hero.clone(),
                    item_slugs: item_slugs.clone(),
                });
            }
        }
        if let Some((ref state_name, game_time_secs)) = self.latest_game_state {
            upsert(
                out,
                DraftEvent::GameRulesStateChanged {
                    state_name: state_name.clone(),
                    game_time_secs,
                },
            );
        }
    }
}

#[derive(Default)]
struct QueueInner {
    queue: VecDeque<DraftEvent>,
    capacity: usize,
    epoch: u64,
    reset_requested: bool,
    overflow_resync: bool,
    resync_pending_completion: bool,
    closed: bool,
    stats: EventQueueStats,
    gsi_snapshot: GsiStateSnapshot,
}

/// Bounded thread-safe queue with key coalescing, critical-event preservation, and resync triggers.
pub struct BoundedDraftQueue {
    inner: Mutex<QueueInner>,
}

impl BoundedDraftQueue {
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            inner: Mutex::new(QueueInner {
                queue: VecDeque::with_capacity(cap),
                capacity: cap,
                ..QueueInner::default()
            }),
        }
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, QueueInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn close(&self) {
        self.lock_inner().closed = true;
    }

    pub fn request_reset(&self) {
        let mut g = self.lock_inner();
        g.epoch = g.epoch.wrapping_add(1);
        g.reset_requested = true;
        g.overflow_resync = false;
        g.resync_pending_completion = false;
        g.queue.clear();
    }

    pub fn needs_reset_or_resync(&self) -> bool {
        let g = self.lock_inner();
        g.reset_requested || (g.overflow_resync && g.queue.is_empty())
    }

    pub fn begin_poll_cycle(&self) -> (u64, bool) {
        let mut g = self.lock_inner();
        if g.reset_requested {
            g.reset_requested = false;
            return (g.epoch, true);
        }
        if g.overflow_resync && g.queue.is_empty() {
            g.overflow_resync = false;
            g.resync_pending_completion = true;
            return (g.epoch, true);
        }
        (g.epoch, false)
    }

    fn push_into_inner(g: &mut QueueInner, poll_seq: u64, ev: DraftEvent) -> bool {
        if g.closed {
            return false;
        }
        if poll_seq != g.epoch {
            return true;
        }
        if let DraftEvent::LocalPlayerHeroDetected {
            hero_slug: ref new_hero,
        } = ev
        {
            g.queue.retain(|queued| {
                !matches!(
                    queued,
                    DraftEvent::InventoryUpdated { hero_slug, .. }
                        if hero_slug.as_deref() != Some(new_hero.as_str())
                )
            });
        } else if let DraftEvent::InventoryUpdated {
            hero_slug: Some(ref inv_hero),
            ..
        } = ev
        {
            let stale_for_queued_hero = g.queue.iter().any(|queued| {
                matches!(
                    queued,
                    DraftEvent::LocalPlayerHeroDetected { hero_slug }
                        if hero_slug != inv_hero
                )
            });
            if stale_for_queued_hero {
                return true;
            }
        }
        g.stats.enqueued += 1;
        if let Some(pos) = g.queue.iter().position(|old| should_coalesce(old, &ev)) {
            g.queue[pos] = ev;
            g.stats.coalesced += 1;
            if g.resync_pending_completion {
                g.resync_pending_completion = false;
                g.stats.recovered_resyncs += 1;
            }
            return true;
        }
        if g.queue.len() < g.capacity {
            g.queue.push_back(ev);
            if g.resync_pending_completion {
                g.resync_pending_completion = false;
                g.stats.recovered_resyncs += 1;
            }
        } else {
            g.overflow_resync = true;
            g.stats.dropped += 1;
            if is_critical_event(&ev) {
                if let Some(idx) = g.queue.iter().position(|e| !is_critical_event(e)) {
                    g.queue.remove(idx);
                    g.queue.push_back(ev);
                }
            }
        }
        true
    }

    pub fn push_for_epoch(&self, poll_seq: u64, ev: DraftEvent) -> bool {
        let mut g = self.lock_inner();
        Self::push_into_inner(&mut g, poll_seq, ev)
    }

    pub fn record_and_push_gsi_events(&self, events: Vec<DraftEvent>) -> bool {
        let mut g = self.lock_inner();
        for mut ev in events {
            if let DraftEvent::InventoryUpdated {
                ref mut hero_slug, ..
            } = ev
                && hero_slug.is_none()
            {
                *hero_slug = g.gsi_snapshot.latest_local_hero.clone();
            }
            if g.gsi_snapshot.apply_event(&ev) {
                g.queue.retain(|queued| {
                    matches!(
                        queued,
                        DraftEvent::FactionDetected { .. }
                            | DraftEvent::PlayerSteamIdDetected { .. }
                    )
                });
            }
            if !Self::push_into_inner(&mut g, 0, ev) {
                return false;
            }
        }
        true
    }

    pub fn drain(&self) -> Vec<DraftEvent> {
        self.lock_inner().queue.drain(..).collect()
    }

    pub fn drain_and_reconcile_gsi(&self) -> Vec<DraftEvent> {
        let mut g = self.lock_inner();
        let mut out: Vec<DraftEvent> = g.queue.drain(..).collect();
        let pending_reset = g.gsi_snapshot.pending_reset_state.take();
        if g.overflow_resync {
            let snapshot = &g.gsi_snapshot;
            snapshot.push_reconcile_events(&mut out, pending_reset);
            g.overflow_resync = false;
            g.resync_pending_completion = false;
            g.stats.recovered_resyncs += 1;
        }
        out
    }

    pub fn stats(&self) -> EventQueueStats {
        self.lock_inner().stats
    }
}

fn is_reset_state(state: &str) -> bool {
    matches!(
        state,
        "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD" | "DOTA_GAMERULES_STATE_HERO_SELECTION"
    )
}

fn is_critical_event(ev: &DraftEvent) -> bool {
    use DraftEvent::*;
    matches!(
        ev,
        GameRulesStateChanged { .. } | FactionDetected { .. } | PlayerSteamIdDetected { .. }
    )
}

fn should_coalesce(existing: &DraftEvent, incoming: &DraftEvent) -> bool {
    use DraftEvent::*;
    match (existing, incoming) {
        (FactionDetected { .. }, FactionDetected { .. })
        | (PlayerSteamIdDetected { .. }, PlayerSteamIdDetected { .. })
        | (LocalPlayerHeroDetected { .. }, LocalPlayerHeroDetected { .. })
        | (InventoryUpdated { .. }, InventoryUpdated { .. }) => true,
        (
            PickDetected {
                side: s1, slot: l1, ..
            },
            PickDetected {
                side: s2, slot: l2, ..
            },
        ) => s1 == s2 && l1 == l2,
        (BanDetected { hero_id: a }, BanDetected { hero_id: b }) => a == b,
        (PlayerHeroMapped { steam32: a, .. }, PlayerHeroMapped { steam32: b, .. }) => a == b,
        (
            GameRulesStateChanged { state_name: s1, .. },
            GameRulesStateChanged { state_name: s2, .. },
        ) => !is_reset_state(s1) || s1 == s2,
        _ => false,
    }
}

/// Bounded local HTTP listener for Dota 2 Game State Integration (`127.0.0.1:<port>`).
pub struct BoundedGsiListener {
    queue: Arc<BoundedDraftQueue>,
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
        let local_addr = listener
            .local_addr()
            .map_err(|e| PortError(e.to_string()))?;

        let queue = Arc::new(BoundedDraftQueue::new(GSI_QUEUE_CAPACITY));
        let queue_worker = Arc::clone(&queue);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_worker = Arc::clone(&shutdown);

        let worker = thread::Builder::new()
            .name("truesight-gsi-listener".into())
            .spawn(move || run_accept_loop(listener, queue_worker, shutdown_worker))
            .map_err(|e| PortError(e.to_string()))?;

        Ok(Self {
            queue,
            shutdown,
            local_addr,
            worker: Some(worker),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn queue_stats(&self) -> EventQueueStats {
        self.queue.stats()
    }
}

impl DraftEventSource for BoundedGsiListener {
    type Error = PortError;

    fn poll_events(&mut self) -> Result<Vec<DraftEvent>, Self::Error> {
        Ok(self.queue.drain_and_reconcile_gsi())
    }
}

impl Drop for BoundedGsiListener {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.queue.close();
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn run_accept_loop(
    listener: TcpListener,
    queue: Arc<BoundedDraftQueue>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                handle_gsi_connection(stream, &queue);
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

fn handle_gsi_connection(mut stream: TcpStream, queue: &BoundedDraftQueue) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));

    let mut raw_buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let mut header_end: Option<usize> = None;
    let mut expected_total: Option<usize> = None;
    let deadline = std::time::Instant::now() + Duration::from_millis(250);

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
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(5));
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

    if !queue.record_and_push_gsi_events(parse_gsi_events_from_root(&root)) {
        return;
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
    if parts.next().is_some() || target.is_empty() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
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
            let parsed = val
                .trim()
                .parse::<usize>()
                .map_err(|_| (400, "Bad Request"))?;
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
