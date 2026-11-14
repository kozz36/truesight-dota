use crate::meta_repo::SeededMetaRepository;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use truesight_core::domain::HeroId;
use truesight_core::ports::PortError;

/// OpenDota `/api/heroStats` refreshes its rolling daily window once per 24h UTC.
pub const META_TTL_SECS: u64 = 86_400;
/// OpenDota `/api/players/{id}/heroes` updates within minutes of completing a match; refresh hourly.
pub const PLAYER_TTL_SECS: u64 = 3_600;
/// Steam64 base offset (`Steam64 = Steam32 + 76561197960265728`).
pub const STEAM64_OFFSET: u64 = 76_561_197_960_265_728;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountLinkConfig {
    Auto,
    Explicit(u64),
    Unlinked,
}

/// Converts either a Steam64 (`7656119...`) or Steam32 (`u32`) identifier into a valid Steam32 ID.
pub fn normalize_steam32_id(raw: u64) -> Option<u64> {
    if raw > STEAM64_OFFSET {
        let s32 = raw - STEAM64_OFFSET;
        if (1..=u64::from(u32::MAX)).contains(&s32) {
            return Some(s32);
        }
        return None;
    }
    if (1..=u64::from(u32::MAX)).contains(&raw) {
        Some(raw)
    } else {
        None
    }
}

/// Parses Valve's `loginusers.vdf` text and returns the `"MostRecent" "1"` account's Steam32 ID
/// (falling back to the first valid Steam64 user entry if none is marked `MostRecent`).
pub fn parse_loginusers_vdf(vdf_content: &str) -> Option<u64> {
    #[derive(Debug)]
    enum Tok {
        Open,
        Close,
        Str(String),
    }

    let mut tokens = Vec::new();
    let bytes = vdf_content.as_bytes();
    let mut i = 0_usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'{' {
            tokens.push(Tok::Open);
            i += 1;
            continue;
        }
        if b == b'}' {
            tokens.push(Tok::Close);
            i += 1;
            continue;
        }
        if b == b'"' {
            i += 1;
            let mut s = String::new();
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    s.push(bytes[i + 1] as char);
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    s.push(bytes[i] as char);
                    i += 1;
                }
            }
            tokens.push(Tok::Str(s));
            continue;
        }
        // Unquoted bareword token
        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'{'
            && bytes[i] != b'}'
            && bytes[i] != b'"'
        {
            i += 1;
        }
        if let Ok(word) = std::str::from_utf8(&bytes[start..i]) {
            tokens.push(Tok::Str(word.to_string()));
        }
    }

    let mut depth = 0_usize;
    let mut current_user: Option<u64> = None;
    let mut first_valid: Option<u64> = None;
    let mut pending_key: Option<String> = None;
    let mut idx = 0_usize;

    while idx < tokens.len() {
        match &tokens[idx] {
            Tok::Open => {
                depth += 1;
                pending_key = None;
            }
            Tok::Close => {
                if depth == 2 {
                    current_user = None;
                }
                depth = depth.saturating_sub(1);
                pending_key = None;
            }
            Tok::Str(val) => {
                if depth == 1 && matches!(tokens.get(idx + 1), Some(Tok::Open)) {
                    if let Ok(raw) = val.trim().parse::<u64>() {
                        if raw > STEAM64_OFFSET {
                            if let Some(s32) = normalize_steam32_id(raw) {
                                current_user = Some(s32);
                                if first_valid.is_none() {
                                    first_valid = Some(s32);
                                }
                            }
                        }
                    }
                    pending_key = None;
                } else if depth == 2 && current_user.is_some() {
                    if matches!(tokens.get(idx + 1), Some(Tok::Open)) {
                        pending_key = None;
                    } else if let Some(key) = pending_key.take() {
                        if key.eq_ignore_ascii_case("MostRecent") && val.trim() == "1" {
                            return current_user;
                        }
                    } else {
                        pending_key = Some(val.clone());
                    }
                } else {
                    pending_key = None;
                }
            }
        }
        idx += 1;
    }

    first_valid
}

/// 3-tier Steam32 Account ID resolution chain:
/// 1. Explicit override (GUI/CLI/config)
/// 2. Live Dota 2 GSI `player.steamid`
/// 3. Local Steam `loginusers.vdf` (`"MostRecent" "1"`)
pub fn resolve_steam32_account_id(
    explicit_override: Option<u64>,
    gsi_steam32: Option<u64>,
    vdf_content: Option<&str>,
) -> Option<u64> {
    explicit_override
        .and_then(normalize_steam32_id)
        .or_else(|| gsi_steam32.and_then(normalize_steam32_id))
        .or_else(|| vdf_content.and_then(parse_loginusers_vdf))
}

fn config_file_path() -> PathBuf {
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join(".config/truesight-dota/config.toml")
}

pub fn load_account_config() -> AccountLinkConfig {
    if let Ok(env_val) = std::env::var("TRUESIGHT_STEAM32_ID") {
        let trimmed = env_val.trim();
        if trimmed.eq_ignore_ascii_case("none") || trimmed.eq_ignore_ascii_case("unlinked") {
            return AccountLinkConfig::Unlinked;
        }
        if let Ok(raw) = trimmed.parse::<u64>() {
            if let Some(id) = normalize_steam32_id(raw) {
                return AccountLinkConfig::Explicit(id);
            }
        }
    }

    let Ok(content) = fs::read_to_string(config_file_path()) else {
        return AccountLinkConfig::Auto;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == "account_id" {
                let cleaned = v.trim().trim_matches('"');
                if cleaned.eq_ignore_ascii_case("none") || cleaned.eq_ignore_ascii_case("unlinked") {
                    return AccountLinkConfig::Unlinked;
                }
                if cleaned.eq_ignore_ascii_case("auto") {
                    return AccountLinkConfig::Auto;
                }
                if let Ok(raw) = cleaned.parse::<u64>() {
                    if let Some(id) = normalize_steam32_id(raw) {
                        return AccountLinkConfig::Explicit(id);
                    }
                }
            }
        }
    }
    AccountLinkConfig::Auto
}

pub fn save_account_config(mode: AccountLinkConfig) -> Result<(), PortError> {
    let path = config_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| PortError(e.to_string()))?;
    }
    let val = match mode {
        AccountLinkConfig::Auto => "\"auto\"".to_string(),
        AccountLinkConfig::Unlinked => "\"none\"".to_string(),
        AccountLinkConfig::Explicit(id) => id.to_string(),
    };
    fs::write(path, format!("account_id = {val}\n")).map_err(|e| PortError(e.to_string()))
}

pub fn detect_local_steam_vdf() -> Option<u64> {
    let home = std::env::var("HOME").ok()?;
    let home_path = PathBuf::from(home);
    let candidates = [
        home_path.join(".local/share/Steam/config/loginusers.vdf"),
        home_path.join(".steam/steam/config/loginusers.vdf"),
    ];
    for candidate in candidates {
        if let Ok(content) = fs::read_to_string(candidate) {
            if let Some(steam32) = parse_loginusers_vdf(&content) {
                return Some(steam32);
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct LocalCacheManager {
    pub cache_dir: PathBuf,
    pub player_account_id: Option<u64>,
    pub explicit_override: bool,
    pub explicitly_unlinked: bool,
}

impl Default for LocalCacheManager {
    fn default() -> Self {
        Self::auto_detected()
    }
}

impl LocalCacheManager {
    pub fn new(player_account_id: u64) -> Self {
        let base = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        let cache_dir = base.join(".local/share/truesight-dota/cache");
        Self {
            cache_dir,
            player_account_id: normalize_steam32_id(player_account_id),
            explicit_override: true,
            explicitly_unlinked: false,
        }
    }

    pub fn auto_detected() -> Self {
        let base = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        let cache_dir = base.join(".local/share/truesight-dota/cache");
        match load_account_config() {
            AccountLinkConfig::Explicit(id) => Self {
                cache_dir,
                player_account_id: Some(id),
                explicit_override: true,
                explicitly_unlinked: false,
            },
            AccountLinkConfig::Unlinked => Self {
                cache_dir,
                player_account_id: None,
                explicit_override: true,
                explicitly_unlinked: true,
            },
            AccountLinkConfig::Auto => Self {
                cache_dir,
                player_account_id: detect_local_steam_vdf(),
                explicit_override: false,
                explicitly_unlinked: false,
            },
        }
    }

    pub fn with_dir(cache_dir: PathBuf, player_account_id: u64) -> Self {
        Self::with_optional_account(cache_dir, normalize_steam32_id(player_account_id))
    }

    pub fn with_optional_account(cache_dir: PathBuf, player_account_id: Option<u64>) -> Self {
        Self {
            cache_dir,
            player_account_id: player_account_id.and_then(normalize_steam32_id),
            explicit_override: false,
            explicitly_unlinked: false,
        }
    }

    pub fn set_account_id(&mut self, account_id: Option<u64>, explicit: bool) {
        self.player_account_id = account_id.and_then(normalize_steam32_id);
        self.explicit_override = explicit;
        self.explicitly_unlinked = explicit && self.player_account_id.is_none();
    }

    pub fn set_auto_account(&mut self) {
        self.explicit_override = false;
        self.explicitly_unlinked = false;
        self.player_account_id = detect_local_steam_vdf();
    }

    pub fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn metadata_path(&self) -> PathBuf {
        self.cache_dir.join("cache_metadata.txt")
    }

    fn meta_json_path(&self) -> PathBuf {
        self.cache_dir.join("opendota_hero_stats.json")
    }

    fn stratz_synergy_json_path(&self) -> PathBuf {
        self.cache_dir.join("stratz_synergy.json")
    }

    fn player_dir(&self) -> Option<PathBuf> {
        self.player_account_id
            .map(|id| self.cache_dir.join("players").join(id.to_string()))
    }

    fn player_metadata_path(&self) -> Option<PathBuf> {
        self.player_dir().map(|d| d.join("cache_metadata.txt"))
    }

    fn player_json_path(&self) -> Option<PathBuf> {
        self.player_dir().map(|d| d.join("player_heroes.json"))
    }

    fn player_matches_json_path(&self) -> Option<PathBuf> {
        self.player_dir().map(|d| d.join("player_matches.json"))
    }

    pub fn read_timestamps(&self) -> (Option<u64>, Option<u64>) {
        let mut meta_ts = None;
        if let Ok(content) = fs::read_to_string(self.metadata_path()) {
            for line in content.lines() {
                if let Some(v) = line.strip_prefix("meta_sync_unix=") {
                    meta_ts = v.trim().parse::<u64>().ok();
                }
            }
        }

        let mut player_ts = None;
        if let Some(p_meta_path) = self.player_metadata_path() {
            if let Ok(content) = fs::read_to_string(p_meta_path) {
                for line in content.lines() {
                    if let Some(v) = line.strip_prefix("player_sync_unix=") {
                        player_ts = v.trim().parse::<u64>().ok();
                    }
                }
            }
        }
        (meta_ts, player_ts)
    }

    pub fn is_meta_stale(&self, now_unix: u64) -> bool {
        let (meta_ts, _) = self.read_timestamps();
        match meta_ts {
            Some(ts) if self.meta_json_path().exists() => now_unix.saturating_sub(ts) > META_TTL_SECS,
            _ => true,
        }
    }

    pub fn is_player_stale(&self, now_unix: u64) -> bool {
        let Some(p_path) = self.player_json_path() else {
            return false;
        };
        let (_, player_ts) = self.read_timestamps();
        match player_ts {
            Some(ts) if p_path.exists() => now_unix.saturating_sub(ts) > PLAYER_TTL_SECS,
            _ => true,
        }
    }

    pub fn write_cached_files(
        &self,
        meta_json: Option<&str>,
        player_json: Option<&str>,
        now_unix: u64,
    ) -> Result<(), PortError> {
        fs::create_dir_all(&self.cache_dir).map_err(|e| PortError(e.to_string()))?;
        let (mut meta_ts, _) = self.read_timestamps();

        if let Some(m_body) = meta_json {
            fs::write(self.meta_json_path(), m_body).map_err(|e| PortError(e.to_string()))?;
            meta_ts = Some(now_unix);
        }
        if let Some(mts) = meta_ts {
            fs::write(self.metadata_path(), format!("meta_sync_unix={mts}\n"))
                .map_err(|e| PortError(e.to_string()))?;
        }

        if let Some(p_body) = player_json {
            if let (Some(p_dir), Some(p_json_path), Some(p_meta_path)) = (
                self.player_dir(),
                self.player_json_path(),
                self.player_metadata_path(),
            ) {
                fs::create_dir_all(&p_dir).map_err(|e| PortError(e.to_string()))?;
                fs::write(p_json_path, p_body).map_err(|e| PortError(e.to_string()))?;
                fs::write(p_meta_path, format!("player_sync_unix={now_unix}\n"))
                    .map_err(|e| PortError(e.to_string()))?;
            }
        }

        Ok(())
    }

    /// Synchronizes stale or forced endpoints from OpenDota (and optional STRATZ token) with a 6s timeout.
    pub fn sync_from_opendota(&self, force: bool) -> Result<String, PortError> {
        let now = Self::now_unix();
        let need_meta = force || self.is_meta_stale(now);
        let need_player = self.player_account_id.is_some() && (force || self.is_player_stale(now));

        if !need_meta && !need_player {
            return Ok("DB cache is already fresh (Meta <24h, Player <1h).".into());
        }

        let mut fetched_meta = None;
        let mut fetched_player = None;
        let mut updated_parts = Vec::new();

        if need_meta {
            let body = fetch_url("https://api.opendota.com/api/heroStats")?;
            if body.trim_start().starts_with('[') {
                fetched_meta = Some(body);
                updated_parts.push("heroStats (24h TTL)");
            }
            if let Ok(token) = std::env::var("STRATZ_API_TOKEN") {
                if !token.trim().is_empty() {
                    if let Ok(s_body) = fetch_stratz_with_synergy_graphql(token.trim()) {
                        let _ = fs::create_dir_all(&self.cache_dir);
                        let _ = fs::write(self.stratz_synergy_json_path(), s_body);
                        updated_parts.push("STRATZ synergy");
                    }
                }
            }
        }

        if need_player {
            if let Some(account_id) = self.player_account_id {
                let url = format!("https://api.opendota.com/api/players/{account_id}/heroes");
                let body = fetch_url(&url)?;
                if body.trim_start().starts_with('[') {
                    fetched_player = Some(body);
                    updated_parts.push("player comfort (1h TTL)");
                }
                let matches_url =
                    format!("https://api.opendota.com/api/players/{account_id}/matches?limit=500");
                if let Ok(m_body) = fetch_url(&matches_url) {
                    if m_body.trim_start().starts_with('[') {
                        if let (Some(p_dir), Some(m_path)) =
                            (self.player_dir(), self.player_matches_json_path())
                        {
                            let _ = fs::create_dir_all(p_dir);
                            let _ = fs::write(m_path, m_body);
                        }
                    }
                }
            }
        }

        self.write_cached_files(fetched_meta.as_deref(), fetched_player.as_deref(), now)?;

        if updated_parts.is_empty() {
            Ok("Synced OpenDota cache.".into())
        } else {
            Ok(format!("Synced OpenDota: {}.", updated_parts.join(" + ")))
        }
    }

    pub fn freshness_badge(&self, now_unix: u64) -> String {
        let (meta_ts, player_ts) = self.read_timestamps();
        let meta_str = meta_ts
            .map(|ts| format_age(now_unix.saturating_sub(ts)))
            .unwrap_or_else(|| "embedded".into());
        let player_part = match self.player_account_id {
            Some(id) => {
                let p_age = player_ts
                    .map(|ts| format_age(now_unix.saturating_sub(ts)))
                    .unwrap_or_else(|| "embedded".into());
                format!("Player #{id}: {p_age}")
            }
            None => "Player: unlinked".into(),
        };
        format!("DB [Meta: {meta_str} | {player_part}]")
    }

    pub fn build_repository(&self) -> SeededMetaRepository {
        let mut repo = SeededMetaRepository::new();
        if let Ok(m_json) = fs::read_to_string(self.meta_json_path()) {
            repo.custom_hero_meta = parse_hero_stats_json(&m_json);
        }
        if let Some(p_path) = self.player_json_path() {
            if let Ok(p_json) = fs::read_to_string(p_path) {
                let matches_json = self
                    .player_matches_json_path()
                    .and_then(|mp| fs::read_to_string(mp).ok());
                repo.custom_personal_pool =
                    parse_player_comfort_with_matches_json(&p_json, matches_json.as_deref());
            }
        }
        if let Ok(s_json) = fs::read_to_string(self.stratz_synergy_json_path()) {
            repo.custom_pair_synergy = parse_stratz_with_synergy_json(&s_json);
        }
        repo
    }
}

/// Lightweight parser for OpenDota `/api/heroStats` JSON array.
/// Returns `Vec<(HeroId, bracket_winrates[8], trend_delta)>`.
pub fn parse_hero_stats_json(json: &str) -> Vec<(HeroId, [f32; 8], f32)> {
    let mut rows = Vec::new();
    for obj_chunk in json.split('{').skip(1) {
        let Some(end) = obj_chunk.find('}') else {
            continue;
        };
        let block = &obj_chunk[..end];
        let Some(hid) = extract_u64_field(block, "\"id\"") else {
            continue;
        };
        let mut wrs = [0.50_f32; 8];
        let mut has_any = false;
        for t in 1..=7 {
            let pk = format!("\"{t}_pick\"");
            let wk = format!("\"{t}_win\"");
            if let (Some(p), Some(w)) = (extract_u64_field(block, &pk), extract_u64_field(block, &wk))
            {
                if p > 0 {
                    wrs[t - 1] = ((w as f32 + 25.0 * 0.50) / (p as f32 + 25.0)).clamp(0.35, 0.65);
                    has_any = true;
                }
            }
        }
        if !has_any {
            continue;
        }
        wrs[7] = (wrs[6] + 0.35 * (wrs[6] - wrs[4])).clamp(0.38, 0.60);

        let pt = extract_u64_array(block, "\"pub_pick_trend\"");
        let wt = extract_u64_array(block, "\"pub_win_trend\"");
        let trend_delta = if pt.len() >= 6 && wt.len() >= 6 {
            let p_sum: u64 = pt.iter().sum();
            let w_sum: u64 = wt.iter().sum();
            if p_sum > 0 {
                let week_wr = (w_sum as f32) / (p_sum as f32);
                let rec_p = (pt[pt.len() - 3] + pt[pt.len() - 2]) as f32;
                let rec_w = (wt[wt.len() - 3] + wt[wt.len() - 2]) as f32;
                let rec_wr = if rec_p > 0.0 {
                    (rec_w + 200.0 * week_wr) / (rec_p + 200.0)
                } else {
                    week_wr
                };
                (rec_wr - week_wr).clamp(-0.025, 0.025)
            } else {
                0.0
            }
        } else {
            0.0
        };

        rows.push((HeroId(hid as u16), wrs, trend_delta));
    }
    rows
}

fn extract_u64_array(block: &str, key: &str) -> Vec<u64> {
    let Some(pos) = block.find(key) else {
        return Vec::new();
    };
    let after = &block[pos + key.len()..];
    let Some(open) = after.find('[') else {
        return Vec::new();
    };
    let Some(close) = after[open..].find(']') else {
        return Vec::new();
    };
    let inside = &after[open + 1..open + close];
    inside
        .split(',')
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .collect()
}

fn format_age(age_secs: u64) -> String {
    if age_secs < 60 {
        format!("{age_secs}s ago")
    } else if age_secs < 3600 {
        format!("{}m ago", age_secs / 60)
    } else if age_secs < 86_400 {
        format!("{}h ago", age_secs / 3600)
    } else {
        format!("{}d ago", age_secs / 86_400)
    }
}

fn fetch_url(url: &str) -> Result<String, PortError> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "6", url])
        .output()
        .map_err(|e| PortError(format!("Failed to execute curl: {e}")))?;
    if !out.status.success() {
        return Err(PortError(format!(
            "HTTP fetch failed for {url} (status {:?})",
            out.status.code()
        )));
    }
    String::from_utf8(out.stdout).map_err(|e| PortError(e.to_string()))
}

/// Lightweight parser for OpenDota `/api/players/{id}/heroes` JSON array.
/// Returns `Vec<(HeroId, games, wins, comfort_decay)>`.
pub fn parse_player_comfort_json(json: &str) -> Vec<(HeroId, u32, u32, f32)> {
    parse_player_comfort_with_matches_json(json, None)
}

/// Parses OpenDota `/api/players/{id}/heroes` combined with optional `/api/players/{id}/matches`
/// to compute per-match exponentially time-decayed effective games and wins (`tau = 75d`, floor `0.0`).
pub fn parse_player_comfort_with_matches_json(
    heroes_json: &str,
    matches_json: Option<&str>,
) -> Vec<(HeroId, u32, u32, f32)> {
    const TAU_DAYS: f32 = 75.0;

    let mut raw_rows: Vec<(u16, u64, u32, u32)> = Vec::new();
    let mut max_ts = 0_u64;

    for obj_chunk in heroes_json.split('{').skip(1) {
        let Some(end) = obj_chunk.find('}') else {
            continue;
        };
        let block = &obj_chunk[..end];
        let Some(hid) = extract_u64_field(block, "\"hero_id\"") else {
            continue;
        };
        let last_played = extract_u64_field(block, "\"last_played\"").unwrap_or(0);
        let games = extract_u64_field(block, "\"games\"").unwrap_or(0) as u32;
        let wins = extract_u64_field(block, "\"win\"").unwrap_or(0) as u32;
        if games > 0 && last_played > max_ts {
            max_ts = last_played;
        }
        raw_rows.push((hid as u16, last_played, games, wins));
    }

    // Optional per-match recency accumulator: (g_eff, w_eff, match_cnt) indexed by hero_id
    let mut match_acc = [(0.0_f32, 0.0_f32, 0_u32); truesight_core::domain::MAX_HEROES];
    let mut has_matches = false;

    if let Some(m_json) = matches_json {
        for obj_chunk in m_json.split('{').skip(1) {
            let Some(end) = obj_chunk.find('}') else {
                continue;
            };
            let block = &obj_chunk[..end];
            if let Some(st) = extract_u64_field(block, "\"start_time\"") {
                if st > max_ts {
                    max_ts = st;
                }
            }
        }
        for obj_chunk in m_json.split('{').skip(1) {
            let Some(end) = obj_chunk.find('}') else {
                continue;
            };
            let block = &obj_chunk[..end];
            let Some(hid) = extract_u64_field(block, "\"hero_id\"") else {
                continue;
            };
            let idx = hid as usize;
            if idx >= truesight_core::domain::MAX_HEROES {
                continue;
            }
            let Some(st) = extract_u64_field(block, "\"start_time\"") else {
                continue;
            };
            let slot = extract_u64_field(block, "\"player_slot\"").unwrap_or(0);
            let rad_win = extract_bool_field(block, "\"radiant_win\"").unwrap_or(false);
            let won = (slot < 128) == rad_win;
            let age_days = (max_ts.saturating_sub(st)) as f32 / 86_400.0;
            let w = (-age_days / TAU_DAYS).exp();
            match_acc[idx].0 += w;
            if won {
                match_acc[idx].1 += w;
            }
            match_acc[idx].2 += 1;
            has_matches = true;
        }
    }

    raw_rows
        .into_iter()
        .map(|(hid, last_played, games, wins)| {
            if games == 0 {
                return (HeroId(hid), 0, 0, 0.0);
            }
            let idx = hid as usize;
            let (g_eff, w_eff, cnt) = if has_matches && idx < truesight_core::domain::MAX_HEROES {
                match_acc[idx]
            } else {
                (0.0, 0.0, 0)
            };

            if cnt > 0 {
                let decay = (g_eff / (games as f32)).clamp(0.0, 1.0);
                let career_wr = (wins as f32) / (games as f32);
                let rec_wr = if g_eff > 0.01 { w_eff / g_eff } else { career_wr };
                let blended_wr = 0.65 * rec_wr + 0.35 * career_wr;
                let eff_wins = ((games as f32) * blended_wr).round() as u32;
                (HeroId(hid), games, eff_wins, decay)
            } else {
                let decay = if last_played > 0 && max_ts > 0 {
                    let age_days = (max_ts.saturating_sub(last_played)) as f32 / 86_400.0;
                    (-age_days / TAU_DAYS).exp().clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (HeroId(hid), games, wins, decay)
            }
        })
        .collect()
}

fn extract_bool_field(block: &str, key: &str) -> Option<bool> {
    let pos = block.find(key)?;
    let after_key = &block[pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    if after_colon.starts_with("true") {
        Some(true)
    } else if after_colon.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn extract_u64_field(block: &str, key: &str) -> Option<u64> {
    let pos = block.find(key)?;
    let after_key = &block[pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    let digits: String = after_colon
        .chars()
        .skip_while(|c| *c == '"')
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u64>().ok()
}

fn extract_f32_field(block: &str, key: &str) -> Option<f32> {
    let pos = block.find(key)?;
    let after_key = &block[pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    let token: String = after_colon
        .chars()
        .skip_while(|c| *c == '"')
        .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '+' || *c == '.')
        .collect();
    token.parse::<f32>().ok()
}

/// Parses STRATZ empirical `with` synergy JSON rows (`heroId1`, `heroId2`, `synergy` / `wins`+`matchCount`).
pub fn parse_stratz_with_synergy_json(json: &str) -> Vec<(HeroId, HeroId, f32, u32)> {
    let mut rows = Vec::new();
    for obj_chunk in json.split('{').skip(1) {
        let Some(end) = obj_chunk.find('}') else {
            continue;
        };
        let block = &obj_chunk[..end];
        let h1 = extract_u64_field(block, "\"heroId1\"")
            .or_else(|| extract_u64_field(block, "\"hero_id_1\""));
        let h2 = extract_u64_field(block, "\"heroId2\"")
            .or_else(|| extract_u64_field(block, "\"hero_id_2\""));
        let (Some(a), Some(b)) = (h1, h2) else {
            continue;
        };
        if a == 0 || b == 0 || a == b {
            continue;
        }
        let matches = extract_u64_field(block, "\"matchCount\"")
            .or_else(|| extract_u64_field(block, "\"matches\""))
            .unwrap_or(400) as u32;
        let syn = if let Some(s) = extract_f32_field(block, "\"synergy\"")
            .or_else(|| extract_f32_field(block, "\"with_synergy\""))
        {
            s.clamp(-0.15, 0.15)
        } else if let Some(wins) = extract_u64_field(block, "\"wins\"") {
            if matches > 0 {
                let shrunk = (wins as f32 + 50.0 * 0.50) / (matches as f32 + 50.0);
                (shrunk - 0.50).clamp(-0.15, 0.15)
            } else {
                continue;
            }
        } else {
            continue;
        };
        rows.push((HeroId(a as u16), HeroId(b as u16), syn, matches));
    }
    rows
}

fn fetch_stratz_with_synergy_graphql(token: &str) -> Result<String, PortError> {
    let query = r#"{"query":"{ heroStats { heroVsHeroMatchup { advantage { heroId1 heroId2 with { heroId1 heroId2 matchCount synergy } } } } }"}"#;
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "6",
            "-H",
            "Content-Type: application/json",
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-d",
            query,
            "https://api.stratz.com/graphql",
        ])
        .output()
        .map_err(|e| PortError(format!("Failed to execute STRATZ curl: {e}")))?;
    if !out.status.success() {
        return Err(PortError(format!(
            "STRATZ GraphQL fetch failed (status {:?})",
            out.status.code()
        )));
    }
    String::from_utf8(out.stdout).map_err(|e| PortError(e.to_string()))
}
