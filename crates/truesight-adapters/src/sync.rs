use crate::item_repo::SeededItemMetaRepository;
use crate::meta_repo::SeededMetaRepository;
use crate::platform::{
    PlatformEnv, detect_steam_vdf_in_candidates, resolve_cache_dir_for,
    resolve_config_file_path_for, run_curl, steam_vdf_candidates_for,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use truesight_core::domain::{
    Bracket, HeroId, HeroItemStat, ItemCatalogEntry, ItemId, ItemPhase, Role,
};
use truesight_core::ports::{ItemMetaRepository, PortError};

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

/// Persisted bracket selection policy stored in `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BracketConfig {
    /// Follow OpenDota `/api/players/{id}` `rank_tier` when linked, defaulting to `Legend`.
    #[default]
    Auto,
    /// Preserve the user's explicit medal selection across restarts and account syncs.
    Explicit(Bracket),
}

/// Unified user configuration loaded from and saved to `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppConfig {
    /// Persisted Steam account linking mode.
    pub account_link: AccountLinkConfig,
    /// Persisted medal bracket selection mode.
    pub bracket: BracketConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            account_link: AccountLinkConfig::Auto,
            bracket: BracketConfig::Auto,
        }
    }
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

pub fn config_file_path() -> PathBuf {
    resolve_config_file_path_for(std::env::consts::OS, &PlatformEnv::from_process())
}

/// Loads unified `AppConfig` (`account_id` + `bracket`) from an explicit `config.toml` path.
pub fn load_app_config_from(path: &Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    let Ok(content) = fs::read_to_string(path) else {
        return cfg;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let cleaned = v.trim().trim_matches('"');
        match key {
            "account_id" => {
                if cleaned.eq_ignore_ascii_case("none") || cleaned.eq_ignore_ascii_case("unlinked")
                {
                    cfg.account_link = AccountLinkConfig::Unlinked;
                } else if cleaned.eq_ignore_ascii_case("auto") {
                    cfg.account_link = AccountLinkConfig::Auto;
                } else if let Ok(raw) = cleaned.parse::<u64>() {
                    if let Some(id) = normalize_steam32_id(raw) {
                        cfg.account_link = AccountLinkConfig::Explicit(id);
                    }
                }
            }
            "bracket" => {
                if cleaned.eq_ignore_ascii_case("auto") {
                    cfg.bracket = BracketConfig::Auto;
                } else if let Some(b) = Bracket::parse_label(cleaned) {
                    cfg.bracket = BracketConfig::Explicit(b);
                }
            }
            _ => {}
        }
    }
    cfg
}

/// Loads unified `AppConfig` from the OS `config.toml`, honoring `TRUESIGHT_STEAM32_ID`.
pub fn load_app_config() -> AppConfig {
    let mut cfg = load_app_config_from(&config_file_path());
    if let Ok(env_val) = std::env::var("TRUESIGHT_STEAM32_ID") {
        let trimmed = env_val.trim();
        if trimmed.eq_ignore_ascii_case("none") || trimmed.eq_ignore_ascii_case("unlinked") {
            cfg.account_link = AccountLinkConfig::Unlinked;
        } else if let Ok(raw) = trimmed.parse::<u64>() {
            if let Some(id) = normalize_steam32_id(raw) {
                cfg.account_link = AccountLinkConfig::Explicit(id);
            }
        }
    }
    cfg
}

pub fn load_account_config() -> AccountLinkConfig {
    load_app_config().account_link
}

fn save_app_config_to(path: &Path, cfg: AppConfig) -> Result<(), PortError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| PortError(e.to_string()))?;
    }
    let acct_val = match cfg.account_link {
        AccountLinkConfig::Auto => "\"auto\"".to_string(),
        AccountLinkConfig::Unlinked => "\"none\"".to_string(),
        AccountLinkConfig::Explicit(id) => id.to_string(),
    };
    let bracket_val = match cfg.bracket {
        BracketConfig::Auto => "auto".to_string(),
        BracketConfig::Explicit(b) => b.label().to_ascii_lowercase(),
    };
    let body = format!("account_id = {acct_val}\nbracket = \"{bracket_val}\"\n");
    atomic_write_file(path, &body)
}

/// Updates `account_id` in `path` while preserving any persisted `bracket` selection.
pub fn save_account_config_to(path: &Path, mode: AccountLinkConfig) -> Result<(), PortError> {
    let mut cfg = load_app_config_from(path);
    cfg.account_link = mode;
    save_app_config_to(path, cfg)
}

pub fn save_account_config(mode: AccountLinkConfig) -> Result<(), PortError> {
    save_account_config_to(&config_file_path(), mode)
}

/// Updates `bracket` in `path` while preserving any persisted `account_id` setting.
pub fn save_bracket_config_to(path: &Path, mode: BracketConfig) -> Result<(), PortError> {
    let mut cfg = load_app_config_from(path);
    cfg.bracket = mode;
    save_app_config_to(path, cfg)
}

/// Updates `bracket` in the default OS `config.toml` while preserving `account_id`.
pub fn save_bracket_config(mode: BracketConfig) -> Result<(), PortError> {
    save_bracket_config_to(&config_file_path(), mode)
}

pub fn detect_local_steam_vdf() -> Option<u64> {
    let env = PlatformEnv::from_process();
    let candidates = steam_vdf_candidates_for(std::env::consts::OS, &env, None);
    if let Some(steam32) = detect_steam_vdf_in_candidates(&candidates) {
        return Some(steam32);
    }
    #[cfg(target_os = "windows")]
    if let Some(reg_path) = crate::platform::query_windows_steam_path() {
        return detect_steam_vdf_in_candidates(&[reg_path.join("config").join("loginusers.vdf")]);
    }
    None
}

#[derive(Debug, Default)]
struct CacheSyncState {
    latest_started_gen: u64,
    latest_committed_meta_gen: u64,
}

type CacheCoordinatorEntry = (PathBuf, Arc<Mutex<CacheSyncState>>);

fn coordinator_for_dir(dir: &Path) -> Arc<Mutex<CacheSyncState>> {
    static REGISTRY: OnceLock<Mutex<Vec<CacheCoordinatorEntry>>> = OnceLock::new();
    let reg = REGISTRY.get_or_init(|| Mutex::new(Vec::new()));
    let mut list = reg.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, coord)) = list.iter().find(|(p, _)| p == dir) {
        return Arc::clone(coord);
    }
    let coord = Arc::new(Mutex::new(CacheSyncState::default()));
    list.push((dir.to_path_buf(), Arc::clone(&coord)));
    coord
}

fn atomic_write_file(path: &Path, content: &str) -> Result<(), PortError> {
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("cache");
    let tmp = path.with_file_name(format!(".{name}.tmp.{}.{seq}", std::process::id()));
    let res = fs::write(&tmp, content).and_then(|()| fs::rename(&tmp, path));
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map_err(|e| PortError(e.to_string()))
}

#[derive(Debug, Clone)]
pub struct LocalCacheManager {
    pub cache_dir: PathBuf,
    pub player_account_id: Option<u64>,
    pub explicit_override: bool,
    pub explicitly_unlinked: bool,
    pub stratz_api_token: Option<String>,
    pub stratz_token_exp: Option<u64>,
    pub target_item_hero_ids: Vec<HeroId>,
    pub target_item_hero_roles: Vec<(HeroId, Role)>,
    sync_generation: u64,
    cancel_token: Arc<AtomicBool>,
    coordinator: Arc<Mutex<CacheSyncState>>,
}

impl Default for LocalCacheManager {
    fn default() -> Self {
        Self::auto_detected()
    }
}

impl LocalCacheManager {
    fn default_cache_dir() -> PathBuf {
        resolve_cache_dir_for(std::env::consts::OS, &PlatformEnv::from_process())
    }

    pub fn new(player_account_id: u64) -> Self {
        Self {
            explicit_override: true,
            ..Self::with_optional_account(
                Self::default_cache_dir(),
                normalize_steam32_id(player_account_id),
            )
        }
    }

    pub fn auto_detected() -> Self {
        let dir = Self::default_cache_dir();
        match load_account_config() {
            AccountLinkConfig::Explicit(id) => Self {
                explicit_override: true,
                ..Self::with_optional_account(dir, Some(id))
            },
            AccountLinkConfig::Unlinked => Self {
                explicit_override: true,
                explicitly_unlinked: true,
                ..Self::with_optional_account(dir, None)
            },
            AccountLinkConfig::Auto => Self::with_optional_account(dir, detect_local_steam_vdf()),
        }
    }

    pub fn with_dir(cache_dir: PathBuf, player_account_id: u64) -> Self {
        Self::with_optional_account(cache_dir, normalize_steam32_id(player_account_id))
    }

    pub fn with_optional_account(cache_dir: PathBuf, player_account_id: Option<u64>) -> Self {
        let coordinator = coordinator_for_dir(&cache_dir);
        Self {
            cache_dir,
            player_account_id: player_account_id.and_then(normalize_steam32_id),
            explicit_override: false,
            explicitly_unlinked: false,
            stratz_api_token: None,
            stratz_token_exp: None,
            target_item_hero_ids: Vec::new(),
            target_item_hero_roles: Vec::new(),
            sync_generation: 0,
            cancel_token: Arc::new(AtomicBool::new(false)),
            coordinator,
        }
    }

    /// Resolves the active STRATZ API bearer token against the current wall-clock time,
    /// giving precedence to `STRATZ_API_TOKEN` and rejecting expired persisted tokens.
    pub fn effective_stratz_token(&self) -> Option<String> {
        self.effective_stratz_token_at(Self::now_unix())
    }

    /// Resolves the active STRATZ API bearer token at `now_unix_secs`.
    pub fn effective_stratz_token_at(&self, now_unix_secs: u64) -> Option<String> {
        let env_val = std::env::var("STRATZ_API_TOKEN").ok();
        self.effective_stratz_token_with_env_at(env_val.as_deref(), now_unix_secs)
    }

    /// Resolves the active STRATZ API bearer token at `now_unix_secs` with an explicit
    /// environment override value (used by `effective_stratz_token_at` and deterministic tests).
    pub fn effective_stratz_token_with_env_at(
        &self,
        env_token: Option<&str>,
        now_unix_secs: u64,
    ) -> Option<String> {
        if let Some(env_tok) = env_token.map(str::trim).filter(|t| !t.is_empty()) {
            return Some(env_tok.to_owned());
        }
        let trimmed = self
            .stratz_api_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())?;
        let exp = self
            .stratz_token_exp
            .or_else(|| extract_jwt_exp_unverified(trimmed));
        if exp.is_some_and(|exp_ts| exp_ts <= now_unix_secs) {
            return None;
        }
        Some(trimmed.to_owned())
    }

    pub fn begin_sync_generation(&mut self) -> (u64, Arc<AtomicBool>) {
        self.cancel_token.store(true, Ordering::Release);
        let mut state = self.coordinator.lock().unwrap_or_else(|e| e.into_inner());
        state.latest_started_gen = state.latest_started_gen.saturating_add(1);
        self.sync_generation = state.latest_started_gen;
        self.cancel_token = Arc::new(AtomicBool::new(false));
        (self.sync_generation, Arc::clone(&self.cancel_token))
    }

    pub fn cancel_in_flight(&mut self) -> u64 {
        self.begin_sync_generation().0
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.load(Ordering::Acquire)
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

    /// Configures the target hero IDs whose item builds should be synchronized.
    /// Duplicates and invalid zero IDs are removed while preserving order.
    pub fn set_target_item_heroes(&mut self, heroes: Vec<HeroId>) {
        self.target_item_hero_ids = deduplicate_hero_ids(heroes);
        self.target_item_hero_roles.clear();
    }

    /// Configures target `(hero, role)` pairs for role-aware STRATZ bracketed item syncs while
    /// preserving deduplicated `target_item_hero_ids` in lockstep.
    pub fn set_target_item_hero_roles(&mut self, hero_roles: Vec<(HeroId, Role)>) {
        let mut dedup_roles = Vec::new();
        let mut dedup_ids = Vec::new();
        for (hid, role) in hero_roles {
            if hid.0 == 0 {
                continue;
            }
            if !dedup_roles.contains(&(hid, role)) {
                dedup_roles.push((hid, role));
            }
            if !dedup_ids.contains(&hid) {
                dedup_ids.push(hid);
            }
        }
        self.target_item_hero_roles = dedup_roles;
        self.target_item_hero_ids = dedup_ids;
    }

    /// Returns the deduplicated list of hero IDs targeted by item-build sync,
    /// defaulting to `[HeroId(1)]` when no explicit target heroes are configured.
    pub fn effective_target_item_heroes(&self) -> Vec<HeroId> {
        let deduped = deduplicate_hero_ids(self.target_item_hero_ids.clone());
        if deduped.is_empty() {
            vec![HeroId(1)]
        } else {
            deduped
        }
    }

    pub(crate) fn effective_target_item_hero_roles(&self) -> Vec<(HeroId, Option<Role>)> {
        if !self.target_item_hero_roles.is_empty() {
            self.target_item_hero_roles
                .iter()
                .map(|&(hid, role)| (hid, Some(role)))
                .collect()
        } else {
            self.effective_target_item_heroes()
                .into_iter()
                .map(|hid| (hid, None))
                .collect()
        }
    }

    /// Returns `true` when any explicit target item hero/role lacks cached item build stats.
    pub fn is_target_item_heroes_uncached(&self) -> bool {
        if !self.target_item_hero_roles.is_empty() {
            let Ok(existing) = fs::read_to_string(self.hero_item_stats_json_path()) else {
                return true;
            };
            let parsed =
                parse_cached_hero_item_stats_entries(&existing, &SeededItemMetaRepository::new());
            let has_token = self.effective_stratz_token().is_some();
            return self.target_item_hero_roles.iter().any(|&(hid, role)| {
                !is_hero_role_covered_in_keys(
                    parsed.iter().map(|(c_hid, c_role, c_bucket, _)| {
                        (
                            c_hid.0,
                            c_role.map(role_to_cache_idx),
                            c_bucket.map(StratzBracketBucket::index),
                        )
                    }),
                    hid,
                    Some(role),
                    has_token,
                )
            });
        }

        let configured = deduplicate_hero_ids(self.target_item_hero_ids.clone());
        if configured.is_empty() {
            return false;
        }
        let Ok(existing) = fs::read_to_string(self.hero_item_stats_json_path()) else {
            return true;
        };
        let grouped =
            parse_cached_hero_item_stats_entries(&existing, &SeededItemMetaRepository::new());
        configured.iter().any(|&hid| {
            !is_hero_role_covered_in_keys(
                grouped.iter().map(|(c_hid, c_role, c_bucket, _)| {
                    (
                        c_hid.0,
                        c_role.map(role_to_cache_idx),
                        c_bucket.map(StratzBracketBucket::index),
                    )
                }),
                hid,
                None,
                false,
            )
        })
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

    pub(crate) fn item_constants_json_path(&self) -> PathBuf {
        self.cache_dir.join("item_constants.json")
    }

    pub(crate) fn hero_item_stats_json_path(&self) -> PathBuf {
        self.cache_dir.join("hero_item_stats.json")
    }

    fn game_patch_path(&self) -> PathBuf {
        self.cache_dir.join("game_patch.txt")
    }

    /// Reads the cached Dota 2 game patch version (e.g. `"7.40b"`), if persisted.
    pub fn read_cached_game_patch(&self) -> Option<String> {
        let content = fs::read_to_string(self.game_patch_path()).ok()?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    /// Persists the active Dota 2 game patch string atomically into the local cache.
    pub fn write_cached_game_patch(&self, patch: &str) -> Result<(), PortError> {
        let trimmed = patch.trim();
        if trimmed.is_empty() {
            return Err(PortError("Game patch string must not be empty".into()));
        }
        fs::create_dir_all(&self.cache_dir).map_err(|e| PortError(e.to_string()))?;
        atomic_write_file(&self.game_patch_path(), trimmed)
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

    fn player_profile_json_path(&self) -> Option<PathBuf> {
        self.player_dir().map(|d| d.join("player_profile.json"))
    }

    /// Reads the linked player's cached OpenDota profile (`player_profile.json`) and maps
    /// `rank_tier` to a `Bracket`, returning `None` when absent or uncalibrated.
    pub fn read_cached_player_bracket(&self) -> Option<Bracket> {
        let p_path = self.player_profile_json_path()?;
        let content = fs::read_to_string(p_path).ok()?;
        parse_player_rank_tier_json(&content)
    }

    /// Persists an OpenDota `/api/players/{id}` JSON object into the linked player's cache.
    pub fn write_cached_profile_json(&self, profile_json: &str) -> Result<(), PortError> {
        let trimmed = profile_json.trim();
        if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
            return Err(PortError(
                "Invalid or truncated player profile JSON payload".into(),
            ));
        }
        let Some(p_dir) = self.player_dir() else {
            return Err(PortError(
                "Cannot cache player profile without a linked account".into(),
            ));
        };
        let Some(prof_path) = self.player_profile_json_path() else {
            return Err(PortError(
                "Cannot resolve player profile cache path without a linked account".into(),
            ));
        };
        fs::create_dir_all(&p_dir).map_err(|e| PortError(e.to_string()))?;
        atomic_write_file(&prof_path, profile_json)
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
            Some(ts)
                if self.meta_json_path().exists() && self.item_constants_json_path().exists() =>
            {
                now_unix.saturating_sub(ts) > META_TTL_SECS
            }
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

    pub(crate) fn publish_staged_cache(
        &self,
        meta_json: Option<&str>,
        stratz_json: Option<&str>,
        player_json: Option<&str>,
        player_matches_json: Option<&str>,
        player_profile_json: Option<&str>,
        now_unix: u64,
    ) -> Result<(), PortError> {
        self.publish_staged_cache_with_items(
            meta_json,
            stratz_json,
            None,
            None,
            player_json,
            player_matches_json,
            player_profile_json,
            now_unix,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_staged_cache_with_items(
        &self,
        meta_json: Option<&str>,
        stratz_json: Option<&str>,
        item_constants_json: Option<&str>,
        hero_item_stats_json: Option<&str>,
        player_json: Option<&str>,
        player_matches_json: Option<&str>,
        player_profile_json: Option<&str>,
        now_unix: u64,
    ) -> Result<(), PortError> {
        let is_arr = |s: &str| {
            let t = s.trim();
            t.starts_with('[') && t.ends_with(']')
        };
        let is_obj = |s: &str| {
            let t = s.trim();
            t.starts_with('{') && t.ends_with('}')
        };
        if let Some(m) = meta_json.filter(|m| !is_arr(m) || parse_hero_stats_json(m).is_empty()) {
            let _ = m;
            return Err(PortError(
                "Invalid or truncated heroStats JSON payload".into(),
            ));
        }
        if stratz_json.is_some_and(|s| parse_stratz_with_synergy_json(s).is_empty()) {
            return Err(PortError(
                "Invalid or truncated STRATZ synergy JSON payload".into(),
            ));
        }
        let parsed_item_catalog = if let Some(ic) = item_constants_json {
            let parsed = parse_opendota_item_constants_json(ic);
            if parsed.is_empty() {
                return Err(PortError(
                    "Invalid or truncated item constants JSON payload".into(),
                ));
            }
            Some(parsed)
        } else {
            None
        };
        if let Some(his) = hero_item_stats_json {
            let mut validation_repo = SeededItemMetaRepository::new();
            if let Some(ref catalog) = parsed_item_catalog {
                validation_repo.custom_catalog = catalog.clone();
            } else if let Ok(existing_ic) = fs::read_to_string(self.item_constants_json_path()) {
                let parsed = parse_opendota_item_constants_json(&existing_ic);
                if !parsed.is_empty() {
                    validation_repo.custom_catalog = parsed;
                }
            }
            if parse_cached_hero_item_stats_entries(his, &validation_repo).is_empty() {
                return Err(PortError(
                    "Invalid or truncated hero item stats JSON payload".into(),
                ));
            }
        }
        if player_json.is_some_and(|p| {
            self.player_account_id.is_none()
                || !is_arr(p)
                || (p.trim() != "[]" && parse_player_comfort_json(p).is_empty())
        }) {
            return Err(PortError(
                "Invalid or truncated player comfort JSON payload".into(),
            ));
        }
        if player_matches_json.is_some_and(|pm| self.player_account_id.is_none() || !is_arr(pm)) {
            return Err(PortError(
                "Invalid or truncated player matches JSON payload".into(),
            ));
        }
        if player_profile_json.is_some_and(|pp| self.player_account_id.is_none() || !is_obj(pp)) {
            return Err(PortError(
                "Invalid or truncated player profile JSON payload".into(),
            ));
        }

        let mut state = self.coordinator.lock().unwrap_or_else(|e| e.into_inner());
        let superseded = self.sync_generation > 0
            && (self.sync_generation < state.latest_started_gen
                || self.sync_generation < state.latest_committed_meta_gen);
        if self.is_cancelled() || superseded {
            return Err(PortError("Sync superseded by newer generation".into()));
        }

        if meta_json.is_some()
            || stratz_json.is_some()
            || item_constants_json.is_some()
            || hero_item_stats_json.is_some()
        {
            fs::create_dir_all(&self.cache_dir).map_err(|e| PortError(e.to_string()))?;
        }
        if let Some(m_body) = meta_json {
            atomic_write_file(&self.meta_json_path(), m_body)?;
            atomic_write_file(
                &self.metadata_path(),
                &format!("meta_sync_unix={now_unix}\n"),
            )?;
            state.latest_committed_meta_gen =
                state.latest_committed_meta_gen.max(self.sync_generation);
        }
        if let Some(s_body) = stratz_json {
            atomic_write_file(&self.stratz_synergy_json_path(), s_body)?;
            state.latest_committed_meta_gen =
                state.latest_committed_meta_gen.max(self.sync_generation);
        }
        if let Some(ic_body) = item_constants_json {
            atomic_write_file(&self.item_constants_json_path(), ic_body)?;
            if meta_json.is_none() {
                atomic_write_file(
                    &self.metadata_path(),
                    &format!("meta_sync_unix={now_unix}\n"),
                )?;
            }
            state.latest_committed_meta_gen =
                state.latest_committed_meta_gen.max(self.sync_generation);
        }
        if let Some(his_body) = hero_item_stats_json {
            atomic_write_file(&self.hero_item_stats_json_path(), his_body)?;
            state.latest_committed_meta_gen =
                state.latest_committed_meta_gen.max(self.sync_generation);
        }
        if player_json.is_some() || player_matches_json.is_some() || player_profile_json.is_some() {
            if let (Some(p_dir), Some(p_path), Some(pm_meta), Some(m_path), Some(prof_path)) = (
                self.player_dir(),
                self.player_json_path(),
                self.player_metadata_path(),
                self.player_matches_json_path(),
                self.player_profile_json_path(),
            ) {
                fs::create_dir_all(&p_dir).map_err(|e| PortError(e.to_string()))?;
                if let Some(pm_body) = player_matches_json {
                    atomic_write_file(&m_path, pm_body)?;
                }
                if let Some(prof_body) = player_profile_json {
                    atomic_write_file(&prof_path, prof_body)?;
                }
                if let Some(p_body) = player_json {
                    atomic_write_file(&p_path, p_body)?;
                    atomic_write_file(&pm_meta, &format!("player_sync_unix={now_unix}\n"))?;
                }
            }
        }
        Ok(())
    }

    pub fn write_cached_files(
        &self,
        meta_json: Option<&str>,
        player_json: Option<&str>,
        now_unix: u64,
    ) -> Result<(), PortError> {
        self.publish_staged_cache(meta_json, None, player_json, None, None, now_unix)
    }

    /// Synchronizes stale or forced endpoints from OpenDota (and optional STRATZ token) with a 6s timeout.
    pub fn sync_from_opendota(&self, force: bool) -> Result<String, PortError> {
        self.sync_from_sources_with_role_items(
            force,
            Self::now_unix(),
            fetch_url,
            fetch_stratz_with_synergy_graphql,
            fetch_stratz_hero_item_purchases_graphql_for_role,
        )
    }

    /// Synchronizes stale or forced endpoints using injected OpenDota and STRATZ fetchers.
    pub fn sync_from_sources_with<FUrl, FStratzSyn, FStratzItems>(
        &self,
        force: bool,
        now_unix: u64,
        fetch_opendota_url: FUrl,
        fetch_stratz_synergy: FStratzSyn,
        mut fetch_stratz_items: FStratzItems,
    ) -> Result<String, PortError>
    where
        FUrl: FnMut(&str) -> Result<String, PortError>,
        FStratzSyn: FnMut(&str) -> Result<String, PortError>,
        FStratzItems: FnMut(&str, u16) -> Result<String, PortError>,
    {
        self.sync_from_sources_with_role_items(
            force,
            now_unix,
            fetch_opendota_url,
            fetch_stratz_synergy,
            |token, hero_id, _role| fetch_stratz_items(token, hero_id),
        )
    }

    /// Synchronizes stale or forced endpoints using role-aware STRATZ item fetchers.
    pub(crate) fn sync_from_sources_with_role_items<FUrl, FStratzSyn, FStratzItems>(
        &self,
        force: bool,
        now_unix: u64,
        mut fetch_opendota_url: FUrl,
        mut fetch_stratz_synergy: FStratzSyn,
        mut fetch_stratz_items: FStratzItems,
    ) -> Result<String, PortError>
    where
        FUrl: FnMut(&str) -> Result<String, PortError>,
        FStratzSyn: FnMut(&str) -> Result<String, PortError>,
        FStratzItems: FnMut(&str, u16, Option<Role>) -> Result<String, PortError>,
    {
        if self.is_cancelled() {
            return Err(PortError("Sync cancelled".into()));
        }
        let need_meta = force || self.is_meta_stale(now_unix);
        let need_item_stats = force || need_meta || self.is_target_item_heroes_uncached();
        let need_player =
            self.player_account_id.is_some() && (force || self.is_player_stale(now_unix));
        if !need_meta && !need_item_stats && !need_player {
            return Ok("DB cache is already fresh (Meta <24h, Player <1h).".into());
        }

        let is_arr = |s: &str| {
            let t = s.trim();
            t.starts_with('[') && t.ends_with(']')
        };
        let is_obj = |s: &str| {
            let t = s.trim();
            t.starts_with('{') && t.ends_with('}')
        };
        let (
            mut fetched_meta,
            mut fetched_stratz,
            mut fetched_item_constants,
            mut fetched_hero_items,
            mut fetched_player,
            mut fetched_matches,
            mut fetched_profile,
        ) = (None, None, None, None, None, None, None);
        let mut updated_parts = Vec::new();
        let mut deferred_errors = Vec::new();
        let stratz_token = self.effective_stratz_token_at(now_unix);

        if need_meta {
            match fetch_opendota_url("https://api.opendota.com/api/heroStats") {
                Ok(body) => {
                    if is_arr(&body) && !parse_hero_stats_json(&body).is_empty() {
                        fetched_meta = Some(body);
                        updated_parts.push("heroStats (24h TTL)");
                    }
                    if !self.is_cancelled() {
                        if let Some(ic_body) =
                            fetch_opendota_url("https://api.opendota.com/api/constants/items")
                                .ok()
                                .filter(|b| !parse_opendota_item_constants_json(b).is_empty())
                        {
                            fetched_item_constants = Some(ic_body);
                            updated_parts.push("itemConstants (24h TTL)");
                        }
                    }
                }
                Err(e) => {
                    deferred_errors.push(format!("meta: {e}"));
                }
            }
            if !self.is_cancelled() {
                if let Some(ref token) = stratz_token {
                    if let Some(sb) = fetch_stratz_synergy(token)
                        .ok()
                        .filter(|s| !parse_stratz_with_synergy_json(s).is_empty())
                    {
                        fetched_stratz = Some(sb);
                        updated_parts.push("STRATZ synergy");
                    }
                }
            }
        }
        if need_item_stats && !self.is_cancelled() {
            let mut catalog_repo = SeededItemMetaRepository::new();
            if let Some(ref ic_body) = fetched_item_constants {
                let parsed = parse_opendota_item_constants_json(ic_body);
                if !parsed.is_empty() {
                    catalog_repo.custom_catalog = parsed;
                }
            } else if let Ok(existing_ic) = fs::read_to_string(self.item_constants_json_path()) {
                let parsed = parse_opendota_item_constants_json(&existing_ic);
                if !parsed.is_empty() {
                    catalog_repo.custom_catalog = parsed;
                }
            }

            let mut merged_entries: CachedHeroItemStatsMap = std::collections::BTreeMap::new();
            if let Ok(existing_his) = fs::read_to_string(self.hero_item_stats_json_path()) {
                for (hid, role_opt, bucket_opt, stats) in
                    parse_cached_hero_item_stats_entries(&existing_his, &catalog_repo)
                {
                    if !stats.is_empty() {
                        merged_entries.insert(
                            (
                                hid.0,
                                role_opt.map(role_to_cache_idx),
                                bucket_opt.map(StratzBracketBucket::index),
                            ),
                            stats,
                        );
                    }
                }
            }

            let restrict_to_requested = !self.target_item_hero_ids.is_empty();
            let mut any_item_stats_fetched = false;
            for (hero_id, role_opt) in self.effective_target_item_hero_roles() {
                if self.is_cancelled() {
                    break;
                }
                if !force
                    && !need_meta
                    && is_hero_role_covered_in_keys(
                        merged_entries.keys().copied(),
                        hero_id,
                        role_opt,
                        stratz_token.is_some(),
                    )
                {
                    continue;
                }
                let mut covered_buckets_mask: u8 = 0;
                let mut covered_unbracketed = false;
                if let Some(ref token) = stratz_token {
                    if let Ok(sb) = fetch_stratz_items(token, hero_id.0, role_opt) {
                        let parsed_entries =
                            parse_stratz_live_bracketed_item_purchases_json(&sb, hero_id, role_opt);
                        for (entry_hid, entry_role, bucket_opt, mut stats) in parsed_entries {
                            stats.retain(|stat| {
                                stat.phase == ItemPhase::Starting
                                    || stat.phase == ItemPhase::Situational
                                    || crate::item_repo::is_valid_timed_progression_item_for_phase(
                                        &catalog_repo,
                                        stat.item_id,
                                        stat.phase,
                                    )
                            });
                            if !stats.is_empty() {
                                if let Some(bucket) = bucket_opt {
                                    covered_buckets_mask |= 1_u8 << bucket.index();
                                } else {
                                    covered_unbracketed = true;
                                }
                                merged_entries.insert(
                                    (
                                        entry_hid.0,
                                        entry_role.map(role_to_cache_idx),
                                        bucket_opt.map(StratzBracketBucket::index),
                                    ),
                                    stats,
                                );
                                any_item_stats_fetched = true;
                            }
                        }
                    }
                }
                let all_buckets_covered = covered_unbracketed || covered_buckets_mask == 0b1111;
                if !all_buckets_covered {
                    let url = format!(
                        "https://api.opendota.com/api/scenarios/itemTimings?hero_id={}",
                        hero_id.0
                    );
                    let mut hero_stats_accum: Vec<HeroItemStat> = Vec::new();
                    if let Ok(ib_body) = fetch_opendota_url(&url) {
                        let grouped = parse_cached_hero_item_stats_entries(&ib_body, &catalog_repo);
                        if !grouped.is_empty() {
                            for (hid, _, _, stats) in grouped {
                                if hid == hero_id {
                                    hero_stats_accum.extend(stats);
                                } else if !restrict_to_requested && !stats.is_empty() {
                                    merged_entries.insert((hid.0, None, None), stats);
                                    any_item_stats_fetched = true;
                                }
                            }
                        } else {
                            hero_stats_accum =
                                parse_opendota_item_timings_json(&ib_body, &catalog_repo);
                        }
                    }

                    let pop_url = format!(
                        "https://api.opendota.com/api/heroes/{}/itemPopularity",
                        hero_id.0
                    );
                    if let Ok(pop_body) = fetch_opendota_url(&pop_url) {
                        let pop_stats =
                            parse_opendota_item_popularity_json(&pop_body, &catalog_repo);
                        for ps in pop_stats {
                            if !hero_stats_accum.iter().any(|s| s.item_id == ps.item_id) {
                                hero_stats_accum.push(ps);
                            }
                        }
                    }

                    if !hero_stats_accum.is_empty() {
                        merged_entries.insert((hero_id.0, None, None), hero_stats_accum);
                        any_item_stats_fetched = true;
                    }
                }
            }
            if any_item_stats_fetched && !merged_entries.is_empty() {
                fetched_hero_items = Some(serialize_cached_hero_item_stats_v2(&merged_entries));
                updated_parts.push("itemBuilds (24h TTL)");
            }
        }
        if need_player && !self.is_cancelled() {
            if let Some(account_id) = self.player_account_id {
                match fetch_opendota_url(&format!(
                    "https://api.opendota.com/api/players/{account_id}/heroes"
                )) {
                    Ok(body) => {
                        if is_arr(&body) {
                            fetched_player = Some(body);
                            updated_parts.push("player comfort (1h TTL)");
                        }
                        if !self.is_cancelled() {
                            if let Some(mb) = fetch_opendota_url(&format!(
                                "https://api.opendota.com/api/players/{account_id}/matches?limit=500"
                            ))
                            .ok()
                            .filter(|m| is_arr(m))
                            {
                                fetched_matches = Some(mb);
                            }
                        }
                        if !self.is_cancelled() {
                            if let Some(pb) = fetch_opendota_url(&format!(
                                "https://api.opendota.com/api/players/{account_id}"
                            ))
                            .ok()
                            .filter(|p| is_obj(p))
                            {
                                fetched_profile = Some(pb);
                            }
                        }
                    }
                    Err(e) => {
                        deferred_errors.push(format!("player: {e}"));
                    }
                }
            }
        }

        let any_staged = fetched_meta.is_some()
            || fetched_stratz.is_some()
            || fetched_item_constants.is_some()
            || fetched_hero_items.is_some()
            || fetched_player.is_some()
            || fetched_matches.is_some()
            || fetched_profile.is_some();
        if !any_staged && !deferred_errors.is_empty() {
            return Err(PortError(deferred_errors.join("; ")));
        }

        self.publish_staged_cache_with_items(
            fetched_meta.as_deref(),
            fetched_stratz.as_deref(),
            fetched_item_constants.as_deref(),
            fetched_hero_items.as_deref(),
            fetched_player.as_deref(),
            fetched_matches.as_deref(),
            fetched_profile.as_deref(),
            now_unix,
        )?;
        if updated_parts.is_empty() {
            Ok("Synced OpenDota cache.".into())
        } else if deferred_errors.is_empty() {
            Ok(format!("Synced OpenDota: {}.", updated_parts.join(" + ")))
        } else {
            Ok(format!(
                "Synced: {} (partial: {}).",
                updated_parts.join(" + "),
                deferred_errors.join("; ")
            ))
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
        self.build_repository_at(Self::now_unix())
    }

    pub fn build_repository_at(&self, now_unix: u64) -> SeededMetaRepository {
        let mut repo = SeededMetaRepository::new();
        if let Ok(m_json) = fs::read_to_string(self.meta_json_path()) {
            repo.custom_hero_meta = parse_hero_stats_json(&m_json);
        }
        if let Some(p_path) = self.player_json_path() {
            if let Ok(p_json) = fs::read_to_string(p_path) {
                let matches_json = self
                    .player_matches_json_path()
                    .and_then(|mp| fs::read_to_string(mp).ok());
                repo.custom_personal_pool = parse_player_comfort_with_matches_at_json(
                    &p_json,
                    matches_json.as_deref(),
                    now_unix,
                );
            }
        }
        if let Ok(s_json) = fs::read_to_string(self.stratz_synergy_json_path()) {
            repo.custom_pair_synergy = parse_stratz_with_synergy_json(&s_json);
        }
        repo
    }

    /// Builds an item repository from cached `item_constants.json` / `hero_item_stats.json`,
    /// falling back seamlessly to embedded snapshots when offline or uncached.
    pub fn build_item_repository(&self) -> SeededItemMetaRepository {
        let mut repo = SeededItemMetaRepository::new();
        repo.active_patch = self.read_cached_game_patch();
        if let Ok(c_json) = fs::read_to_string(self.item_constants_json_path()) {
            let parsed = parse_opendota_item_constants_json(&c_json);
            if !parsed.is_empty() {
                repo.custom_catalog = parsed;
            }
        }
        if let Ok(s_json) = fs::read_to_string(self.hero_item_stats_json_path()) {
            for (hero_id, role_opt, bucket_opt, stats) in
                parse_cached_hero_item_stats_entries(&s_json, &repo)
            {
                if stats.is_empty() {
                    continue;
                }
                if let Some(bucket) = bucket_opt {
                    for (medal, intra_scale) in bucket.medals_with_intra_scale() {
                        let scaled_stats: Vec<HeroItemStat> = stats
                            .iter()
                            .copied()
                            .map(|mut s| {
                                if matches!(
                                    s.phase,
                                    ItemPhase::Early | ItemPhase::Core | ItemPhase::Late
                                ) {
                                    s.avg_time_mins = (s.avg_time_mins * intra_scale).max(1.0);
                                }
                                s
                            })
                            .collect();
                        repo.custom_hero_stats
                            .push((hero_id, role_opt, Some(medal), scaled_stats));
                    }
                } else {
                    repo.custom_hero_stats
                        .push((hero_id, role_opt, None, stats));
                }
            }
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
            if let (Some(p), Some(w)) =
                (extract_u64_field(block, &pk), extract_u64_field(block, &wk))
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
    run_curl(url, &[], &format!("HTTP fetch for {url}"))
}

/// Extracts `rank_tier` from an OpenDota `/api/players/{account_id}` JSON object and maps it
/// to a `Bracket`, returning `None` when uncalibrated (`null` / `0`) or out of range.
pub fn parse_player_rank_tier_json(json: &str) -> Option<Bracket> {
    let trimmed = json.trim();
    if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
        return None;
    }
    let tier = extract_top_level_u64_field(trimmed, "rank_tier")?;
    let tier_u8 = u8::try_from(tier).ok()?;
    Bracket::from_rank_tier(tier_u8)
}

fn extract_top_level_u64_field(json: &str, target_key: &str) -> Option<u64> {
    let bytes = json.as_bytes();
    let mut i = 0;
    let mut depth = 0_usize;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'"' => {
                let start = i + 1;
                i += 1;
                let mut escaped = false;
                while i < bytes.len() {
                    let b = bytes[i];
                    if escaped {
                        escaped = false;
                    } else if b == b'\\' {
                        escaped = true;
                    } else if b == b'"' {
                        break;
                    }
                    i += 1;
                }
                let end = i;
                i += 1;
                if depth == 1 && json.get(start..end) == Some(target_key) {
                    let mut rest = json[i..].trim_start();
                    if let Some(after_colon) = rest.strip_prefix(':') {
                        rest = after_colon.trim_start();
                        let digits: String = rest
                            .chars()
                            .skip_while(|c| *c == '"')
                            .take_while(|c| c.is_ascii_digit())
                            .collect();
                        return digits.parse::<u64>().ok();
                    }
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// Lightweight parser for OpenDota `/api/players/{id}/heroes` JSON array.
/// Returns `Vec<(HeroId, games, wins, comfort_decay)>`.
pub fn parse_player_comfort_json(json: &str) -> Vec<(HeroId, u32, u32, f32)> {
    parse_player_comfort_with_matches_json(json, None)
}

/// Parses OpenDota `/api/players/{id}/heroes` combined with optional `/api/players/{id}/matches`
/// using the current wall-clock Unix timestamp as the recency reference (`tau = 75d`).
pub fn parse_player_comfort_with_matches_json(
    heroes_json: &str,
    matches_json: Option<&str>,
) -> Vec<(HeroId, u32, u32, f32)> {
    parse_player_comfort_with_matches_at_json(
        heroes_json,
        matches_json,
        LocalCacheManager::now_unix(),
    )
}

/// Parses OpenDota `/api/players/{id}/heroes` combined with optional `/api/players/{id}/matches`
/// against an explicit `now_unix` reference clock (`tau = 75d`, floor `0.0`).
pub fn parse_player_comfort_with_matches_at_json(
    heroes_json: &str,
    matches_json: Option<&str>,
    now_unix: u64,
) -> Vec<(HeroId, u32, u32, f32)> {
    const TAU_DAYS: f32 = 75.0;

    let mut raw_rows: Vec<(u16, u64, u32, u32)> = Vec::new();
    let ref_ts = now_unix;

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
            let age_days = (ref_ts.saturating_sub(st)) as f32 / 86_400.0;
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
                let rec_wr = if g_eff > 0.01 {
                    w_eff / g_eff
                } else {
                    career_wr
                };
                let career_weight = 0.35 * decay;
                let blended_wr = (1.0 - career_weight) * rec_wr + career_weight * career_wr;
                let eff_wins = ((games as f32) * blended_wr).round() as u32;
                (HeroId(hid), games, eff_wins, decay)
            } else {
                let raw_decay = if last_played > 0 && ref_ts > 0 {
                    let age_days = (ref_ts.saturating_sub(last_played)) as f32 / 86_400.0;
                    (-age_days / TAU_DAYS).exp().clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let decay = if matches_json.is_some() {
                    (raw_decay / (games as f32).max(1.0)).clamp(0.0, 1.0)
                } else {
                    raw_decay
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

pub(crate) const STRATZ_USER_AGENT_HEADER: &str = "User-Agent: STRATZ_API";

pub(crate) fn stratz_matchup_graphql_payload(hero_ids: &[u16]) -> String {
    let ids = hero_ids
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"query":"{{ heroStats {{ matchUp(heroIds: [{ids}], take: 200) {{ heroId with {{ heroId1 heroId2 matchCount synergy winCount }} }} }} }}"}}"#
    )
}

/// Returns `true` when `json` contains an `"errors"` field whose array value has at least one
/// non-whitespace character before `]`.
pub(crate) fn has_nonempty_graphql_errors(json: &str) -> bool {
    for (idx, _) in json.match_indices("\"errors\"") {
        let after_key = json[idx + "\"errors\"".len()..].trim_start();
        let Some(after_colon) = after_key.strip_prefix(':') else {
            continue;
        };
        let after_ws = after_colon.trim_start();
        if let Some(after_bracket) = after_ws.strip_prefix('[') {
            let trimmed = after_bracket.trim_start();
            if !trimmed.is_empty() && !trimmed.starts_with(']') {
                return true;
            }
        }
    }
    false
}

/// Parses STRATZ empirical `with` synergy JSON rows (`heroId1`, `heroId2`, `synergy` / `wins`+`matchCount`).
pub fn parse_stratz_with_synergy_json(json: &str) -> Vec<(HeroId, HeroId, f32, u32)> {
    if has_nonempty_graphql_errors(json) {
        return Vec::new();
    }
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
        let syn = if let Some(s) = extract_f32_field(block, "\"synergy\"") {
            // STRATZ always reports `"synergy"` in percentage points (-100..+100).
            (s / 100.0).clamp(-0.15, 0.15)
        } else if let Some(s) = extract_f32_field(block, "\"with_synergy\"") {
            // Explicit probability-ratio field used by synthetic ratio fixtures.
            s.clamp(-0.15, 0.15)
        } else if let Some(wins) = extract_u64_field(block, "\"wins\"")
            .or_else(|| extract_u64_field(block, "\"winCount\""))
        {
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

/// Fetches STRATZ `matchUp` synergy batches using `fetch_batch`, failing the entire refresh if
/// any chunk fails or returns zero parseable synergy rows so partial responses never overwrite cache.
pub(crate) fn fetch_stratz_with_synergy_batches_with<F>(
    mut fetch_batch: F,
) -> Result<String, PortError>
where
    F: FnMut(&[u16]) -> Result<String, PortError>,
{
    let hero_ids: Vec<u16> = crate::snapshot_data::OPENDOTA_HERO_SNAPSHOT
        .iter()
        .map(|row| row.id)
        .collect();
    let mut responses = Vec::new();
    for (batch_idx, chunk) in hero_ids.chunks(32).enumerate() {
        let body = fetch_batch(chunk)?;
        if has_nonempty_graphql_errors(&body) {
            return Err(PortError(format!(
                "STRATZ GraphQL batch {batch_idx} returned GraphQL errors"
            )));
        }
        if parse_stratz_with_synergy_json(&body).is_empty() {
            return Err(PortError(
                "STRATZ GraphQL batch returned no parseable synergy rows".to_string(),
            ));
        }
        responses.push(body);
    }
    if responses.is_empty() {
        return Err(PortError(
            "STRATZ GraphQL fetch returned no data".to_string(),
        ));
    }
    Ok(format!("[{}]", responses.join(",")))
}

fn fetch_stratz_with_synergy_graphql(token: &str) -> Result<String, PortError> {
    let auth_header = format!("Authorization: Bearer {token}");
    fetch_stratz_with_synergy_batches_with(|chunk| {
        let payload = stratz_matchup_graphql_payload(chunk);
        run_curl(
            "https://api.stratz.com/graphql",
            &[
                "-H",
                "Content-Type: application/json",
                "-H",
                STRATZ_USER_AGENT_HEADER,
                "-H",
                &auth_header,
                "-d",
                &payload,
            ],
            "STRATZ GraphQL fetch",
        )
    })
}

pub(crate) type ParsedHeroItemStatsEntry = (
    HeroId,
    Option<Role>,
    Option<StratzBracketBucket>,
    Vec<HeroItemStat>,
);
type CachedHeroItemStatsKey = (u16, Option<u8>, Option<u8>);
type CachedHeroItemStatsMap = std::collections::BTreeMap<CachedHeroItemStatsKey, Vec<HeroItemStat>>;

/// Four STRATZ `RankBracketBasicEnum` pairs queried and cached per `(hero, role)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StratzBracketBucket {
    HeraldGuardian = 0,
    CrusaderArchon = 1,
    LegendAncient = 2,
    DivineImmortal = 3,
}

impl StratzBracketBucket {
    const ALL: [Self; 4] = [
        Self::HeraldGuardian,
        Self::CrusaderArchon,
        Self::LegendAncient,
        Self::DivineImmortal,
    ];

    const fn index(self) -> u8 {
        self as u8
    }

    const fn gql_name(self) -> &'static str {
        match self {
            Self::HeraldGuardian => "HERALD_GUARDIAN",
            Self::CrusaderArchon => "CRUSADER_ARCHON",
            Self::LegendAncient => "LEGEND_ANCIENT",
            Self::DivineImmortal => "DIVINE_IMMORTAL",
        }
    }

    const fn alias_prefix(self) -> &'static str {
        match self {
            Self::HeraldGuardian => "hg",
            Self::CrusaderArchon => "ca",
            Self::LegendAncient => "la",
            Self::DivineImmortal => "di",
        }
    }

    pub(crate) const fn cache_key(self) -> &'static str {
        self.gql_name()
    }

    fn from_cache_key(raw: &str) -> Option<Self> {
        match raw.trim() {
            "HERALD_GUARDIAN" => Some(Self::HeraldGuardian),
            "CRUSADER_ARCHON" => Some(Self::CrusaderArchon),
            "LEGEND_ANCIENT" => Some(Self::LegendAncient),
            "DIVINE_IMMORTAL" => Some(Self::DivineImmortal),
            _ => None,
        }
    }

    const fn medals_with_intra_scale(self) -> [(Bracket, f32); 2] {
        match self {
            Self::HeraldGuardian => [(Bracket::Herald, 1.022), (Bracket::Guardian, 0.978)],
            Self::CrusaderArchon => [(Bracket::Crusader, 1.024), (Bracket::Archon, 0.976)],
            Self::LegendAncient => [(Bracket::Legend, 1.020), (Bracket::Ancient, 0.980)],
            Self::DivineImmortal => [(Bracket::Divine, 1.016), (Bracket::Immortal, 0.984)],
        }
    }
}

const fn role_to_cache_idx(role: Role) -> u8 {
    role.index() as u8
}

const fn role_to_cache_key(role: Role) -> &'static str {
    match role {
        Role::Carry => "Carry",
        Role::Mid => "Mid",
        Role::Offlane => "Offlane",
        Role::SoftSupport => "SoftSupport",
        Role::HardSupport => "HardSupport",
        _ => "Carry",
    }
}

fn role_from_cache_key(raw: &str) -> Option<Role> {
    match raw.trim() {
        "Carry" | "POSITION_1" => Some(Role::Carry),
        "Mid" | "POSITION_2" => Some(Role::Mid),
        "Offlane" | "POSITION_3" => Some(Role::Offlane),
        "SoftSupport" | "POSITION_4" => Some(Role::SoftSupport),
        "HardSupport" | "POSITION_5" => Some(Role::HardSupport),
        _ => None,
    }
}

const fn stratz_position_enum(role: Role) -> &'static str {
    match role {
        Role::Carry => "POSITION_1",
        Role::Mid => "POSITION_2",
        Role::Offlane => "POSITION_3",
        Role::SoftSupport => "POSITION_4",
        Role::HardSupport => "POSITION_5",
        _ => "POSITION_1",
    }
}

pub(crate) fn stratz_hero_item_purchases_graphql_payload_for_role(
    hero_id: u16,
    role: Option<Role>,
) -> String {
    let pos_arg = role
        .map(|r| format!(", positionIds: [{}]", stratz_position_enum(r)))
        .unwrap_or_default();
    let mut parts = Vec::with_capacity(StratzBracketBucket::ALL.len() * 3);
    for bucket in StratzBracketBucket::ALL {
        let p = bucket.alias_prefix();
        let b = bucket.gql_name();
        parts.push(format!(
            "{p}Start: itemStartingPurchase(heroId: {hero_id}{pos_arg}, bracketBasicIds: [{b}]) {{ itemId instance matchCount winCount wasGiven }}"
        ));
        parts.push(format!(
            "{p}Boot: itemBootPurchase(heroId: {hero_id}{pos_arg}, bracketBasicIds: [{b}]) {{ itemId time matchCount winCount }}"
        ));
        parts.push(format!(
            "{p}Full: itemFullPurchase(heroId: {hero_id}{pos_arg}, bracketBasicIds: [{b}], matchLimit: 5) {{ itemId time instance matchCount winCount }}"
        ));
    }
    format!(r#"{{"query":"{{ heroStats {{ {} }} }}"}}"#, parts.join(" "))
}

#[cfg(test)]
pub(crate) fn stratz_hero_item_purchases_graphql_payload(hero_id: u16) -> String {
    stratz_hero_item_purchases_graphql_payload_for_role(hero_id, None)
}

fn fetch_stratz_hero_item_purchases_graphql_for_role(
    token: &str,
    hero_id: u16,
    role: Option<Role>,
) -> Result<String, PortError> {
    let auth_header = format!("Authorization: Bearer {token}");
    let query = stratz_hero_item_purchases_graphql_payload_for_role(hero_id, role);
    run_curl(
        "https://api.stratz.com/graphql",
        &[
            "-H",
            "Content-Type: application/json",
            "-H",
            STRATZ_USER_AGENT_HEADER,
            "-H",
            &auth_header,
            "-d",
            &query,
        ],
        "STRATZ item builds fetch",
    )
}

fn extract_jwt_exp_unverified(token: &str) -> Option<u64> {
    let payload_b64 = token.trim().split('.').nth(1)?;
    let clean = payload_b64.trim_end_matches('=');
    if clean.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity((clean.len() * 3) / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for byte in clean.bytes() {
        let val: u32 = match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a') + 26,
            b'0'..=b'9' => u32::from(byte - b'0') + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    let payload = std::str::from_utf8(&out).ok()?;
    extract_u64_field(payload, "\"exp\"")
}

fn extract_str_field<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    let idx = block.find(key)?;
    let rest = block[idx + key.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut end = 0usize;
    let bytes = rest.as_bytes();
    while end < bytes.len() {
        if bytes[end] == b'\\' {
            end += 2;
            continue;
        }
        if bytes[end] == b'"' {
            return Some(&rest[..end]);
        }
        end += 1;
    }
    None
}

fn extract_u64_or_quoted_field(block: &str, key: &str) -> Option<u64> {
    if let Some(v) = extract_u64_field(block, key) {
        return Some(v);
    }
    extract_str_field(block, key)?.trim().parse::<u64>().ok()
}

fn extract_i64_field(block: &str, key: &str) -> Option<i64> {
    let idx = block.find(key)?;
    let rest = block[idx + key.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(rest.len());
    rest[..end].parse::<i64>().ok()
}

fn phase_from_avg_mins(avg_mins: f32) -> ItemPhase {
    if avg_mins < 12.0 {
        ItemPhase::Early
    } else if avg_mins <= 24.5 {
        ItemPhase::Core
    } else {
        ItemPhase::Late
    }
}

fn phase_from_avg_mins_for_role(avg_mins: f32, role: Option<Role>) -> ItemPhase {
    match role {
        Some(Role::SoftSupport | Role::HardSupport) => {
            if avg_mins < 13.0 {
                ItemPhase::Early
            } else if avg_mins <= 28.5 {
                ItemPhase::Core
            } else {
                ItemPhase::Late
            }
        }
        _ => phase_from_avg_mins(avg_mins),
    }
}

const fn phase_to_cache_key(phase: ItemPhase) -> &'static str {
    match phase {
        ItemPhase::Starting => "Starting",
        ItemPhase::Early => "Early",
        ItemPhase::Core => "Core",
        ItemPhase::Late => "Late",
        ItemPhase::Situational => "Situational",
    }
}

fn phase_from_cache_key(raw: &str) -> Option<ItemPhase> {
    match raw.trim() {
        "Starting" => Some(ItemPhase::Starting),
        "Early" => Some(ItemPhase::Early),
        "Core" => Some(ItemPhase::Core),
        "Late" => Some(ItemPhase::Late),
        "Situational" => Some(ItemPhase::Situational),
        _ => None,
    }
}

/// Extracts a JSON string array field (e.g. `"components": ["pers", "broadsword"]`),
/// ignoring `null` or empty strings `""` and stripping any `"item_"` prefix.
fn extract_str_array_field(block: &str, key: &str) -> Vec<String> {
    let Some(idx) = block.find(key) else {
        return Vec::new();
    };
    let rest = block[idx + key.len()..].trim_start();
    let Some(rest) = rest.strip_prefix(':') else {
        return Vec::new();
    };
    let rest = rest.trim_start();
    if !rest.starts_with('[') {
        return Vec::new();
    }
    let Some((arr_slice, _)) = extract_balanced_brackets(rest, 0) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let bytes = arr_slice.as_bytes();
    let mut pos = 1usize;
    while pos < bytes.len() {
        if bytes[pos] == b'"' {
            pos += 1;
            let start = pos;
            while pos < bytes.len() {
                if bytes[pos] == b'\\' {
                    pos += 2;
                    continue;
                }
                if bytes[pos] == b'"' {
                    break;
                }
                pos += 1;
            }
            if pos <= bytes.len() {
                let raw = arr_slice[start..pos].trim();
                let slug = raw.strip_prefix("item_").unwrap_or(raw).trim();
                if !slug.is_empty() {
                    out.push(slug.to_string());
                }
            }
            pos += 1;
        } else {
            pos += 1;
        }
    }
    out
}

/// Parses OpenDota `/api/constants/items` object or STRATZ `constants.items` array into `ItemCatalogEntry` records.
pub fn parse_opendota_item_constants_json(json: &str) -> Vec<ItemCatalogEntry> {
    let trimmed = json.trim();
    if trimmed.is_empty() || has_nonempty_graphql_errors(trimmed) {
        return Vec::new();
    }

    if trimmed.contains("\"constants\"") && trimmed.contains("\"items\"") {
        return parse_stratz_item_constants_json(trimmed);
    }

    let mut staged: Vec<(ItemCatalogEntry, Vec<String>)> = Vec::new();
    let bytes = trimmed.as_bytes();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let Some(rel_quote) = trimmed[pos..].find('"') else {
            break;
        };
        let key_start = pos + rel_quote + 1;
        let Some(rel_end) = trimmed[key_start..].find('"') else {
            break;
        };
        let key = &trimmed[key_start..key_start + rel_end];
        let after_key = trimmed[key_start + rel_end + 1..].trim_start();
        if let Some(after_colon) = after_key.strip_prefix(':') {
            let after_colon_trim = after_colon.trim_start();
            if after_colon_trim.starts_with('{') {
                let obj_offset = trimmed.len() - after_colon_trim.len();
                if let Some((obj_slice, next_pos)) = extract_balanced_braces(trimmed, obj_offset) {
                    if let Some(id_val) =
                        extract_u64_field(obj_slice, "\"id\"").and_then(|v| u16::try_from(v).ok())
                    {
                        if id_val > 0 {
                            let slug = key.strip_prefix("item_").unwrap_or(key).to_string();
                            let display_name = extract_str_field(obj_slice, "\"dname\"")
                                .unwrap_or(&slug)
                                .to_string();
                            let cost = extract_u64_field(obj_slice, "\"cost\"").unwrap_or(0) as u32;
                            let is_recipe =
                                slug.contains("recipe") || display_name.contains("Recipe");
                            let raw_components =
                                extract_str_array_field(obj_slice, "\"components\"");
                            staged.push((
                                ItemCatalogEntry {
                                    id: ItemId(id_val),
                                    slug,
                                    display_name,
                                    cost,
                                    components: Vec::new(),
                                    is_recipe,
                                    is_consumable_drop: cost == 0 && !is_recipe,
                                },
                                raw_components,
                            ));
                        }
                    }
                    pos = next_pos;
                    continue;
                }
            }
        }
        pos = key_start + rel_end + 1;
    }

    let fallback_repo = SeededItemMetaRepository::new();
    let mut slug_to_id = std::collections::HashMap::with_capacity(staged.len());
    for (entry, _) in &staged {
        slug_to_id.insert(entry.slug.to_ascii_lowercase(), entry.id);
    }

    staged
        .into_iter()
        .map(|(mut entry, raw_components)| {
            entry.components = raw_components
                .into_iter()
                .filter_map(|comp_slug| {
                    slug_to_id
                        .get(&comp_slug.to_ascii_lowercase())
                        .copied()
                        .or_else(|| fallback_repo.item_by_slug(&comp_slug).map(|e| e.id))
                })
                .collect();
            entry
        })
        .collect()
}

fn parse_stratz_item_constants_json(json: &str) -> Vec<ItemCatalogEntry> {
    let mut entries = Vec::new();
    let Some(items_idx) = json.find("\"items\"") else {
        return entries;
    };
    let rest = &json[items_idx..];
    let Some(arr_start_rel) = rest.find('[') else {
        return entries;
    };
    let mut pos = items_idx + arr_start_rel + 1;
    let bytes = json.as_bytes();
    while pos < bytes.len() {
        match bytes[pos] {
            b']' => break,
            b'{' => {
                let Some((obj_slice, next_pos)) = extract_balanced_braces(json, pos) else {
                    break;
                };
                if let Some(id_val) =
                    extract_u64_field(obj_slice, "\"id\"").and_then(|v| u16::try_from(v).ok())
                {
                    if id_val > 0 {
                        let raw_name = extract_str_field(obj_slice, "\"name\"").unwrap_or("");
                        let slug = raw_name
                            .strip_prefix("item_")
                            .unwrap_or(raw_name)
                            .to_string();
                        let display_name = extract_str_field(obj_slice, "\"displayName\"")
                            .unwrap_or(&slug)
                            .to_string();
                        let cost = extract_u64_field(obj_slice, "\"cost\"").unwrap_or(0) as u32;
                        let is_recipe =
                            extract_bool_field(obj_slice, "\"isRecipe\"").unwrap_or(false);
                        let mut components = Vec::new();
                        let mut comp_search = obj_slice;
                        while let Some(c_idx) = comp_search.find("\"componentId\"") {
                            let sub = &comp_search[c_idx..];
                            if let Some(cid) = extract_u64_field(sub, "\"componentId\"")
                                .and_then(|v| u16::try_from(v).ok())
                            {
                                if cid > 0 {
                                    components.push(ItemId(cid));
                                }
                            }
                            comp_search = &comp_search[c_idx + "\"componentId\"".len()..];
                        }
                        entries.push(ItemCatalogEntry {
                            id: ItemId(id_val),
                            slug,
                            display_name,
                            cost,
                            components,
                            is_recipe,
                            is_consumable_drop: cost == 0 && !is_recipe,
                        });
                    }
                }
                pos = next_pos;
            }
            _ => pos += 1,
        }
    }
    entries
}

fn extract_balanced_braces(src: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = src.as_bytes();
    if bytes.get(start).copied() != Some(b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_str = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_str = false;
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some((&src[start..=i], i + 1));
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn extract_balanced_brackets(src: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = src.as_bytes();
    if bytes.get(start).copied() != Some(b'[') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_str = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_str = false;
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'[' => depth += 1,
                b']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some((&src[start..=i], i + 1));
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn extract_json_array_for_key<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let idx = json.find(key)?;
    let after_key = json[idx + key.len()..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    if !after_colon.starts_with('[') {
        return None;
    }
    extract_balanced_brackets(after_colon, 0).map(|(arr, _)| arr)
}

fn parse_stratz_purchase_arrays(
    start_arr: Option<&str>,
    boot_arr: Option<&str>,
    full_arr: Option<&str>,
    force_full_minutes: bool,
    role: Option<Role>,
) -> Vec<HeroItemStat> {
    let mut buckets: Vec<(ItemId, ItemPhase, f64, u64, u64)> = Vec::new();

    if let Some(arr_slice) = start_arr {
        for obj_chunk in arr_slice.split('{').skip(1) {
            let Some(end) = obj_chunk.find('}') else {
                continue;
            };
            let block = &obj_chunk[..end];
            if extract_bool_field(block, "\"wasGiven\"") == Some(true) {
                continue;
            }
            let Some(id_val) =
                extract_u64_field(block, "\"itemId\"").and_then(|v| u16::try_from(v).ok())
            else {
                continue;
            };
            let matches = extract_u64_field(block, "\"matchCount\"").unwrap_or(0);
            let wins = extract_u64_field(block, "\"winCount\"").unwrap_or(0);
            if id_val == 0 || matches == 0 {
                continue;
            }
            upsert_item_stat_bucket(
                &mut buckets,
                ItemId(id_val),
                ItemPhase::Starting,
                0.0,
                matches,
                wins,
            );
        }
    }

    if let Some(arr_slice) = boot_arr {
        for obj_chunk in arr_slice.split('{').skip(1) {
            let Some(end) = obj_chunk.find('}') else {
                continue;
            };
            let block = &obj_chunk[..end];
            let Some(id_val) =
                extract_u64_field(block, "\"itemId\"").and_then(|v| u16::try_from(v).ok())
            else {
                continue;
            };
            let matches = extract_u64_field(block, "\"matchCount\"").unwrap_or(0);
            let wins = extract_u64_field(block, "\"winCount\"").unwrap_or(0);
            let time_raw = extract_i64_field(block, "\"time\"").unwrap_or(0).max(0) as u64;
            if id_val == 0 || matches == 0 {
                continue;
            }
            // STRATZ `itemBootPurchase.time` returns the cumulative seconds sum across all matches.
            let avg_mins = if time_raw >= matches.saturating_mul(120) {
                (time_raw as f64 / matches as f64) / 60.0
            } else if time_raw >= 120 {
                time_raw as f64 / 60.0
            } else {
                time_raw as f64
            };
            let avg_mins_f32 = avg_mins as f32;
            let phase = if avg_mins_f32 <= 16.0 {
                ItemPhase::Early
            } else {
                phase_from_avg_mins_for_role(avg_mins_f32, role)
            };
            upsert_item_stat_bucket(&mut buckets, ItemId(id_val), phase, avg_mins, matches, wins);
        }
    }

    if let Some(arr_slice) = full_arr {
        // Aggregate per itemId across minute buckets so weighted average time cleanly maps to one phase,
        // while preserving explicit persisted `phase` fields from v2 cache entries.
        let mut full_agg: std::collections::BTreeMap<u16, (f64, u64, u64)> =
            std::collections::BTreeMap::new();
        for obj_chunk in arr_slice.split('{').skip(1) {
            let Some(end) = obj_chunk.find('}') else {
                continue;
            };
            let block = &obj_chunk[..end];
            let Some(id_val) =
                extract_u64_field(block, "\"itemId\"").and_then(|v| u16::try_from(v).ok())
            else {
                continue;
            };
            let matches = extract_u64_field(block, "\"matchCount\"").unwrap_or(0);
            let wins = extract_u64_field(block, "\"winCount\"").unwrap_or(0);
            if id_val == 0 || matches == 0 {
                continue;
            }
            let avg_mins = if let Some(secs) = extract_i64_field(block, "\"timeSecs\"") {
                secs.max(0) as f64 / 60.0
            } else {
                let raw_time = extract_i64_field(block, "\"time\"").unwrap_or(0).max(0);
                if force_full_minutes && raw_time < 180 {
                    raw_time as f64
                } else if raw_time >= 120 {
                    raw_time as f64 / 60.0
                } else {
                    raw_time as f64
                }
            };
            if let Some(explicit_phase) =
                extract_str_field(block, "\"phase\"").and_then(phase_from_cache_key)
            {
                upsert_item_stat_bucket(
                    &mut buckets,
                    ItemId(id_val),
                    explicit_phase,
                    avg_mins,
                    matches,
                    wins,
                );
            } else {
                let slot = full_agg.entry(id_val).or_insert((0.0, 0, 0));
                slot.0 += avg_mins * matches as f64;
                slot.1 += matches;
                slot.2 += wins;
            }
        }
        for (raw_id, (weighted_mins, matches, wins)) in full_agg {
            if matches > 0 {
                let avg_mins = weighted_mins / matches as f64;
                let phase = phase_from_avg_mins_for_role(avg_mins as f32, role);
                upsert_item_stat_bucket(
                    &mut buckets,
                    ItemId(raw_id),
                    phase,
                    avg_mins,
                    matches,
                    wins,
                );
            }
        }
    }

    buckets
        .into_iter()
        .map(
            |(item_id, phase, weighted_mins, matches, wins)| HeroItemStat {
                item_id,
                phase,
                avg_time_mins: if matches > 0 {
                    (weighted_mins / matches as f64) as f32
                } else {
                    0.0
                },
                match_count: matches.min(u64::from(u32::MAX)) as u32,
                win_count: wins.min(u64::from(u32::MAX)) as u32,
            },
        )
        .collect()
}

/// Parses STRATZ `itemStartingPurchase` + `itemBootPurchase` + `itemFullPurchase` JSON arrays
/// (or cached `timeSecs` payloads) into aggregated `HeroItemStat` rows.
pub fn parse_stratz_hero_item_purchase_json(json: &str) -> Vec<HeroItemStat> {
    parse_stratz_hero_item_purchase_json_for_role(json, None)
}

pub(crate) fn parse_stratz_hero_item_purchase_json_for_role(
    json: &str,
    role: Option<Role>,
) -> Vec<HeroItemStat> {
    if has_nonempty_graphql_errors(json) {
        return Vec::new();
    }
    parse_stratz_purchase_arrays(
        extract_json_array_for_key(json, "\"itemStartingPurchase\""),
        extract_json_array_for_key(json, "\"itemBootPurchase\""),
        extract_json_array_for_key(json, "\"itemFullPurchase\""),
        false,
        role,
    )
}

/// Parses an aliased 4-bracket live STRATZ item-build response (or legacy unaliased STRATZ payload)
/// into `(HeroId, Option<Role>, Option<StratzBracketBucket>, Vec<HeroItemStat>)` records.
pub(crate) fn parse_stratz_live_bracketed_item_purchases_json(
    json: &str,
    hero_id: HeroId,
    role: Option<Role>,
) -> Vec<ParsedHeroItemStatsEntry> {
    if has_nonempty_graphql_errors(json) {
        return Vec::new();
    }
    let has_aliases = json.contains("\"hgFull\"")
        || json.contains("\"caFull\"")
        || json.contains("\"laFull\"")
        || json.contains("\"diFull\"");
    if has_aliases {
        let mut out = Vec::new();
        for bucket in StratzBracketBucket::ALL {
            let p = bucket.alias_prefix();
            let start_key = format!("\"{p}Start\"");
            let boot_key = format!("\"{p}Boot\"");
            let full_key = format!("\"{p}Full\"");
            let stats = parse_stratz_purchase_arrays(
                extract_json_array_for_key(json, &start_key),
                extract_json_array_for_key(json, &boot_key),
                extract_json_array_for_key(json, &full_key),
                true,
                role,
            );
            if !stats.is_empty() {
                out.push((hero_id, role, Some(bucket), stats));
            }
        }
        return out;
    }

    let stats = parse_stratz_hero_item_purchase_json_for_role(json, role);
    if stats.is_empty() {
        Vec::new()
    } else {
        vec![(hero_id, role, None, stats)]
    }
}

fn upsert_item_stat_bucket(
    buckets: &mut Vec<(ItemId, ItemPhase, f64, u64, u64)>,
    item_id: ItemId,
    phase: ItemPhase,
    avg_mins: f64,
    matches: u64,
    wins: u64,
) {
    if let Some(entry) = buckets
        .iter_mut()
        .find(|(id, p, _, _, _)| *id == item_id && *p == phase)
    {
        entry.2 += avg_mins * matches as f64;
        entry.3 += matches;
        entry.4 += wins;
    } else {
        buckets.push((item_id, phase, avg_mins * matches as f64, matches, wins));
    }
}

fn finalize_weighted_item_buckets(
    buckets: std::collections::BTreeMap<u16, (f64, u64, u64)>,
) -> Vec<HeroItemStat> {
    buckets
        .into_iter()
        .map(|(raw_id, (weighted_mins, matches, wins))| {
            let avg_time_mins = if matches > 0 {
                (weighted_mins / matches as f64) as f32
            } else {
                0.0
            };
            HeroItemStat {
                item_id: ItemId(raw_id),
                phase: phase_from_avg_mins(avg_time_mins),
                avg_time_mins,
                match_count: matches.min(u64::from(u32::MAX)) as u32,
                win_count: wins.min(u64::from(u32::MAX)) as u32,
            }
        })
        .collect()
}

/// Parses OpenDota `/api/scenarios/itemTimings` JSON rows into aggregated `Vec<HeroItemStat>`,
/// weighting purchase times across all time buckets for each item.
pub fn parse_opendota_item_timings_json(
    json: &str,
    catalog: &SeededItemMetaRepository,
) -> Vec<HeroItemStat> {
    let mut agg: std::collections::BTreeMap<u16, (f64, u64, u64)> =
        std::collections::BTreeMap::new();
    for obj_chunk in json.split('{').skip(1) {
        let Some(end) = obj_chunk.find('}') else {
            continue;
        };
        let block = &obj_chunk[..end];
        let Some(slug) = extract_str_field(block, "\"item\"") else {
            continue;
        };
        let Some(entry) = catalog.item_by_slug(slug) else {
            continue;
        };
        let time_secs = extract_i64_field(block, "\"time\"").unwrap_or(0).max(0);
        let matches = extract_u64_or_quoted_field(block, "\"games\"").unwrap_or(0);
        let wins = extract_u64_or_quoted_field(block, "\"wins\"").unwrap_or(0);
        if matches == 0 {
            continue;
        }
        let avg_mins = f64::from(time_secs as f32 / 60.0);
        let prelim_phase = phase_from_avg_mins(avg_mins as f32);
        if !crate::item_repo::is_valid_timed_progression_item_for_phase(
            catalog,
            entry.id,
            prelim_phase,
        ) {
            continue;
        }
        let slot = agg.entry(entry.id.0).or_insert((0.0, 0, 0));
        slot.0 += avg_mins * matches as f64;
        slot.1 += matches;
        slot.2 += wins;
    }
    finalize_weighted_item_buckets(agg)
}

/// Parses OpenDota `/api/heroes/{hero_id}/itemPopularity` JSON into `Vec<HeroItemStat>`.
pub fn parse_opendota_item_popularity_json(
    json: &str,
    catalog: &SeededItemMetaRepository,
) -> Vec<HeroItemStat> {
    let sections = [
        ("\"start_game_items\"", ItemPhase::Starting, 0.0_f32),
        ("\"early_game_items\"", ItemPhase::Early, 7.5_f32),
        ("\"mid_game_items\"", ItemPhase::Core, 18.5_f32),
        ("\"late_game_items\"", ItemPhase::Late, 31.0_f32),
    ];
    let mut out: Vec<HeroItemStat> = Vec::new();
    for (section_key, phase, default_mins) in sections {
        let Some(idx) = json.find(section_key) else {
            continue;
        };
        let Some(brace_rel) = json[idx..].find('{') else {
            continue;
        };
        let Some((obj_slice, _)) = extract_balanced_braces(json, idx + brace_rel) else {
            continue;
        };
        let inner = obj_slice
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}');
        for pair in inner.split(',') {
            let Some((raw_k, raw_v)) = pair.split_once(':') else {
                continue;
            };
            let Ok(id_num) = raw_k.trim().trim_matches('"').parse::<u16>() else {
                continue;
            };
            let Ok(count) = raw_v.trim().trim_matches('"').parse::<u32>() else {
                continue;
            };
            if id_num == 0 || count == 0 {
                continue;
            }
            let item_id = ItemId(id_num);
            let Some(entry) = catalog.item_catalog(item_id) else {
                continue;
            };
            if (entry.cost == 0 && entry.is_consumable_drop) || entry.is_recipe {
                continue;
            }
            if phase != ItemPhase::Starting
                && (!crate::item_repo::is_valid_timed_progression_item(catalog, item_id)
                    || truesight_core::domain::all_referenced_counter_item_ids().contains(&item_id))
            {
                continue;
            }
            if !out.iter().any(|s| s.item_id == item_id) {
                out.push(HeroItemStat {
                    item_id,
                    phase,
                    avg_time_mins: default_mins,
                    match_count: count,
                    win_count: count.div_ceil(2),
                });
            }
        }
    }
    out
}

/// Parses v2 `(hero, optional role, optional bracket)` or legacy v1 cached hero item stats JSON.
fn parse_cached_hero_item_stats_entries(
    json: &str,
    catalog: &SeededItemMetaRepository,
) -> Vec<ParsedHeroItemStatsEntry> {
    let mut entries: Vec<ParsedHeroItemStatsEntry> = Vec::new();

    if json.contains("\"itemStartingPurchase\"")
        || json.contains("\"itemFullPurchase\"")
        || json.contains("\"itemBootPurchase\"")
    {
        let trimmed = json.trim_start();
        if trimmed.starts_with('[') {
            let bytes = trimmed.as_bytes();
            let mut pos = 1usize;
            while pos < bytes.len() {
                match bytes[pos] {
                    b']' => break,
                    b'{' => {
                        let Some((obj_slice, next_pos)) = extract_balanced_braces(trimmed, pos)
                        else {
                            break;
                        };
                        if let Some(hid) = extract_u64_field(obj_slice, "\"heroId\"")
                            .or_else(|| extract_u64_field(obj_slice, "\"hero_id\""))
                            .and_then(|v| u16::try_from(v).ok())
                            .filter(|&v| v > 0)
                        {
                            let role_opt = extract_str_field(obj_slice, "\"role\"")
                                .and_then(role_from_cache_key);
                            let bucket_opt = extract_str_field(obj_slice, "\"bracket\"")
                                .and_then(StratzBracketBucket::from_cache_key);
                            let stats =
                                parse_stratz_hero_item_purchase_json_for_role(obj_slice, role_opt);
                            if !stats.is_empty() {
                                entries.push((HeroId(hid), role_opt, bucket_opt, stats));
                            }
                        }
                        pos = next_pos;
                    }
                    _ => pos += 1,
                }
            }
        } else {
            let role_opt = extract_str_field(trimmed, "\"role\"").and_then(role_from_cache_key);
            let bucket_opt = extract_str_field(trimmed, "\"bracket\"")
                .and_then(StratzBracketBucket::from_cache_key);
            let stats = parse_stratz_hero_item_purchase_json_for_role(json, role_opt);
            if !stats.is_empty() {
                if let Some(hid) = extract_u64_field(json, "\"heroId\"")
                    .or_else(|| extract_u64_field(json, "\"hero_id\""))
                    .and_then(|v| u16::try_from(v).ok())
                    .filter(|&v| v > 0)
                {
                    entries.push((HeroId(hid), role_opt, bucket_opt, stats));
                }
            }
        }
    }

    if entries.is_empty() && !has_nonempty_graphql_errors(json) {
        let mut agg_by_hero: std::collections::BTreeMap<
            u16,
            std::collections::BTreeMap<u16, (f64, u64, u64)>,
        > = std::collections::BTreeMap::new();
        for obj_chunk in json.split('{').skip(1) {
            let Some(end) = obj_chunk.find('}') else {
                continue;
            };
            let block = &obj_chunk[..end];
            let Some(hid) = extract_u64_field(block, "\"hero_id\"")
                .or_else(|| extract_u64_field(block, "\"heroId\""))
                .and_then(|v| u16::try_from(v).ok())
                .filter(|&v| v > 0)
            else {
                continue;
            };
            let Some(slug) = extract_str_field(block, "\"item\"") else {
                continue;
            };
            let Some(entry) = catalog.item_by_slug(slug) else {
                continue;
            };
            let time_secs = extract_i64_field(block, "\"time\"").unwrap_or(0).max(0);
            let matches = extract_u64_or_quoted_field(block, "\"games\"").unwrap_or(0);
            let wins = extract_u64_or_quoted_field(block, "\"wins\"").unwrap_or(0);
            if matches == 0 {
                continue;
            }
            let avg_mins = f64::from(time_secs as f32 / 60.0);
            let slot = agg_by_hero
                .entry(hid)
                .or_default()
                .entry(entry.id.0)
                .or_insert((0.0, 0, 0));
            slot.0 += avg_mins * matches as f64;
            slot.1 += matches;
            slot.2 += wins;
        }
        for (hid, item_buckets) in agg_by_hero {
            entries.push((
                HeroId(hid),
                None,
                None,
                finalize_weighted_item_buckets(item_buckets),
            ));
        }
    }

    for (_, _, _, stats) in &mut entries {
        stats.retain(|stat| {
            stat.phase == ItemPhase::Starting
                || stat.phase == ItemPhase::Situational
                || crate::item_repo::is_valid_timed_progression_item_for_phase(
                    catalog,
                    stat.item_id,
                    stat.phase,
                )
        });
    }
    entries.retain(|(_, _, _, stats)| !stats.is_empty());
    entries
}

fn deduplicate_hero_ids(heroes: Vec<HeroId>) -> Vec<HeroId> {
    let mut out = Vec::new();
    for hid in heroes {
        if hid.0 > 0 && !out.contains(&hid) {
            out.push(hid);
        }
    }
    out
}

fn is_hero_role_covered_in_keys<I>(
    keys: I,
    hero_id: HeroId,
    role_opt: Option<Role>,
    has_stratz_token: bool,
) -> bool
where
    I: Clone + Iterator<Item = (u16, Option<u8>, Option<u8>)>,
{
    match role_opt {
        Some(role) => {
            let target_role_idx = role_to_cache_idx(role);
            if has_stratz_token {
                let has_all_buckets = (0..StratzBracketBucket::ALL.len() as u8).all(|bucket_idx| {
                    keys.clone().any(|(hid, r_idx, b_idx)| {
                        hid == hero_id.0
                            && r_idx == Some(target_role_idx)
                            && b_idx == Some(bucket_idx)
                    })
                });
                let has_role_unbracketed = keys.clone().any(|(hid, r_idx, b_idx)| {
                    hid == hero_id.0 && r_idx == Some(target_role_idx) && b_idx.is_none()
                });
                let has_any_role_bucket = keys.clone().any(|(hid, r_idx, b_idx)| {
                    hid == hero_id.0 && r_idx == Some(target_role_idx) && b_idx.is_some()
                });
                let has_hero_unbracketed_fallback = keys.clone().any(|(hid, r_idx, b_idx)| {
                    hid == hero_id.0 && r_idx.is_none() && b_idx.is_none()
                });
                has_all_buckets
                    || has_role_unbracketed
                    || (has_any_role_bucket && has_hero_unbracketed_fallback)
            } else {
                keys.clone().any(|(hid, r_idx, _)| {
                    hid == hero_id.0 && (r_idx == Some(target_role_idx) || r_idx.is_none())
                })
            }
        }
        None => keys.clone().any(|(hid, _, _)| hid == hero_id.0),
    }
}

fn serialize_cached_hero_item_stats_v2(entries_map: &CachedHeroItemStatsMap) -> String {
    let bucket_from_idx = |idx: u8| match idx {
        0 => Some(StratzBracketBucket::HeraldGuardian),
        1 => Some(StratzBracketBucket::CrusaderArchon),
        2 => Some(StratzBracketBucket::LegendAncient),
        3 => Some(StratzBracketBucket::DivineImmortal),
        _ => None,
    };
    let mut entries = Vec::with_capacity(entries_map.len());
    for (&(hero_id, role_idx_opt, bucket_idx_opt), stats) in entries_map {
        let mut starting = Vec::new();
        let mut full = Vec::new();
        for stat in stats {
            if stat.phase == ItemPhase::Starting {
                starting.push(format!(
                    r#"{{"itemId":{},"instance":0,"matchCount":{},"winCount":{},"wasGiven":false}}"#,
                    stat.item_id.0, stat.match_count, stat.win_count
                ));
            } else {
                let time_secs = (stat.avg_time_mins * 60.0).round().max(0.0) as i64;
                full.push(format!(
                    r#"{{"itemId":{},"phase":"{}","timeSecs":{},"instance":0,"matchCount":{},"winCount":{}}}"#,
                    stat.item_id.0,
                    phase_to_cache_key(stat.phase),
                    time_secs,
                    stat.match_count,
                    stat.win_count
                ));
            }
        }
        let role_field = role_idx_opt
            .and_then(|idx| Role::ALL.get(usize::from(idx)).copied())
            .map(|r| format!(r#","role":"{}""#, role_to_cache_key(r)))
            .unwrap_or_default();
        let bracket_field = bucket_idx_opt
            .and_then(bucket_from_idx)
            .map(|b| format!(r#","bracket":"{}""#, b.cache_key()))
            .unwrap_or_default();
        entries.push(format!(
            r#"{{"heroId":{}{}{},"itemStartingPurchase":[{}],"itemFullPurchase":[{}]}}"#,
            hero_id,
            role_field,
            bracket_field,
            starting.join(","),
            full.join(",")
        ));
    }
    format!("[{}]", entries.join(","))
}
