use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_adapters::item_repo::SeededItemMetaRepository;
use truesight_adapters::sync::{
    AccountLinkConfig, BracketConfig, LocalCacheManager, config_file_path, load_app_config,
    load_app_config_from, normalize_steam32_id, save_account_config_to, save_bracket_config_to,
};
use truesight_adapters::vision::{BoundedVisionWorker, CaptureAvailability, VisionCaptureMode};
use truesight_core::domain::{
    Bracket, BuildVariant, DraftPick, DraftState, Faction, HeroId, ItemId, RecommendedBuild, Role,
    Side, TEAM_SIZE, counter_trait_table_patch,
};
use truesight_core::math::{
    DraftAnalysis5v5, HeroRecommendation, MatchupMatrix, ModelWeights, evaluate_draft_5v5,
    recommend_heroes, recommend_item_build,
};
use truesight_core::ports::{DraftEvent, DraftEventSource, ItemMetaRepository, StatsRepository};

pub(crate) struct SyncCompletion {
    generation: u64,
    account_id: Option<u64>,
    result: Result<String, String>,
}

type BackgroundSyncRunner =
    Arc<dyn Fn(LocalCacheManager, bool) -> Result<String, String> + Send + Sync>;

/// User-selected item variant override keyed by `(hero_id, role, primary_item_id)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VariantSelection {
    /// Hero for which this build variant override was chosen.
    pub hero_id: HeroId,
    /// Role for which this build variant override was chosen.
    pub role: Role,
    /// Original primary item ID of the build step before swapping.
    pub primary_item_id: ItemId,
    /// Currently selected variant item ID replacing `primary_item_id`.
    pub variant_item_id: ItemId,
}

pub const DEFAULT_OVERLAY_OPACITY: f32 = 0.58;
pub const MIN_OVERLAY_OPACITY: f32 = 0.35;
pub const MAX_OVERLAY_OPACITY: f32 = 0.95;

/// Active UI presentation state and persisted configuration backing the desktop/CLI views.
pub struct AppSession {
    pub draft: DraftState,
    pub matrix: MatchupMatrix,
    pub weights: ModelWeights,
    pub cache_mgr: LocalCacheManager,
    /// Active bracket selection policy (`Auto` from OpenDota `rank_tier` or `Explicit` override).
    pub bracket_config: BracketConfig,
    config_path: Option<PathBuf>,
    pub db_status: String,
    pub last_gsi_state: String,
    pub vision_available: bool,
    pub vision_error: bool,
    pub vision_status: String,
    pub vision_detail: String,
    vision_backend: Option<truesight_adapters::vision::CaptureBackendKind>,
    last_runtime_health: Option<truesight_adapters::vision::CaptureRuntimeHealth>,
    pub ally_locked_slots: [bool; TEAM_SIZE],
    pub enemy_locked_slots: [bool; TEAM_SIZE],
    pub manually_dismissed: Vec<HeroId>,
    pub local_player_hero: Option<HeroId>,
    pub local_player_hero_from_gsi: bool,
    /// Optional allied hero explicitly clicked/selected for browsing item builds without altering local-player identity.
    pub build_view_hero: Option<HeroId>,
    pub sync_in_flight: bool,
    pending_item_build_refresh: bool,
    pub last_sync_notice: Option<String>,
    sync_generation: u64,
    sync_rx: Option<Receiver<SyncCompletion>>,
    sync_workers: Vec<std::thread::JoinHandle<()>>,
    sync_runner: BackgroundSyncRunner,
    last_rules_state: Option<String>,
    vision_reset_pending: bool,
    vision_slots: [[Option<HeroId>; TEAM_SIZE]; 2],
    vision_owned: Vec<(Side, HeroId)>,
    pub onboarding_dismissed: bool,
    pub stratz_api_token: Option<String>,
    pub stratz_token_exp: Option<u64>,
    pub pending_stratz_steam32_link: Option<u64>,
    pub item_repo: SeededItemMetaRepository,
    pub game_time_secs: i32,
    pub owned_items: Vec<ItemId>,
    /// Confirmed GSI owner of [`AppSession::owned_items`], kept across manual slot clears so fallback allies never inherit another hero's inventory.
    pub owned_items_owner: Option<HeroId>,
    pub selected_build_variants: Vec<VariantSelection>,
    pub post_draft_view_override: Option<bool>,
    pub active_game_patch: String,
    pub overlay_mode: bool,
    pub auto_overlay_with_dota: bool,
    pub dota_process_running: bool,
    pub auto_overlay_dismissed: bool,
    pub overlay_geometry: OverlayGeometry,
    pub build_overlay_geometry: OverlayGeometry,
    pub overlay_opacity: f32,
    pub active_monitor_rect: Option<MonitorRect>,
    pub known_monitors: Vec<MonitorRect>,
    pub active_capture_strip_rect: Option<(f32, f32, f32, f32)>,
    pub ui_language: UiLanguage,
    pub pass_hotkey: PassHotkey,
}

impl Drop for AppSession {
    fn drop(&mut self) {
        let _ = self.cache_mgr.cancel_in_flight();
        self.reap_finished_sync_workers();
    }
}

fn hero_role_affinity(matrix: &MatchupMatrix, hero_id: HeroId, role: Role) -> f32 {
    if let Some(h) = matrix.hero(hero_id) {
        let role_bonus = h
            .primary_roles
            .iter()
            .position(|&r| r == role)
            .map(|pos| 100.0 - (pos as f32) * 8.0)
            .unwrap_or(0.0);
        role_bonus + h.meta_winrate(role) * 10.0
    } else {
        5.0
    }
}

fn normalized_inventory_signature(items: &[ItemId]) -> Vec<u16> {
    let mut ids: Vec<u16> = items.iter().map(|id| id.0).collect();
    ids.sort_unstable();
    ids
}

struct RoleAssignmentSearch<'a> {
    matrix: &'a MatchupMatrix,
    heroes: &'a [HeroId],
    available_slots: &'a [usize],
    current_slots: Vec<usize>,
    best_score: f32,
    best_slots: Vec<usize>,
}

impl RoleAssignmentSearch<'_> {
    fn search(&mut self, hero_idx: usize, used_mask: u8, current_score: f32) {
        if hero_idx == self.heroes.len() {
            if current_score > self.best_score + 1e-4 {
                self.best_score = current_score;
                self.best_slots = self.current_slots.clone();
            }
            return;
        }
        for (pos, &slot) in self.available_slots.iter().enumerate() {
            if (used_mask & (1 << pos)) != 0 {
                continue;
            }
            let aff = hero_role_affinity(self.matrix, self.heroes[hero_idx], Role::ALL[slot]);
            self.current_slots.push(slot);
            self.search(hero_idx + 1, used_mask | (1 << pos), current_score + aff);
            self.current_slots.pop();
        }
    }
}

impl AppSession {
    pub fn new() -> Result<Self, String> {
        Self::from_cache_and_config(LocalCacheManager::auto_detected(), None, None)
    }

    /// Creates a session backed by explicit cache/config paths (used by integration tests).
    pub fn with_cache_and_config(
        cache_mgr: LocalCacheManager,
        config_path: Option<PathBuf>,
    ) -> Result<Self, String> {
        Self::from_cache_and_config(cache_mgr, config_path.clone(), config_path)
    }

    fn from_cache_and_config(
        mut cache_mgr: LocalCacheManager,
        read_config_path: Option<PathBuf>,
        write_config_path: Option<PathBuf>,
    ) -> Result<Self, String> {
        let app_cfg = read_config_path
            .as_deref()
            .map(load_app_config_from)
            .unwrap_or_else(load_app_config);
        let initial_bracket = match app_cfg.bracket {
            BracketConfig::Explicit(b) => b,
            BracketConfig::Auto => cache_mgr
                .read_cached_player_bracket()
                .unwrap_or(Bracket::Legend),
        };
        let repo = cache_mgr.build_repository();
        let matrix = repo
            .load_matrix(initial_bracket)
            .map_err(|e| e.to_string())?;
        let item_repo = cache_mgr.build_item_repository();
        let active_game_patch = cache_mgr
            .read_cached_game_patch()
            .unwrap_or_else(|| counter_trait_table_patch().to_string());
        let ui_cfg = read_config_path
            .as_deref()
            .and_then(Path::parent)
            .map(load_ui_config_from_dir)
            .unwrap_or_default();
        let now = LocalCacheManager::now_unix();
        let resolved_exp =
            resolve_stratz_exp(ui_cfg.stratz_api_token.as_deref(), ui_cfg.stratz_token_exp);
        if !is_stratz_exp_expired(resolved_exp, now) {
            cache_mgr.stratz_api_token = ui_cfg.stratz_api_token.clone();
            cache_mgr.stratz_token_exp = resolved_exp;
        }
        let db_status = cache_mgr.freshness_badge(now);
        Ok(Self {
            draft: DraftState {
                bracket: initial_bracket,
                ..DraftState::default()
            },
            matrix,
            weights: ModelWeights::default(),
            cache_mgr,
            bracket_config: app_cfg.bracket,
            config_path: write_config_path,
            db_status,
            last_gsi_state: "IDLE (Waiting for Dota 2 GSI)".into(),
            vision_available: false,
            vision_error: false,
            vision_status: "CAP: MANUAL".into(),
            vision_detail: "Manual pick mode (screen capture inactive).".into(),
            vision_backend: None,
            last_runtime_health: None,
            ally_locked_slots: [false; TEAM_SIZE],
            enemy_locked_slots: [false; TEAM_SIZE],
            manually_dismissed: Vec::new(),
            local_player_hero: None,
            local_player_hero_from_gsi: false,
            build_view_hero: None,
            sync_in_flight: false,
            pending_item_build_refresh: false,
            last_sync_notice: None,
            sync_generation: 0,
            sync_rx: None,
            sync_workers: Vec::new(),
            sync_runner: Arc::new(|mgr, force| {
                mgr.sync_from_opendota(force).map_err(|e| e.to_string())
            }),
            last_rules_state: None,
            vision_reset_pending: false,
            vision_slots: [[None; TEAM_SIZE]; 2],
            vision_owned: Vec::new(),
            onboarding_dismissed: ui_cfg.onboarding_dismissed,
            stratz_api_token: ui_cfg.stratz_api_token,
            stratz_token_exp: resolved_exp,
            pending_stratz_steam32_link: None,
            item_repo,
            game_time_secs: 0,
            owned_items: Vec::new(),
            owned_items_owner: None,
            selected_build_variants: Vec::new(),
            post_draft_view_override: None,
            active_game_patch,
            overlay_mode: ui_cfg.overlay_mode,
            auto_overlay_with_dota: ui_cfg.auto_overlay_with_dota,
            dota_process_running: false,
            auto_overlay_dismissed: false,
            overlay_geometry: ui_cfg.overlay_geometry,
            build_overlay_geometry: ui_cfg.build_overlay_geometry,
            overlay_opacity: Self::sanitize_overlay_opacity(ui_cfg.overlay_opacity),
            active_monitor_rect: None,
            known_monitors: Vec::new(),
            active_capture_strip_rect: None,
            ui_language: ui_cfg.ui_language,
            pass_hotkey: ui_cfg.pass_hotkey,
        })
    }

    /// Enables saving manual bracket and UI config changes to the OS config directory.
    pub fn enable_default_config_persistence(&mut self) {
        self.enable_config_persistence_for_path(config_file_path());
    }

    pub(crate) fn enable_config_persistence_for_path(&mut self, cfg_path: PathBuf) {
        if let Some(dir) = cfg_path.parent() {
            let ui_cfg = load_ui_config_from_dir(dir);
            let now = LocalCacheManager::now_unix();
            let resolved_exp =
                resolve_stratz_exp(ui_cfg.stratz_api_token.as_deref(), ui_cfg.stratz_token_exp);
            self.onboarding_dismissed = ui_cfg.onboarding_dismissed;
            self.stratz_api_token = ui_cfg.stratz_api_token;
            self.stratz_token_exp = resolved_exp;
            self.overlay_mode = ui_cfg.overlay_mode;
            self.auto_overlay_with_dota = ui_cfg.auto_overlay_with_dota;
            self.overlay_geometry = ui_cfg.overlay_geometry;
            self.build_overlay_geometry = ui_cfg.build_overlay_geometry;
            self.overlay_opacity = Self::sanitize_overlay_opacity(ui_cfg.overlay_opacity);
            self.ui_language = ui_cfg.ui_language;
            self.pass_hotkey = ui_cfg.pass_hotkey;
            if is_stratz_exp_expired(resolved_exp, now) {
                self.cache_mgr.stratz_api_token = None;
                self.cache_mgr.stratz_token_exp = None;
            } else {
                self.cache_mgr.stratz_api_token = self.stratz_api_token.clone();
                self.cache_mgr.stratz_token_exp = resolved_exp;
            }
        }
        self.config_path = Some(cfg_path);
    }

    #[cfg(test)]
    pub(crate) fn set_background_sync_runner<F>(&mut self, runner: F)
    where
        F: Fn(LocalCacheManager, bool) -> Result<String, String> + Send + Sync + 'static,
    {
        self.sync_runner = Arc::new(runner);
    }

    fn reap_finished_sync_workers(&mut self) {
        let mut active = Vec::with_capacity(self.sync_workers.len());
        for handle in self.sync_workers.drain(..) {
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                active.push(handle);
            }
        }
        self.sync_workers = active;
    }

    pub fn set_bracket(&mut self, bracket: Bracket) {
        self.bracket_config = BracketConfig::Explicit(bracket);
        self.draft.bracket = bracket;
        if let Ok(m) = self.cache_mgr.build_repository().load_matrix(bracket) {
            self.matrix = m;
        }
        if let Some(path) = self.config_path.as_deref() {
            let _ = save_bracket_config_to(path, BracketConfig::Explicit(bracket));
        }
    }

    /// Restores automatic bracket selection from the linked account's OpenDota `rank_tier`.
    pub fn set_bracket_auto(&mut self) {
        self.bracket_config = BracketConfig::Auto;
        let next = self
            .cache_mgr
            .read_cached_player_bracket()
            .unwrap_or(Bracket::Legend);
        self.draft.bracket = next;
        if let Ok(m) = self.cache_mgr.build_repository().load_matrix(next) {
            self.matrix = m;
        }
        if let Some(path) = self.config_path.as_deref() {
            let _ = save_bracket_config_to(path, BracketConfig::Auto);
        }
    }

    fn apply_auto_bracket_from_cache(&mut self) -> Option<Bracket> {
        if self.bracket_config != BracketConfig::Auto {
            return None;
        }
        let detected = self.cache_mgr.read_cached_player_bracket()?;
        if detected != self.draft.bracket {
            self.draft.bracket = detected;
            return Some(detected);
        }
        None
    }

    /// Updates UI/CLI capture capability indicators from the platform capture factory.
    pub fn set_vision_availability(&mut self, availability: &CaptureAvailability) {
        self.vision_available = availability.is_available();
        self.vision_error = false;
        self.vision_status = availability.badge_label().to_string();
        self.vision_detail = availability.summary().to_string();
        self.vision_backend = availability.is_available().then(|| availability.backend());
        self.last_runtime_health = None;
    }

    fn apply_runtime_health(
        &mut self,
        health: truesight_adapters::vision::CaptureRuntimeHealth,
    ) -> bool {
        if self.last_runtime_health.as_ref() == Some(&health) {
            return false;
        }
        let backend = self
            .vision_backend
            .unwrap_or(if cfg!(target_os = "windows") {
                truesight_adapters::vision::CaptureBackendKind::WindowsNative
            } else if cfg!(target_os = "linux") {
                truesight_adapters::vision::CaptureBackendKind::X11
            } else {
                truesight_adapters::vision::CaptureBackendKind::Unsupported
            });
        let next_available = health.is_capturing();
        let next_error = health.is_error();
        let next_status = health.status_badge(backend);
        let next_detail = health.summary();
        let visual_changed = self.vision_available != next_available
            || self.vision_error != next_error
            || self.vision_status != next_status
            || self.vision_detail != next_detail;
        self.vision_available = next_available;
        self.vision_error = next_error;
        self.vision_status = next_status;
        self.vision_detail = next_detail;
        self.last_runtime_health = Some(health);
        visual_changed
    }

    pub fn reload_repository(&mut self) {
        if let Ok(m) = self
            .cache_mgr
            .build_repository()
            .load_matrix(self.draft.bracket)
        {
            self.matrix = m;
        }
        self.item_repo = self.cache_mgr.build_item_repository();
        self.active_game_patch = self
            .cache_mgr
            .read_cached_game_patch()
            .unwrap_or_else(|| counter_trait_table_patch().to_string());
        self.db_status = self
            .cache_mgr
            .freshness_badge(LocalCacheManager::now_unix());
    }

    fn apply_account_transition(&mut self, prev_account: Option<u64>) {
        if self.cache_mgr.player_account_id != prev_account {
            self.sync_generation = self.cache_mgr.cancel_in_flight();
            self.sync_in_flight = false;
            self.sync_rx = None;
            self.reap_finished_sync_workers();
            if self.bracket_config == BracketConfig::Auto {
                self.draft.bracket = self
                    .cache_mgr
                    .read_cached_player_bracket()
                    .unwrap_or(Bracket::Legend);
            }
            self.reload_repository();
            if self.pending_item_build_refresh {
                if self.is_post_draft_build_view() {
                    let _ = self.request_item_build_refresh_if_uncached();
                } else {
                    self.pending_item_build_refresh = false;
                }
            }
        } else if !self.sync_in_flight {
            let _ = self.apply_auto_bracket_from_cache();
            self.reload_repository();
        }
    }

    pub fn set_account_id(&mut self, account_id: Option<u64>, explicit: bool) {
        let prev_account = self.cache_mgr.player_account_id;
        self.cache_mgr.set_account_id(account_id, explicit);
        self.apply_account_transition(prev_account);
    }

    pub fn set_account_auto(&mut self) {
        let prev_account = self.cache_mgr.player_account_id;
        self.cache_mgr.set_auto_account();
        self.apply_account_transition(prev_account);
    }

    pub fn start_background_sync_with<F>(&mut self, force: bool, task: F) -> bool
    where
        F: FnOnce(LocalCacheManager, bool) -> Result<String, String> + Send + 'static,
    {
        if self.sync_in_flight {
            return false;
        }
        self.sync_target_item_heroes_to_cache();
        self.reap_finished_sync_workers();
        let (generation, _) = self.cache_mgr.begin_sync_generation();
        self.sync_generation = generation;
        let account_id = self.cache_mgr.player_account_id;
        let bg_mgr = self.cache_mgr.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("truesight-sync-worker".into())
            .spawn(move || {
                let result = task(bg_mgr, force);
                let _ = tx.send(SyncCompletion {
                    generation,
                    account_id,
                    result,
                });
            }) {
            Ok(handle) => {
                self.sync_in_flight = true;
                self.sync_rx = Some(rx);
                self.sync_workers.push(handle);
                self.db_status = "DB [SYNCING...]".into();
                true
            }
            Err(_) => false,
        }
    }

    pub fn start_background_sync(&mut self, force: bool) -> bool {
        let runner = Arc::clone(&self.sync_runner);
        self.start_background_sync_with(force, move |mgr, force| runner(mgr, force))
    }

    /// Starts a non-blocking background sync (`force = false`) when a linked player's cache is
    /// missing or past the 1h TTL, preserving any fresh Meta cache.
    pub fn sync_if_player_stale(&mut self) -> bool {
        let now = LocalCacheManager::now_unix();
        if self.cache_mgr.is_player_stale(now) {
            self.start_background_sync(false)
        } else {
            false
        }
    }

    pub fn poll_sync_completion(&mut self) -> bool {
        let outcome = match self.sync_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(done)) => Some(Ok(done)),
            Some(Err(TryRecvError::Disconnected)) => Some(Err(())),
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        let Some(outcome) = outcome else {
            return false;
        };

        self.sync_in_flight = false;
        self.sync_rx = None;
        self.reap_finished_sync_workers();

        let Ok(done) = outcome else {
            self.db_status = format!(
                "{} · sync failed",
                self.cache_mgr
                    .freshness_badge(LocalCacheManager::now_unix())
            );
            self.drain_pending_item_build_refresh();
            return true;
        };

        if done.generation != self.sync_generation
            || done.account_id != self.cache_mgr.player_account_id
        {
            self.drain_pending_item_build_refresh();
            return true;
        }

        match done.result {
            Ok(msg) => {
                let auto_bracket = self.apply_auto_bracket_from_cache();
                let loaded = self
                    .cache_mgr
                    .build_repository()
                    .load_matrix(self.draft.bracket);
                match loaded {
                    Ok(m) if m.heroes().count() > 0 => {
                        self.matrix = m;
                        self.item_repo = self.cache_mgr.build_item_repository();
                        self.active_game_patch = self
                            .cache_mgr
                            .read_cached_game_patch()
                            .unwrap_or_else(|| counter_trait_table_patch().to_string());
                        self.db_status = self
                            .cache_mgr
                            .freshness_badge(LocalCacheManager::now_unix());
                        self.last_sync_notice = Some(match auto_bracket {
                            Some(b) => format!("{msg} · Bracket: {}", b.label()),
                            None => msg,
                        });
                    }
                    _ => {
                        self.db_status = format!(
                            "{} · reload failed",
                            self.cache_mgr
                                .freshness_badge(LocalCacheManager::now_unix())
                        );
                        self.last_sync_notice = Some(
                            "Sync notice: refreshed cache failed validation (using last valid model)"
                                .into(),
                        );
                    }
                }
            }
            Err(e) => {
                self.db_status = format!(
                    "{} · sync failed",
                    self.cache_mgr
                        .freshness_badge(LocalCacheManager::now_unix())
                );
                self.last_sync_notice = Some(format!("Sync notice: {e} (using last valid model)"));
            }
        }
        self.drain_pending_item_build_refresh();
        true
    }

    fn drain_pending_item_build_refresh(&mut self) {
        if self.pending_item_build_refresh {
            self.pending_item_build_refresh = false;
            if self.is_post_draft_build_view() {
                let _ = self.request_item_build_refresh_if_uncached();
            }
        }
    }

    pub fn sync_db(&mut self, force: bool) -> Result<String, String> {
        self.sync_target_item_heroes_to_cache();
        let (generation, _) = self.cache_mgr.begin_sync_generation();
        self.sync_generation = generation;
        let msg = self
            .cache_mgr
            .sync_from_opendota(force)
            .map_err(|e| e.to_string())?;
        let auto_bracket = self.apply_auto_bracket_from_cache();
        if let Ok(m) = self
            .cache_mgr
            .build_repository()
            .load_matrix(self.draft.bracket)
        {
            self.matrix = m;
        }
        self.item_repo = self.cache_mgr.build_item_repository();
        self.active_game_patch = self
            .cache_mgr
            .read_cached_game_patch()
            .unwrap_or_else(|| counter_trait_table_patch().to_string());
        self.db_status = self
            .cache_mgr
            .freshness_badge(LocalCacheManager::now_unix());
        Ok(match auto_bracket {
            Some(b) => format!("{msg} · Bracket: {}", b.label()),
            None => msg,
        })
    }

    /// Returns available heroes for `(side, slot)` matching `query`, ordered with prefix
    /// matches first and then alphabetical substring matches.
    pub fn filter_heroes_for_slot(
        &self,
        side: Side,
        slot: usize,
        query: &str,
    ) -> Vec<(HeroId, String)> {
        let q = query.trim().to_ascii_lowercase();
        let same_side_picks = match side {
            Side::Allies => &self.draft.allies,
            Side::Enemies => &self.draft.enemies,
        };
        let _ = slot;

        let mut matches: Vec<(bool, HeroId, String)> = self
            .matrix
            .heroes()
            .filter_map(|h| {
                let on_same_side = same_side_picks.iter().flatten().any(|p| p.hero_id == h.id);
                if self.draft.is_unavailable(h.id) && !on_same_side {
                    return None;
                }
                if q.is_empty() {
                    return Some((true, h.id, h.display_name.clone()));
                }
                let name_lower = h.display_name.to_ascii_lowercase();
                let slug_short = h
                    .slug
                    .strip_prefix("npc_dota_hero_")
                    .unwrap_or(&h.slug)
                    .to_ascii_lowercase();
                if name_lower.starts_with(&q) || slug_short.starts_with(&q) {
                    Some((true, h.id, h.display_name.clone()))
                } else if name_lower.contains(&q) || slug_short.contains(&q) {
                    Some((false, h.id, h.display_name.clone()))
                } else {
                    None
                }
            })
            .collect();

        matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
        matches
            .into_iter()
            .map(|(_, id, name)| (id, name))
            .collect()
    }

    pub fn find_hero(&self, query: &str) -> Option<HeroId> {
        let q_raw = query.trim().to_ascii_lowercase();
        let q = q_raw
            .strip_prefix("npc_dota_hero_")
            .unwrap_or(&q_raw)
            .to_string();
        if q.is_empty() {
            return None;
        }
        if let Ok(num) = q.parse::<u16>() {
            if self.matrix.hero(HeroId(num)).is_some() {
                return Some(HeroId(num));
            }
        }
        // Exact prefix or substring match on display_name or slug
        self.matrix
            .heroes()
            .find(|h| {
                let name = h.display_name.to_ascii_lowercase();
                let slug = h
                    .slug
                    .strip_prefix("npc_dota_hero_")
                    .unwrap_or(&h.slug)
                    .to_ascii_lowercase();
                name == q || slug == q || name.starts_with(&q) || slug.starts_with(&q)
            })
            .or_else(|| {
                self.matrix
                    .heroes()
                    .find(|h| h.display_name.to_ascii_lowercase().contains(&q))
            })
            .map(|h| h.id)
    }

    pub fn reset_draft(&mut self) {
        self.draft.clear();
        self.ally_locked_slots = [false; TEAM_SIZE];
        self.enemy_locked_slots = [false; TEAM_SIZE];
        self.manually_dismissed.clear();
        self.local_player_hero = None;
        self.local_player_hero_from_gsi = false;
        self.build_view_hero = None;
        self.pending_item_build_refresh = false;
        self.vision_reset_pending = true;
        self.vision_slots = [[None; TEAM_SIZE]; 2];
        self.vision_owned.clear();
        self.game_time_secs = 0;
        self.owned_items.clear();
        self.owned_items_owner = None;
        self.selected_build_variants.clear();
        self.post_draft_view_override = None;
        self.sync_target_item_heroes_to_cache();
    }

    fn side_picks(&self, side: Side) -> &[Option<DraftPick>; TEAM_SIZE] {
        match side {
            Side::Allies => &self.draft.allies,
            Side::Enemies => &self.draft.enemies,
        }
    }

    fn side_picks_mut(&mut self, side: Side) -> &mut [Option<DraftPick>; TEAM_SIZE] {
        match side {
            Side::Allies => &mut self.draft.allies,
            Side::Enemies => &mut self.draft.enemies,
        }
    }

    fn side_locked_mut(&mut self, side: Side) -> &mut [bool; TEAM_SIZE] {
        match side {
            Side::Allies => &mut self.ally_locked_slots,
            Side::Enemies => &mut self.enemy_locked_slots,
        }
    }

    fn sync_local_player_target_role(&mut self) {
        let Some(local_id) = self.local_player_hero else {
            return;
        };
        if let Some(slot) = self
            .draft
            .allies
            .iter()
            .position(|p| p.map(|d| d.hero_id) == Some(local_id))
        {
            if let Some(role) = Role::ALL.get(slot).copied() {
                self.draft.target_role = role;
            }
        }
    }

    fn rebalance_unlocked_side_roles(&mut self, side: Side) {
        let locked = match side {
            Side::Allies => self.ally_locked_slots,
            Side::Enemies => self.enemy_locked_slots,
        };
        let unlocked_slots: Vec<usize> = (0..TEAM_SIZE).filter(|&s| !locked[s]).collect();
        let unlocked_heroes: Vec<HeroId> = unlocked_slots
            .iter()
            .filter_map(|&s| self.side_picks(side)[s].map(|p| p.hero_id))
            .collect();

        if unlocked_heroes.is_empty() {
            for &s in &unlocked_slots {
                self.side_picks_mut(side)[s] = None;
            }
            self.sync_local_player_target_role();
            return;
        }

        let mut search = RoleAssignmentSearch {
            matrix: &self.matrix,
            heroes: &unlocked_heroes,
            available_slots: &unlocked_slots,
            current_slots: Vec::with_capacity(unlocked_heroes.len()),
            best_score: f32::NEG_INFINITY,
            best_slots: Vec::with_capacity(unlocked_heroes.len()),
        };
        search.search(0, 0, 0.0);
        let best_slots = search.best_slots;

        let picks = self.side_picks_mut(side);
        for &s in &unlocked_slots {
            picks[s] = None;
        }
        for (idx, &slot) in best_slots.iter().enumerate() {
            picks[slot] = Some(DraftPick {
                hero_id: unlocked_heroes[idx],
                role: Role::ALL.get(slot).copied(),
            });
        }
        self.sync_local_player_target_role();
    }

    /// Swaps two role slots on `side` and locks any occupied slots against vision re-ordering.
    pub fn swap_slots(&mut self, side: Side, slot_a: usize, slot_b: usize) {
        if slot_a >= TEAM_SIZE || slot_b >= TEAM_SIZE || slot_a == slot_b {
            return;
        }
        let prev_active_hero = self.active_build_hero();
        let (pick_a, pick_b) = {
            let picks = self.side_picks_mut(side);
            picks.swap(slot_a, slot_b);
            (
                picks[slot_a].map(|p| DraftPick {
                    hero_id: p.hero_id,
                    role: Role::ALL.get(slot_a).copied(),
                }),
                picks[slot_b].map(|p| DraftPick {
                    hero_id: p.hero_id,
                    role: Role::ALL.get(slot_b).copied(),
                }),
            )
        };
        let picks = self.side_picks_mut(side);
        picks[slot_a] = pick_a;
        picks[slot_b] = pick_b;
        let locked = self.side_locked_mut(side);
        locked[slot_a] = pick_a.is_some();
        locked[slot_b] = pick_b.is_some();
        self.sync_local_player_target_role();
        self.sync_target_item_heroes_to_cache();
        self.refresh_item_build_if_hero_changed(prev_active_hero);
    }

    fn remove_hero_from_opposite_and_bans(&mut self, side: Side, hero_id: HeroId) {
        let opposite = match side {
            Side::Allies => Side::Enemies,
            Side::Enemies => Side::Allies,
        };
        for idx in 0..TEAM_SIZE {
            if self.side_picks(opposite)[idx].map(|p| p.hero_id) == Some(hero_id) {
                self.side_picks_mut(opposite)[idx] = None;
                self.side_locked_mut(opposite)[idx] = false;
                if opposite == Side::Allies && self.build_view_hero == Some(hero_id) {
                    self.build_view_hero = None;
                }
            }
        }
        for b in &mut self.draft.bans {
            if *b == Some(hero_id) {
                *b = None;
            }
        }
    }

    /// Manually assigns `hero_id` to `(side, slot)` and locks it; swaps if already on the same side.
    pub fn set_slot_manual(&mut self, side: Side, slot: usize, hero_id: HeroId) {
        if slot >= TEAM_SIZE {
            return;
        }
        let prev_active_hero = self.active_build_hero();
        self.manually_dismissed.retain(|&id| id != hero_id);
        if let Some(existing_slot) = self
            .side_picks(side)
            .iter()
            .position(|p| p.map(|d| d.hero_id) == Some(hero_id))
        {
            if existing_slot == slot {
                self.side_locked_mut(side)[slot] = true;
            } else {
                self.swap_slots(side, existing_slot, slot);
            }
            return;
        }

        self.remove_hero_from_opposite_and_bans(side, hero_id);

        let replaced_hero = self.side_picks(side)[slot].map(|p| p.hero_id);
        let pick = Some(DraftPick {
            hero_id,
            role: Role::ALL.get(slot).copied(),
        });
        self.side_picks_mut(side)[slot] = pick;
        self.side_locked_mut(side)[slot] = true;
        if side == Side::Allies && replaced_hero.is_some() {
            if !self.local_player_hero_from_gsi && self.local_player_hero == replaced_hero {
                self.local_player_hero = Some(hero_id);
            }
            if self.build_view_hero == replaced_hero {
                self.build_view_hero = Some(hero_id);
            }
        }
        self.sync_local_player_target_role();
        self.sync_target_item_heroes_to_cache();
        self.refresh_item_build_if_hero_changed(prev_active_hero);
    }

    /// Clears `(side, slot)`, unlocks the slot, and suppresses vision re-adding that hero until reset.
    pub fn clear_slot_manual(&mut self, side: Side, slot: usize) {
        if slot >= TEAM_SIZE {
            return;
        }
        let prev_active_hero = self.active_build_hero();
        let old = self.side_picks_mut(side)[slot].take();
        self.side_locked_mut(side)[slot] = false;
        if let Some(pick) = old {
            if !self.manually_dismissed.contains(&pick.hero_id) {
                self.manually_dismissed.push(pick.hero_id);
            }
            if side == Side::Allies {
                if self.local_player_hero == Some(pick.hero_id) {
                    self.local_player_hero = None;
                    self.local_player_hero_from_gsi = false;
                }
                if self.build_view_hero == Some(pick.hero_id) {
                    self.build_view_hero = None;
                }
            }
        }
        self.sync_target_item_heroes_to_cache();
        self.refresh_item_build_if_hero_changed(prev_active_hero);
    }

    pub fn apply_event(&mut self, event: DraftEvent) {
        match event {
            DraftEvent::GameRulesStateChanged {
                state_name,
                game_time_secs,
            } => {
                let was_build_view = self.is_post_draft_build_view();
                self.last_gsi_state = format!("{state_name} (t={game_time_secs}s)");
                let entered_wait_for_players = state_name
                    == "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD"
                    && self.last_rules_state.as_deref() != Some(state_name.as_str());
                let entered_new_hero_selection = state_name
                    == "DOTA_GAMERULES_STATE_HERO_SELECTION"
                    && matches!(
                        self.last_rules_state.as_deref(),
                        Some(prev)
                            if !matches!(
                                prev,
                                "DOTA_GAMERULES_STATE_HERO_SELECTION"
                                    | "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD"
                            )
                    );
                if entered_wait_for_players || entered_new_hero_selection {
                    self.reset_draft();
                }
                self.game_time_secs = game_time_secs;
                self.last_rules_state = Some(state_name);
                if !was_build_view && self.is_post_draft_build_view() {
                    let _ = self.request_item_build_refresh_if_uncached();
                }
            }
            DraftEvent::FactionDetected { faction } => {
                self.set_ally_faction(faction);
            }
            DraftEvent::PlayerSteamIdDetected { steam32 } => {
                if !self.cache_mgr.explicit_override
                    && self.cache_mgr.player_account_id != Some(steam32)
                {
                    self.set_account_id(Some(steam32), false);
                    let _ = self.sync_if_player_stale();
                }
            }
            DraftEvent::LocalPlayerHeroDetected { hero_slug } => {
                if let Some(id) = self.find_hero(&hero_slug) {
                    self.apply_local_hero_identity(id, true);
                }
            }
            DraftEvent::PlayerHeroMapped {
                steam32,
                faction,
                hero_slug,
            } => {
                if self.cache_mgr.player_account_id == Some(steam32) {
                    self.set_ally_faction(faction);
                    if let Some(id) = self.find_hero(&hero_slug) {
                        self.apply_local_hero_identity(id, false);
                    }
                }
            }
            DraftEvent::PickDetected {
                side,
                slot,
                hero_id,
                role,
            } => {
                self.vision_owned.retain(|&(_, id)| id != hero_id);
                self.apply_side_pick(side, slot, hero_id, role);
            }
            DraftEvent::BanDetected { hero_id } => {
                if !self.draft.is_unavailable(hero_id) {
                    let _ = self.draft.toggle_ban(hero_id);
                }
            }
            DraftEvent::InventoryUpdated {
                hero_slug,
                item_slugs,
            } => {
                let explicit_owner = match hero_slug.as_deref() {
                    Some(slug) => match self.find_hero(slug) {
                        Some(id) => Some(id),
                        None => return,
                    },
                    None => None,
                };
                if let (Some(local_id), Some(inv_owner)) = (self.local_player_hero, explicit_owner)
                    && local_id != inv_owner
                {
                    return;
                }
                if let Some(owner_id) = explicit_owner.or(self.local_player_hero) {
                    self.owned_items_owner = Some(owner_id);
                }
                let new_items: Vec<ItemId> = item_slugs
                    .iter()
                    .filter_map(|slug| self.item_repo.item_by_slug(slug).map(|e| e.id))
                    .collect();
                let inventory_changed = normalized_inventory_signature(&new_items)
                    != normalized_inventory_signature(&self.owned_items);
                self.owned_items = new_items;
                if inventory_changed {
                    self.build_view_hero = None;
                }
            }
            DraftEvent::DraftReset => {
                self.reset_draft();
            }
            _ => {}
        }
    }

    fn apply_side_pick(
        &mut self,
        side: Side,
        _slot: usize,
        hero_id: HeroId,
        _fallback_role: Option<Role>,
    ) {
        if self.manually_dismissed.contains(&hero_id) {
            return;
        }
        if self
            .side_picks(side)
            .iter()
            .flatten()
            .any(|p| p.hero_id == hero_id)
        {
            return;
        }

        self.remove_hero_from_opposite_and_bans(side, hero_id);

        let locked = match side {
            Side::Allies => self.ally_locked_slots,
            Side::Enemies => self.enemy_locked_slots,
        };
        let picks = self.side_picks(side);
        let empty_slot = (0..TEAM_SIZE)
            .find(|&idx| !locked[idx] && picks[idx].is_none())
            .or_else(|| (0..TEAM_SIZE).find(|&idx| picks[idx].is_none()));

        if let Some(dst) = empty_slot {
            self.side_picks_mut(side)[dst] = Some(DraftPick {
                hero_id,
                role: Role::ALL.get(dst).copied(),
            });
            self.rebalance_unlocked_side_roles(side);
            self.sync_target_item_heroes_to_cache();
        }
    }

    // Reconcile a whole poll before removing identities: a hero can move to another
    // physical portrait in the same batch, including when all role slots are full.
    // Missing events are not removals (death/occlusion grace belongs to the adapter).
    pub(crate) fn apply_vision_events(&mut self, events: Vec<DraftEvent>) {
        let prev_active_hero = self.active_build_hero();
        // Cached observations support retirement only; replaying them as additions
        // would undo manual replacements and subsequent GSI side corrections.
        let mut fresh_slots = [[None; TEAM_SIZE]; 2];
        for event in events {
            if let DraftEvent::PickDetected {
                side,
                slot,
                hero_id,
                ..
            } = event
            {
                if slot < TEAM_SIZE {
                    let index = usize::from(side == Side::Enemies);
                    self.vision_slots[index][slot] = Some(hero_id);
                    fresh_slots[index][slot] = Some(hero_id);
                }
            }
        }
        let previous_picks = [self.draft.allies, self.draft.enemies];
        for (index, side) in [Side::Allies, Side::Enemies].into_iter().enumerate() {
            let observed = self.vision_slots[index];
            let locked = match side {
                Side::Allies => self.ally_locked_slots,
                Side::Enemies => self.enemy_locked_slots,
            };
            for (slot, &is_locked) in locked.iter().enumerate() {
                if let Some(pick) = self.side_picks(side)[slot] {
                    if self.vision_owned.contains(&(side, pick.hero_id))
                        && !observed.contains(&Some(pick.hero_id))
                        && !is_locked
                        && self.local_player_hero != Some(pick.hero_id)
                    {
                        self.side_picks_mut(side)[slot] = None;
                        if side == Side::Allies && self.build_view_hero == Some(pick.hero_id) {
                            self.build_view_hero = None;
                        }
                    }
                }
            }
            self.vision_owned
                .retain(|&(owner_side, id)| owner_side != side || observed.contains(&Some(id)));
        }
        for (index, side) in [Side::Allies, Side::Enemies].into_iter().enumerate() {
            for hero_id in fresh_slots[index].into_iter().flatten() {
                if self
                    .side_picks(side)
                    .iter()
                    .flatten()
                    .any(|p| p.hero_id == hero_id)
                {
                    continue;
                }
                // Vision cannot move a user-locked hero or the authoritative local hero
                // from the opposite team. Ordinary automatic placement remains shared.
                let opposite = if side == Side::Allies {
                    Side::Enemies
                } else {
                    Side::Allies
                };
                let locked = match opposite {
                    Side::Allies => self.ally_locked_slots,
                    Side::Enemies => self.enemy_locked_slots,
                };
                if self
                    .side_picks(opposite)
                    .iter()
                    .enumerate()
                    .any(|(slot, pick)| {
                        pick.is_some_and(|p| p.hero_id == hero_id)
                            && (locked[slot] || self.local_player_hero == Some(hero_id))
                    })
                {
                    continue;
                }
                self.apply_side_pick(side, 0, hero_id, None);
                if self
                    .side_picks(side)
                    .iter()
                    .flatten()
                    .any(|p| p.hero_id == hero_id)
                    && !self.vision_owned.contains(&(side, hero_id))
                {
                    self.vision_owned.push((side, hero_id));
                }
            }
        }
        // Compare after both sides are processed: a pick can also remove a hero
        // from the opposite side, even if the destination has no free slot.
        for (side, previous) in [Side::Allies, Side::Enemies]
            .into_iter()
            .zip(previous_picks)
        {
            if self.side_picks(side) != &previous {
                self.rebalance_unlocked_side_roles(side);
            }
        }
        self.sync_target_item_heroes_to_cache();
        self.refresh_item_build_if_hero_changed(prev_active_hero);
    }

    pub fn poll_live_sources(
        &mut self,
        gsi_listener: Option<&mut BoundedGsiListener>,
        vision_worker: Option<&mut BoundedVisionWorker>,
    ) -> bool {
        let mut changed = self.poll_sync_completion();
        if let Some(listener) = gsi_listener {
            if let Ok(events) = listener.poll_events() {
                for ev in events {
                    self.apply_event(ev);
                    changed = true;
                }
            }
        }
        if let Some(worker) = vision_worker {
            if self.vision_reset_pending {
                worker.reset();
                self.vision_reset_pending = false;
                changed = true;
            }
            // Suppressed prevents new vision picks before hero selection / post-game while preserving
            // prior draft picks until WAIT_FOR_PLAYERS_TO_LOAD or HERO_SELECTION starts a new match.
            let capture_mode = match self.last_rules_state.as_deref() {
                Some(
                    "DOTA_GAMERULES_STATE_INIT"
                    | "DOTA_GAMERULES_STATE_WAIT_FOR_PLAYERS_TO_LOAD"
                    | "DOTA_GAMERULES_STATE_CUSTOM_GAME_SETUP"
                    | "DOTA_GAMERULES_STATE_POST_GAME"
                    | "DOTA_GAMERULES_STATE_DISCONNECT",
                ) => VisionCaptureMode::Suppressed,
                Some(
                    "DOTA_GAMERULES_STATE_HERO_SELECTION" | "DOTA_GAMERULES_STATE_STRATEGY_TIME",
                ) => VisionCaptureMode::DraftOnly,
                _ => VisionCaptureMode::Any,
            };
            worker.set_capture_mode(capture_mode);
            worker.set_ally_faction(self.draft.ally_faction);
            if let Ok(events) = worker.poll_events() {
                if capture_mode != VisionCaptureMode::Suppressed {
                    changed |= !events.is_empty();
                    self.apply_vision_events(events);
                }
            }
            if self.apply_runtime_health(worker.runtime_health()) {
                changed = true;
            }
            let next_strip = if capture_mode == VisionCaptureMode::Suppressed {
                None
            } else {
                worker.capture_strip_rect()
            };
            if self.active_capture_strip_rect != next_strip {
                self.active_capture_strip_rect = next_strip;
                changed = true;
            }
        }
        changed
    }

    /// Switches the user's faction and atomically swaps Allied/Enemy picks and slot locks.
    pub fn set_ally_faction(&mut self, faction: Faction) {
        if self.draft.ally_faction == faction {
            return;
        }
        let prev_active = self.active_build_hero();
        self.draft.ally_faction = faction;
        std::mem::swap(&mut self.draft.allies, &mut self.draft.enemies);
        std::mem::swap(&mut self.ally_locked_slots, &mut self.enemy_locked_slots);
        self.vision_slots.swap(0, 1);
        for (side, _) in &mut self.vision_owned {
            *side = if *side == Side::Allies {
                Side::Enemies
            } else {
                Side::Allies
            };
        }
        if let Some(browse_id) = self.build_view_hero
            && !self
                .draft
                .allies
                .iter()
                .flatten()
                .any(|p| p.hero_id == browse_id)
        {
            self.build_view_hero = None;
        }
        self.sync_local_player_target_role();
        self.sync_target_item_heroes_to_cache();
        self.refresh_item_build_if_hero_changed(prev_active);
    }

    /// Evaluates top enemy draft candidates for `enemy_role` using neutral comfort (`0.0`).
    pub fn recommend_enemy_heroes(
        &self,
        enemy_role: Role,
        top_n: usize,
    ) -> Vec<HeroRecommendation> {
        let mut enemy_draft = DraftState {
            ally_faction: self.draft.ally_faction.opposite(),
            allies: self.draft.enemies,
            enemies: self.draft.allies,
            bans: self.draft.bans,
            bracket: self.draft.bracket,
            target_role: enemy_role,
        };
        enemy_draft.target_role = enemy_role;
        let mut enemy_weights = self.weights;
        enemy_weights.comfort_weight = 0.0;
        let mut recs = recommend_heroes(&enemy_draft, &self.matrix, &enemy_weights, top_n);
        for rec in &mut recs {
            rec.comfort_delta = 0.0;
        }
        recs
    }

    pub fn evaluate(&self) -> (DraftAnalysis5v5, Vec<HeroRecommendation>) {
        let analysis = evaluate_draft_5v5(&self.draft, &self.matrix, &self.weights);
        let recs = recommend_heroes(&self.draft, &self.matrix, &self.weights, 6);
        (analysis, recs)
    }

    fn persist_ui_config(&mut self) -> Result<(), String> {
        if let Some(dir) = self.config_path.as_deref().and_then(Path::parent) {
            let cfg = UiConfig {
                onboarding_dismissed: self.onboarding_dismissed,
                stratz_api_token: self.stratz_api_token.clone(),
                stratz_token_exp: self.stratz_token_exp,
                overlay_mode: self.overlay_mode,
                auto_overlay_with_dota: self.auto_overlay_with_dota,
                overlay_geometry: self.overlay_geometry,
                build_overlay_geometry: self.build_overlay_geometry,
                overlay_opacity: self.overlay_opacity,
                ui_language: self.ui_language,
                pass_hotkey: self.pass_hotkey,
            };
            if let Err(err) = save_ui_config_to_dir(dir, &cfg) {
                self.last_sync_notice = Some(format!(
                    "Warning: failed to save ui_config.toml ({err}); changes apply to this session only."
                ));
                return Err(err);
            }
        }
        Ok(())
    }

    /// Returns `true` when the bottom-left panel should display the post-draft item build bar.
    pub fn is_post_draft_build_view(&self) -> bool {
        self.post_draft_view_override.unwrap_or(matches!(
            self.last_rules_state.as_deref(),
            Some("DOTA_GAMERULES_STATE_PRE_GAME" | "DOTA_GAMERULES_STATE_GAME_IN_PROGRESS")
        ))
    }

    fn invalidate_inventory_on_identity_change(&mut self, new_hero_id: HeroId) {
        if self.owned_items_owner != Some(new_hero_id)
            && (!self.owned_items.is_empty() || self.owned_items_owner.is_some())
        {
            self.owned_items.clear();
            self.owned_items_owner = None;
        }
    }

    fn apply_local_hero_identity(&mut self, id: HeroId, place_ally_pick: bool) {
        let hero_changed = self.local_player_hero != Some(id);
        self.local_player_hero = Some(id);
        self.local_player_hero_from_gsi = true;
        if hero_changed {
            self.invalidate_inventory_on_identity_change(id);
            self.build_view_hero = None;
        }
        if place_ally_pick {
            self.apply_side_pick(Side::Allies, 0, id, None);
        }
        self.sync_local_player_target_role();
        self.sync_target_item_heroes_to_cache();
        if hero_changed && self.is_post_draft_build_view() {
            let _ = self.request_item_build_refresh_if_uncached();
        }
    }

    fn refresh_item_build_if_hero_changed(&mut self, prev_active: Option<(HeroId, Role)>) {
        if self.is_post_draft_build_view() && self.active_build_hero() != prev_active {
            let _ = self.request_item_build_refresh_if_uncached();
        }
    }

    /// Requests a background item-build refresh when live persistence is configured and active
    /// target heroes lack cached item stats. If another sync is already in flight, keeps the
    /// request pending until `poll_sync_completion` drains it.
    pub fn request_item_build_refresh_if_uncached(&mut self) -> bool {
        self.sync_target_item_heroes_to_cache();
        if self.config_path.is_none() || !self.cache_mgr.is_target_item_heroes_uncached() {
            self.pending_item_build_refresh = false;
            return false;
        }
        if self.sync_in_flight {
            self.pending_item_build_refresh = true;
            return false;
        }
        self.pending_item_build_refresh = false;
        self.start_background_sync(false)
    }

    /// Selects an allied hero to inspect in the item build view without altering `local_player_hero`
    /// or vision replacement authority. Schedules an item refresh if already in Build view.
    pub fn select_build_view_hero(&mut self, hero_id: HeroId) {
        let changed = self.build_view_hero != Some(hero_id);
        self.build_view_hero = Some(hero_id);
        self.sync_target_item_heroes_to_cache();
        if changed && self.is_post_draft_build_view() {
            let _ = self.request_item_build_refresh_if_uncached();
        }
    }

    /// Toggles between the post-draft item build view and the suggested picks view, syncing
    /// target hero item stats in the background when live persistence is active and uncached.
    pub fn toggle_post_draft_build_view(&mut self) {
        let next = !self.is_post_draft_build_view();
        self.post_draft_view_override = Some(next);
        if next {
            let _ = self.request_item_build_refresh_if_uncached();
        } else {
            self.pending_item_build_refresh = false;
        }
    }

    /// Returns `true` when the active game patch differs from the embedded counter-trait table patch.
    pub fn is_counter_table_patch_stale(&self) -> bool {
        let patch = self.active_game_patch.trim();
        !patch.is_empty() && patch != counter_trait_table_patch()
    }

    /// Resolves the active allied hero and role for item build recommendations.
    pub fn active_build_hero(&self) -> Option<(HeroId, Role)> {
        if let Some(browse_id) = self.build_view_hero
            && let Some(found) = self
                .draft
                .allies
                .iter()
                .enumerate()
                .find_map(|(idx, slot)| {
                    slot.filter(|p| p.hero_id == browse_id)
                        .map(|p| (browse_id, p.role.unwrap_or(Role::ALL[idx])))
                })
        {
            return Some(found);
        }
        if let Some(local_id) = self.local_player_hero
            && let Some(found) = self
                .draft
                .allies
                .iter()
                .enumerate()
                .find_map(|(idx, slot)| {
                    slot.filter(|p| p.hero_id == local_id)
                        .map(|p| (local_id, p.role.unwrap_or(Role::ALL[idx])))
                })
        {
            return Some(found);
        }
        if let Some(pick) = self.draft.allies[self.draft.target_role.index()] {
            return Some((pick.hero_id, pick.role.unwrap_or(self.draft.target_role)));
        }
        self.draft
            .allies
            .iter()
            .enumerate()
            .find_map(|(idx, slot)| slot.map(|p| (p.hero_id, p.role.unwrap_or(Role::ALL[idx]))))
    }

    /// Returns the GSI inventory items belonging to [`AppSession::active_build_hero`], or `&[]`
    /// when browsing a teammate's build so local purchases never mark another hero's items owned.
    pub fn active_build_owned_items(&self) -> &[ItemId] {
        let Some((hero_id, _)) = self.active_build_hero() else {
            return &[];
        };
        let owner = self.owned_items_owner.or(self.local_player_hero);
        let owns_inventory = match owner {
            Some(owner_id) => owner_id == hero_id,
            // Without an identified inventory owner, allow fallback target inventory only when not browsing an explicit ally.
            None => self.build_view_hero.is_none(),
        };
        if owns_inventory {
            &self.owned_items
        } else {
            &[]
        }
    }

    fn is_direct_upgrade(&self, primary_item_id: ItemId, variant_item_id: ItemId) -> bool {
        self.item_repo
            .item_catalog(variant_item_id)
            .is_some_and(|entry| entry.components.contains(&primary_item_id))
    }

    /// Selects a variant item override keyed by the step's currently displayed primary item ID
    /// within the active `(hero_id, role)` build context.
    pub fn select_build_variant_for_item(
        &mut self,
        primary_item_id: ItemId,
        variant_item_id: ItemId,
    ) {
        let Some((hero_id, role)) = self.active_build_hero() else {
            return;
        };

        // If `primary_item_id` is currently the displayed variant of an existing override,
        // update that original entry (or remove it when switching back to the original primary).
        if let Some(pos) = self.selected_build_variants.iter().position(|entry| {
            entry.hero_id == hero_id
                && entry.role == role
                && entry.variant_item_id == primary_item_id
        }) {
            let orig_primary = self.selected_build_variants[pos].primary_item_id;
            if variant_item_id == orig_primary {
                self.selected_build_variants.remove(pos);
            } else if !self.is_direct_upgrade(primary_item_id, variant_item_id)
                && !self.is_direct_upgrade(orig_primary, variant_item_id)
            {
                self.selected_build_variants[pos].variant_item_id = variant_item_id;
            }
            return;
        }

        // Direct upgrades (e.g. Aghanim's Scepter -> Aghanim's Blessing) keep their child position
        // and preserve the parent step's owned state instead of swapping into the primary slot.
        if self.is_direct_upgrade(primary_item_id, variant_item_id) {
            return;
        }

        if let Some(pos) = self.selected_build_variants.iter().position(|entry| {
            entry.hero_id == hero_id
                && entry.role == role
                && entry.primary_item_id == primary_item_id
        }) {
            if variant_item_id == primary_item_id {
                self.selected_build_variants.remove(pos);
            } else {
                self.selected_build_variants[pos].variant_item_id = variant_item_id;
            }
        } else if variant_item_id != primary_item_id {
            self.selected_build_variants.push(VariantSelection {
                hero_id,
                role,
                primary_item_id,
                variant_item_id,
            });
        }
    }

    /// Selects a variant item override for a given build progression step index by resolving
    /// its current primary item ID and delegating to [`AppSession::select_build_variant_for_item`].
    pub fn select_build_variant(&mut self, step_index: usize, variant_item_id: ItemId) {
        if let Some(primary_item_id) = self
            .compute_recommended_build()
            .and_then(|build| build.steps.get(step_index).map(|s| s.item_id))
        {
            self.select_build_variant_for_item(primary_item_id, variant_item_id);
        }
    }

    /// Computes the recommended item build for the active allied hero, applying any selected variant swaps.
    pub fn compute_recommended_build(&self) -> Option<RecommendedBuild> {
        let (hero_id, role) = self.active_build_hero()?;
        let owned_items = self.active_build_owned_items();
        let enemy_ids: [Option<HeroId>; TEAM_SIZE] =
            self.draft.enemies.map(|slot| slot.map(|p| p.hero_id));
        let mut build = recommend_item_build(
            &self.item_repo,
            hero_id,
            role,
            self.draft.bracket,
            &enemy_ids,
            owned_items,
            self.game_time_secs,
        )
        .ok()?;

        for sel in self
            .selected_build_variants
            .iter()
            .filter(|sel| sel.hero_id == hero_id && sel.role == role)
        {
            if self.is_direct_upgrade(sel.primary_item_id, sel.variant_item_id) {
                continue;
            }
            let Some(step) = build
                .steps
                .iter_mut()
                .find(|s| s.item_id == sel.primary_item_id)
            else {
                continue;
            };
            let Some(var_pos) = step
                .variants
                .iter()
                .position(|v| v.item_id == sel.variant_item_id)
            else {
                continue;
            };
            let chosen = step.variants.remove(var_pos);
            let old_primary = BuildVariant {
                item_id: step.item_id,
                score: step.score,
                avg_time_mins: step.avg_time_mins,
                counter_trait: step.counter_trait,
            };
            step.item_id = chosen.item_id;
            step.score = chosen.score;
            step.avg_time_mins = chosen.avg_time_mins;
            step.counter_trait = chosen.counter_trait;
            step.owned = owned_items.contains(&chosen.item_id);
            if let Some(entry) = self.item_repo.item_catalog(chosen.item_id) {
                step.total_components = entry.components.len();
                step.owned_components = entry
                    .components
                    .iter()
                    .filter(|c| owned_items.contains(c))
                    .count();
            } else {
                step.total_components = 0;
                step.owned_components = 0;
            }
            step.variants.insert(var_pos, old_primary);
        }

        build.next_unowned_index = build.steps.iter().position(|s| !s.owned);
        Some(build)
    }

    /// Computes the bottom Y coordinate of the Dota 2 top-bar portrait capture strip.
    ///
    /// When `dota_window_rect` `(x, y, w, h)` is known, uses `y + (h * 0.08).ceil().max(48.0)`.
    /// Otherwise uses the monitor-level floor `(monitor_h * 0.08).ceil().max(96.0)`.
    pub fn top_bar_capture_bottom_y(
        monitor_h: f32,
        dota_window_rect: Option<(f32, f32, f32, f32)>,
    ) -> f32 {
        match dota_window_rect {
            Some((_, dota_y, _, dota_h)) => dota_y + (dota_h * 0.08).ceil().max(48.0),
            None => (monitor_h * 0.08).ceil().max(96.0),
        }
    }

    /// Synchronizes `cache_mgr` target item `(hero, role)` pairs with the active build hero and picked allies.
    pub fn sync_target_item_heroes_to_cache(&mut self) {
        let mut hero_roles = Vec::new();
        if let Some((active_hero, active_role)) = self.active_build_hero() {
            hero_roles.push((active_hero, active_role));
        }
        for (idx, pick) in self.draft.allies.iter().enumerate() {
            if let Some(p) = pick {
                let role = p.role.unwrap_or(Role::ALL[idx]);
                if !hero_roles.contains(&(p.hero_id, role)) {
                    hero_roles.push((p.hero_id, role));
                }
            }
        }
        self.cache_mgr.set_target_item_hero_roles(hero_roles);
    }

    /// Sets the active monitor desktop bounds used for live overlay clamping.
    pub fn set_active_monitor_rect(&mut self, monitor: Option<MonitorRect>) {
        self.active_monitor_rect = monitor;
    }

    /// Sets the known desktop monitor bounds used when resolving the overlay's host monitor.
    pub fn set_known_monitors(&mut self, monitors: Vec<MonitorRect>) {
        self.known_monitors = monitors;
    }

    /// Queries native OS monitor bounds and updates `known_monitors` when available.
    pub fn refresh_native_monitors(&mut self) {
        let detected: Vec<MonitorRect> = truesight_adapters::vision::detect_native_monitor_rects()
            .into_iter()
            .map(|(x, y, w, h)| MonitorRect::new(x, y, w, h))
            .collect();
        if !detected.is_empty() {
            self.known_monitors = detected;
        }
    }

    /// Resolves the host monitor bounds for `geom` from `active_monitor_rect` or `known_monitors`.
    /// Returns `None` when reliable native monitor bounds are unavailable.
    pub fn resolve_monitor_for_overlay(&self, geom: OverlayGeometry) -> Option<MonitorRect> {
        if let Some(mon) = self.active_monitor_rect {
            return Some(mon);
        }
        if self.known_monitors.is_empty() {
            return None;
        }
        let cx = geom.x + geom.width * 0.5;
        let cy = geom.y + geom.height * 0.5;
        if !cx.is_finite() || !cy.is_finite() {
            return self.known_monitors.first().copied();
        }
        if let Some(containing) = self
            .known_monitors
            .iter()
            .copied()
            .find(|m| cx >= m.x && cx <= m.x + m.width && cy >= m.y && cy <= m.y + m.height)
        {
            return Some(containing);
        }
        self.known_monitors.iter().copied().min_by(|a, b| {
            let da = monitor_squared_distance(*a, cx, cy);
            let db = monitor_squared_distance(*b, cx, cy);
            da.total_cmp(&db)
        })
    }

    /// Sanitizes an `egui` `pixels_per_point` scale factor into a safe finite range `[0.5, 8.0]`.
    pub fn sanitize_pixels_per_point(pixels_per_point: f32) -> f32 {
        if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
            pixels_per_point.clamp(0.5, 8.0)
        } else {
            1.0
        }
    }

    /// Resolves the effective viewport `pixels_per_point` (`native_pixels_per_point * zoom_factor`
    /// when native DPI is reported, falling back to `context_pixels_per_point`).
    pub fn resolve_effective_pixels_per_point(
        vp_native_pixels_per_point: Option<f32>,
        context_pixels_per_point: f32,
        zoom_factor: f32,
    ) -> f32 {
        let zoom = Self::sanitize_pixels_per_point(zoom_factor);
        let raw_ppp = match vp_native_pixels_per_point.filter(|n| n.is_finite() && *n > 0.0) {
            Some(native_ppp) => native_ppp * zoom,
            None => context_pixels_per_point,
        };
        Self::sanitize_pixels_per_point(raw_ppp)
    }

    /// Returns the persisted overlay geometry converted into physical desktop pixels
    /// using `pixels_per_point`.
    pub fn overlay_geometry_in_desktop_pixels(&self, pixels_per_point: f32) -> OverlayGeometry {
        self.overlay_geometry
            .scale_to_desktop_pixels(pixels_per_point)
    }

    /// Clamps overlay geometry against `monitor` when reliable bounds are available; otherwise
    /// clamps dimensions and avoids `capture_strip_rect` without guessing a monitor origin.
    pub fn clamp_overlay_geometry_with_optional_monitor(
        geom: OverlayGeometry,
        monitor: Option<MonitorRect>,
        fallback_monitor_size: Option<(f32, f32)>,
        capture_strip_rect: Option<(f32, f32, f32, f32)>,
    ) -> OverlayGeometry {
        Self::clamp_overlay_geometry_with_optional_monitor_scaled(
            geom,
            monitor,
            fallback_monitor_size,
            capture_strip_rect,
            1.0,
        )
    }

    /// Clamps physical desktop pixel `geom_px` against physical `monitor_px` (when available)
    /// and physical `capture_strip_rect_px`, scaling logical point bounds (`280..520` x `160..360`)
    /// by `pixels_per_point`.
    pub fn clamp_overlay_geometry_with_optional_monitor_scaled(
        geom_px: OverlayGeometry,
        monitor_px: Option<MonitorRect>,
        fallback_monitor_size_px: Option<(f32, f32)>,
        capture_strip_rect_px: Option<(f32, f32, f32, f32)>,
        pixels_per_point: f32,
    ) -> OverlayGeometry {
        Self::clamp_overlay_geometry_with_optional_monitor_min_scaled(
            geom_px,
            monitor_px,
            fallback_monitor_size_px,
            capture_strip_rect_px,
            pixels_per_point,
            280.0,
            160.0,
        )
    }

    /// Clamps physical desktop pixel `geom_px` against optional physical monitor bounds,
    /// enforcing custom logical minimum bounds `(min_w_pts, min_h_pts)` scaled by `pixels_per_point`.
    pub fn clamp_overlay_geometry_with_optional_monitor_min_scaled(
        mut geom_px: OverlayGeometry,
        monitor_px: Option<MonitorRect>,
        fallback_monitor_size_px: Option<(f32, f32)>,
        capture_strip_rect_px: Option<(f32, f32, f32, f32)>,
        pixels_per_point: f32,
        min_w_pts: f32,
        min_h_pts: f32,
    ) -> OverlayGeometry {
        let ppp = Self::sanitize_pixels_per_point(pixels_per_point);
        if let Some(mon) = monitor_px {
            return Self::clamp_overlay_geometry_in_monitor_with_min_scaled(
                geom_px,
                mon,
                capture_strip_rect_px,
                ppp,
                min_w_pts,
                min_h_pts,
            );
        }

        let min_w_px = min_w_pts * ppp;
        let max_w_cap_px = 840.0 * ppp;
        let min_h_px = min_h_pts * ppp;
        let max_h_cap_px = 720.0 * ppp;
        let min_mon_w_px = 640.0 * ppp;
        let min_mon_h_px = 480.0 * ppp;

        let max_w = fallback_monitor_size_px
            .map(|(w, _)| w)
            .filter(|w| w.is_finite() && *w >= min_mon_w_px)
            .unwrap_or(max_w_cap_px);
        let max_h = fallback_monitor_size_px
            .map(|(_, h)| h)
            .filter(|h| h.is_finite() && *h >= min_mon_h_px)
            .unwrap_or(max_h_cap_px);

        let raw_w = if geom_px.width.is_finite() {
            geom_px.width
        } else {
            380.0 * ppp
        };
        let raw_h = if geom_px.height.is_finite() {
            geom_px.height
        } else {
            230.0 * ppp
        };
        geom_px.width = raw_w.clamp(min_w_px, max_w_cap_px.min(max_w.max(min_w_px)));
        geom_px.height = raw_h.clamp(min_h_px, max_h_cap_px.min(max_h.max(min_h_px)));

        if !geom_px.x.is_finite() {
            geom_px.x = 24.0 * ppp;
        }
        if !geom_px.y.is_finite() {
            geom_px.y = 96.0 * ppp;
        }

        if let Some((cx, cy, cw, ch)) = capture_strip_rect_px.filter(|&(cx, cy, cw, ch)| {
            cx.is_finite()
                && cy.is_finite()
                && cw.is_finite()
                && ch.is_finite()
                && cw > 0.0
                && ch > 0.0
        }) {
            let overlaps_x = geom_px.x < cx + cw && geom_px.x + geom_px.width > cx;
            let overlaps_y = geom_px.y < cy + ch && geom_px.y + geom_px.height > cy;
            if overlaps_x && overlaps_y {
                let below_y = cy + ch;
                let fits_below = fallback_monitor_size_px
                    .map(|(_, h)| {
                        !h.is_finite() || h < min_mon_h_px || below_y + geom_px.height <= h
                    })
                    .unwrap_or(true);
                let above_y = cy - geom_px.height;
                if fits_below || above_y < 0.0 {
                    geom_px.y = below_y;
                } else {
                    geom_px.y = above_y;
                }
            }
        }

        geom_px
    }

    /// Sets the active Dota 2 top-bar capture strip rectangle `(x, y, w, h)` in desktop coordinates.
    pub fn set_active_capture_strip_rect(&mut self, rect: Option<(f32, f32, f32, f32)>) {
        self.active_capture_strip_rect = rect;
    }

    /// Clamps overlay geometry to safe monitor bounds while avoiding the Dota 2 top-bar capture strip.
    pub fn clamp_overlay_geometry(
        geom: OverlayGeometry,
        monitor_w: f32,
        monitor_h: f32,
    ) -> OverlayGeometry {
        Self::clamp_overlay_geometry_against_capture(geom, monitor_w, monitor_h, None)
    }

    /// Clamps overlay geometry to safe monitor bounds and pushes it below `capture_strip_rect`
    /// `(cx, cy, cw, ch)` when horizontally overlapping, or below `(monitor_h * 0.08).ceil().max(96.0)`.
    pub fn clamp_overlay_geometry_against_capture(
        geom: OverlayGeometry,
        monitor_w: f32,
        monitor_h: f32,
        capture_strip_rect: Option<(f32, f32, f32, f32)>,
    ) -> OverlayGeometry {
        Self::clamp_overlay_geometry_in_monitor(
            geom,
            MonitorRect::new(0.0, 0.0, monitor_w, monitor_h),
            capture_strip_rect,
        )
    }

    /// Clamps `OverlayGeometry` within `monitor` (preserving secondary monitor origins)
    /// and prevents it from intersecting `capture_strip_rect` `(cx, cy, cw, ch)` (including
    /// when Dota 2 runs in a window offset from the top of the monitor).
    pub fn clamp_overlay_geometry_in_monitor(
        geom: OverlayGeometry,
        monitor: MonitorRect,
        capture_strip_rect: Option<(f32, f32, f32, f32)>,
    ) -> OverlayGeometry {
        Self::clamp_overlay_geometry_in_monitor_scaled(geom, monitor, capture_strip_rect, 1.0)
    }

    /// Clamps physical desktop pixel `geom_px` within physical `monitor_px` (preserving secondary
    /// monitor origins) and prevents it from intersecting physical `capture_strip_rect_px`,
    /// scaling logical point bounds (`280..840` x `160..720`) by `pixels_per_point`.
    pub fn clamp_overlay_geometry_in_monitor_scaled(
        geom_px: OverlayGeometry,
        monitor_px: MonitorRect,
        capture_strip_rect_px: Option<(f32, f32, f32, f32)>,
        pixels_per_point: f32,
    ) -> OverlayGeometry {
        Self::clamp_overlay_geometry_in_monitor_with_min_scaled(
            geom_px,
            monitor_px,
            capture_strip_rect_px,
            pixels_per_point,
            280.0,
            160.0,
        )
    }

    /// Clamps physical desktop pixel `geom_px` within physical `monitor_px`, enforcing custom
    /// logical minimum bounds `(min_w_pts, min_h_pts)` scaled by `pixels_per_point`.
    pub fn clamp_overlay_geometry_in_monitor_with_min_scaled(
        mut geom_px: OverlayGeometry,
        monitor_px: MonitorRect,
        capture_strip_rect_px: Option<(f32, f32, f32, f32)>,
        pixels_per_point: f32,
        min_w_pts: f32,
        min_h_pts: f32,
    ) -> OverlayGeometry {
        let ppp = Self::sanitize_pixels_per_point(pixels_per_point);
        let min_w_px = (min_w_pts * ppp).ceil();
        let max_w_cap_px = (840.0 * ppp).floor();
        let min_h_px = (min_h_pts * ppp).ceil();
        let max_h_cap_px = (720.0 * ppp).floor();
        let min_mon_w_px = 640.0 * ppp;
        let min_mon_h_px = 480.0 * ppp;

        let mon_x = if monitor_px.x.is_finite() {
            monitor_px.x.ceil()
        } else {
            0.0
        };
        let mon_y = if monitor_px.y.is_finite() {
            monitor_px.y.ceil()
        } else {
            0.0
        };
        let mon_w = if monitor_px.width.is_finite() && monitor_px.width >= min_mon_w_px {
            monitor_px.width
        } else {
            1920.0 * ppp
        };
        let mon_h = if monitor_px.height.is_finite() && monitor_px.height >= min_mon_h_px {
            monitor_px.height
        } else {
            1080.0 * ppp
        };

        let raw_w = if geom_px.width.is_finite() {
            geom_px.width
        } else {
            380.0 * ppp
        };
        let raw_h = if geom_px.height.is_finite() {
            geom_px.height
        } else {
            230.0 * ppp
        };
        geom_px.width = raw_w.clamp(min_w_px, max_w_cap_px.min(mon_w.max(min_w_px)));
        geom_px.height = raw_h.clamp(min_h_px, max_h_cap_px.min(mon_h.max(min_h_px)));

        let default_top_floor = (mon_h * 0.08).ceil().max((96.0 * ppp).ceil());
        let raw_x = if geom_px.x.is_finite() {
            geom_px.x
        } else {
            mon_x + (24.0 * ppp).ceil()
        };
        let raw_y = if geom_px.y.is_finite() {
            geom_px.y
        } else {
            mon_y + default_top_floor
        };

        let max_x = (mon_x + mon_w - geom_px.width).floor().max(mon_x);
        let max_y = (mon_y + mon_h - geom_px.height).floor().max(mon_y);
        geom_px.x = raw_x.clamp(mon_x, max_x);
        geom_px.y = raw_y.clamp(mon_y, max_y);

        if let Some((cx, cy, cw, ch)) = capture_strip_rect_px.filter(|&(cx, cy, cw, ch)| {
            cx.is_finite()
                && cy.is_finite()
                && cw.is_finite()
                && ch.is_finite()
                && cw > 0.0
                && ch > 0.0
        }) {
            let overlaps_x = geom_px.x < cx + cw && geom_px.x + geom_px.width > cx;
            let overlaps_y = geom_px.y < cy + ch && geom_px.y + geom_px.height > cy;
            if overlaps_x && overlaps_y {
                let below_y = (cy + ch).ceil();
                let above_y = (cy - geom_px.height).floor();
                if below_y <= max_y {
                    geom_px.y = below_y.max(mon_y);
                } else if above_y >= mon_y {
                    geom_px.y = above_y.min(max_y);
                } else {
                    geom_px.y = max_y;
                }
            }
        } else {
            let min_y = (mon_y + default_top_floor).min(max_y);
            if geom_px.y < min_y {
                geom_px.y = min_y;
            }
        }

        geom_px
    }

    /// Returns `true` when the compact overlay should be visible (pinned manually or auto-opened with Dota 2).
    pub fn is_overlay_visible(&self) -> bool {
        self.overlay_mode
            || (self.auto_overlay_with_dota
                && self.dota_process_running
                && !self.auto_overlay_dismissed)
    }

    /// Enables or disables manual compact overlay pinning and persists the preference.
    pub fn set_overlay_mode(&mut self, enabled: bool) {
        self.overlay_mode = enabled;
        if enabled {
            self.auto_overlay_dismissed = false;
        } else if self.dota_process_running {
            self.auto_overlay_dismissed = true;
        }
        if !self.is_overlay_visible() {
            self.overlay_geometry.click_through = false;
            self.build_overlay_geometry.click_through = false;
        }
        let _ = self.persist_ui_config();
    }

    /// Enables or disables automatic overlay visibility synced to the Dota 2 lifecycle and persists it.
    pub fn set_auto_overlay_with_dota(&mut self, enabled: bool) {
        self.auto_overlay_with_dota = enabled;
        if enabled {
            self.auto_overlay_dismissed = false;
        }
        if !self.is_overlay_visible() {
            self.overlay_geometry.click_through = false;
            self.build_overlay_geometry.click_through = false;
        }
        let _ = self.persist_ui_config();
    }

    fn clear_overlay_click_through_and_persist(&mut self) {
        let had_click_through =
            self.overlay_geometry.click_through || self.build_overlay_geometry.click_through;
        self.overlay_geometry.click_through = false;
        self.build_overlay_geometry.click_through = false;
        if had_click_through {
            let _ = self.persist_ui_config();
        }
    }

    /// Updates Dota 2 process liveness and returns `true` when effective overlay visibility changes.
    pub fn set_dota_process_running(&mut self, running: bool) -> bool {
        let was_visible = self.is_overlay_visible();
        self.dota_process_running = running;
        if !running {
            self.auto_overlay_dismissed = false;
        }
        let now_visible = self.is_overlay_visible();
        if was_visible && !now_visible {
            self.clear_overlay_click_through_and_persist();
        }
        was_visible != now_visible
    }

    /// Closes the active overlay for now, suppressing auto-reopen until Dota 2 exits and relaunches.
    pub fn dismiss_overlay(&mut self) {
        if self.dota_process_running {
            self.auto_overlay_dismissed = true;
        }
        if self.overlay_mode {
            self.set_overlay_mode(false);
        } else if !self.is_overlay_visible() {
            self.clear_overlay_click_through_and_persist();
        }
    }

    /// Toggles overlay visibility from the `F8` shortcut across manual and auto-opened states.
    pub fn toggle_overlay_shortcut(&mut self) {
        if self.is_overlay_visible() {
            self.dismiss_overlay();
        } else {
            self.set_overlay_mode(true);
        }
    }

    /// Enables or disables mouse click-through on the overlay window and persists the preference.
    pub fn set_overlay_click_through(&mut self, click_through: bool) {
        self.overlay_geometry.click_through = click_through;
        self.build_overlay_geometry.click_through = click_through;
        let _ = self.persist_ui_config();
    }

    /// Updates and persists the Picks overlay window geometry.
    pub fn set_overlay_geometry(&mut self, geom: OverlayGeometry) {
        self.overlay_geometry = geom;
        let _ = self.persist_ui_config();
    }

    /// Returns the geometry corresponding to the active overlay mode (`build_overlay_geometry` in
    /// post-draft Build view, `overlay_geometry` in Suggested Picks view), preserving shared `click_through`.
    pub fn active_overlay_geometry(&self) -> OverlayGeometry {
        let mut geom = if self.is_post_draft_build_view() {
            self.build_overlay_geometry
        } else {
            self.overlay_geometry
        };
        geom.click_through = self.overlay_geometry.click_through;
        geom
    }

    /// Updates and persists geometry only for the currently active overlay mode (`Picks` vs `Build`).
    /// Persists the overlay geometry for the explicit view (`false` = Draft Picks, `true` = Build)
    /// without altering the opposite view's stored rectangle.
    pub fn set_overlay_geometry_for_view(
        &mut self,
        is_post_draft: bool,
        mut geom: OverlayGeometry,
    ) {
        geom.click_through = self.overlay_geometry.click_through;
        if is_post_draft {
            self.build_overlay_geometry = geom;
        } else {
            self.overlay_geometry = geom;
        }
        let _ = self.persist_ui_config();
    }

    pub fn set_active_overlay_geometry(&mut self, geom: OverlayGeometry) {
        self.set_overlay_geometry_for_view(self.is_post_draft_build_view(), geom);
    }

    /// Sanitizes an overlay glass opacity value to `[MIN_OVERLAY_OPACITY, MAX_OVERLAY_OPACITY]`.
    pub fn sanitize_overlay_opacity(opacity: f32) -> f32 {
        if !opacity.is_finite() {
            DEFAULT_OVERLAY_OPACITY
        } else {
            opacity.clamp(MIN_OVERLAY_OPACITY, MAX_OVERLAY_OPACITY)
        }
    }

    /// Updates and persists the compact overlay glass opacity (`0.35..=0.95`).
    pub fn set_overlay_opacity(&mut self, opacity: f32) {
        self.overlay_opacity = Self::sanitize_overlay_opacity(opacity);
        let _ = self.persist_ui_config();
    }

    /// Persists the onboarding dismissal preference when config persistence is enabled.
    pub fn set_onboarding_dismissed(&mut self, dismissed: bool) -> Result<(), String> {
        self.onboarding_dismissed = dismissed;
        self.persist_ui_config()
    }

    /// Updates and persists the selected UI display language (`es`, `en`, `ru`).
    pub fn set_ui_language(&mut self, language: UiLanguage) {
        self.ui_language = language;
        let _ = self.persist_ui_config();
    }

    /// Updates and persists the pass-through toggle hotkey (`ui_config.toml`).
    pub fn set_pass_hotkey(&mut self, hotkey: PassHotkey) {
        self.pass_hotkey = hotkey;
        let _ = self.persist_ui_config();
    }

    /// Returns whole days remaining before the persisted STRATZ token expires at `now_unix_secs`,
    /// or `None` when no token is set or the token is already expired.
    pub fn stratz_token_days_remaining_at(&self, now_unix_secs: u64) -> Option<u32> {
        self.stratz_api_token.as_ref()?;
        let exp = resolve_stratz_exp(self.stratz_api_token.as_deref(), self.stratz_token_exp)?;
        if exp > now_unix_secs {
            Some(((exp - now_unix_secs) / 86_400) as u32)
        } else {
            None
        }
    }

    /// Returns `true` when a STRATZ token is present and its expiry timestamp has passed.
    pub fn is_stratz_token_expired_at(&self, now_unix_secs: u64) -> bool {
        if self.stratz_api_token.is_none() {
            return false;
        }
        let exp = resolve_stratz_exp(self.stratz_api_token.as_deref(), self.stratz_token_exp);
        is_stratz_exp_expired(exp, now_unix_secs)
    }

    /// Validates a STRATZ JWT, persists it privately, wires it into `LocalCacheManager`,
    /// and links `SteamId` when no different explicit Steam account is already configured.
    pub fn apply_stratz_token(
        &mut self,
        token: &str,
        now_unix_secs: u64,
    ) -> Result<StratzTokenInfo, String> {
        let info = parse_stratz_jwt_claims(token, now_unix_secs)?;
        let cleaned = token.trim().to_string();
        self.stratz_api_token = Some(cleaned.clone());
        self.cache_mgr.stratz_api_token = Some(cleaned);
        self.stratz_token_exp = Some(info.exp);
        self.cache_mgr.stratz_token_exp = Some(info.exp);
        let _ = self.persist_ui_config();
        if let Some(steam32) = info.steam32_id {
            let has_different_explicit = self.cache_mgr.explicit_override
                && self
                    .cache_mgr
                    .player_account_id
                    .is_some_and(|existing| existing != steam32);
            if has_different_explicit {
                self.pending_stratz_steam32_link = Some(steam32);
            } else {
                self.pending_stratz_steam32_link = None;
                self.set_account_id(Some(steam32), true);
                if let Some(path) = self.config_path.as_deref() {
                    let _ = save_account_config_to(path, AccountLinkConfig::Explicit(steam32));
                }
            }
        } else {
            self.pending_stratz_steam32_link = None;
        }
        Ok(info)
    }

    /// Confirms replacing the current Steam account with the pending STRATZ JWT `SteamId`.
    pub fn confirm_pending_stratz_steam_link(&mut self) -> Option<u64> {
        let steam32 = self.pending_stratz_steam32_link.take()?;
        self.set_account_id(Some(steam32), true);
        if let Some(path) = self.config_path.as_deref() {
            let _ = save_account_config_to(path, AccountLinkConfig::Explicit(steam32));
        }
        Some(steam32)
    }

    /// Dismisses the pending STRATZ JWT `SteamId` prompt and keeps the current explicit account.
    pub fn dismiss_pending_stratz_steam_link(&mut self) {
        self.pending_stratz_steam32_link = None;
    }
}

/// Decoded claims from a STRATZ GraphQL API JWT bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StratzTokenInfo {
    pub steam_id64: Option<u64>,
    pub steam32_id: Option<u64>,
    pub exp: u64,
    pub iat: Option<u64>,
    pub days_remaining: u32,
}

fn monitor_squared_distance(mon: MonitorRect, px: f32, py: f32) -> f32 {
    let dx = if px < mon.x {
        mon.x - px
    } else if px > mon.x + mon.width {
        px - (mon.x + mon.width)
    } else {
        0.0
    };
    let dy = if py < mon.y {
        mon.y - py
    } else if py > mon.y + mon.height {
        py - (mon.y + mon.height)
    } else {
        0.0
    };
    dx * dx + dy * dy
}

/// Desktop-coordinate bounds of the monitor hosting the overlay or Dota 2 window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl MonitorRect {
    /// Constructs a `MonitorRect` from origin `(x, y)` and dimensions `(width, height)`.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub click_through: bool,
}

impl OverlayGeometry {
    /// Converts logical `egui` viewport points (`x, y, width, height`) into physical desktop
    /// pixels using the viewport's `pixels_per_point` scale factor.
    pub fn scale_to_desktop_pixels(self, pixels_per_point: f32) -> Self {
        let ppp = AppSession::sanitize_pixels_per_point(pixels_per_point);
        Self {
            x: self.x * ppp,
            y: self.y * ppp,
            width: self.width * ppp,
            height: self.height * ppp,
            click_through: self.click_through,
        }
    }

    /// Converts physical desktop pixel geometry back into logical `egui` viewport points
    /// using the viewport's `pixels_per_point` scale factor.
    pub fn scale_to_egui_points(self, pixels_per_point: f32) -> Self {
        let ppp = AppSession::sanitize_pixels_per_point(pixels_per_point);
        Self {
            x: self.x / ppp,
            y: self.y / ppp,
            width: self.width / ppp,
            height: self.height / ppp,
            click_through: self.click_through,
        }
    }
}

impl OverlayGeometry {
    /// Default geometry for the compact phased Build overlay.
    pub const fn default_build() -> Self {
        Self {
            x: 24.0,
            y: 112.0,
            width: 420.0,
            height: 168.0,
            click_through: false,
        }
    }
}

impl Default for OverlayGeometry {
    fn default() -> Self {
        Self {
            x: 24.0,
            y: 112.0,
            width: 380.0,
            height: 230.0,
            click_through: false,
        }
    }
}

/// Supported GUI display languages (`[ES]`, `[EN]`, `[RU]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiLanguage {
    #[default]
    En,
    Es,
    Ru,
}

impl UiLanguage {
    /// Display order in language selector bars (`[ES]`, `[EN]`, `[RU]`).
    pub const ALL: [Self; 3] = [Self::Es, Self::En, Self::Ru];

    /// Compact uppercase selector pill label (`"ES"`, `"EN"`, `"RU"`).
    pub const fn code(self) -> &'static str {
        match self {
            Self::Es => "ES",
            Self::En => "EN",
            Self::Ru => "RU",
        }
    }

    /// Canonical lowercase configuration string written to `ui_config.toml`.
    pub const fn as_config_str(self) -> &'static str {
        match self {
            Self::Es => "es",
            Self::En => "en",
            Self::Ru => "ru",
        }
    }

    /// Parses a persisted or locale string into `UiLanguage` (case-insensitive, trims quotes).
    pub fn parse(raw: &str) -> Option<Self> {
        let norm = raw.trim().trim_matches('"').trim().to_ascii_lowercase();
        match norm.as_str() {
            "en" | "en-us" | "en-gb" => Some(Self::En),
            "es" | "es-es" | "es-la" | "es-pe" | "es-mx" => Some(Self::Es),
            "ru" | "ru-ru" => Some(Self::Ru),
            _ => None,
        }
    }
}

/// Supported base keys for the configurable overlay click-through hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassHotkeyKey {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl PassHotkeyKey {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "A" => Some(Self::A),
            "B" => Some(Self::B),
            "C" => Some(Self::C),
            "D" => Some(Self::D),
            "E" => Some(Self::E),
            "F" => Some(Self::F),
            "G" => Some(Self::G),
            "H" => Some(Self::H),
            "I" => Some(Self::I),
            "J" => Some(Self::J),
            "K" => Some(Self::K),
            "L" => Some(Self::L),
            "M" => Some(Self::M),
            "N" => Some(Self::N),
            "O" => Some(Self::O),
            "P" => Some(Self::P),
            "Q" => Some(Self::Q),
            "R" => Some(Self::R),
            "S" => Some(Self::S),
            "T" => Some(Self::T),
            "U" => Some(Self::U),
            "V" => Some(Self::V),
            "W" => Some(Self::W),
            "X" => Some(Self::X),
            "Y" => Some(Self::Y),
            "Z" => Some(Self::Z),
            "0" | "DIGIT0" => Some(Self::Digit0),
            "1" | "DIGIT1" => Some(Self::Digit1),
            "2" | "DIGIT2" => Some(Self::Digit2),
            "3" | "DIGIT3" => Some(Self::Digit3),
            "4" | "DIGIT4" => Some(Self::Digit4),
            "5" | "DIGIT5" => Some(Self::Digit5),
            "6" | "DIGIT6" => Some(Self::Digit6),
            "7" | "DIGIT7" => Some(Self::Digit7),
            "8" | "DIGIT8" => Some(Self::Digit8),
            "9" | "DIGIT9" => Some(Self::Digit9),
            "F1" => Some(Self::F1),
            "F2" => Some(Self::F2),
            "F3" => Some(Self::F3),
            "F4" => Some(Self::F4),
            "F5" => Some(Self::F5),
            "F6" => Some(Self::F6),
            "F7" => Some(Self::F7),
            "F8" => Some(Self::F8),
            "F9" => Some(Self::F9),
            "F10" => Some(Self::F10),
            "F11" => Some(Self::F11),
            "F12" => Some(Self::F12),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::Digit0 => "0",
            Self::Digit1 => "1",
            Self::Digit2 => "2",
            Self::Digit3 => "3",
            Self::Digit4 => "4",
            Self::Digit5 => "5",
            Self::Digit6 => "6",
            Self::Digit7 => "7",
            Self::Digit8 => "8",
            Self::Digit9 => "9",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
        }
    }

    pub const fn xdg_key(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
            Self::C => "c",
            Self::D => "d",
            Self::E => "e",
            Self::F => "f",
            Self::G => "g",
            Self::H => "h",
            Self::I => "i",
            Self::J => "j",
            Self::K => "k",
            Self::L => "l",
            Self::M => "m",
            Self::N => "n",
            Self::O => "o",
            Self::P => "p",
            Self::Q => "q",
            Self::R => "r",
            Self::S => "s",
            Self::T => "t",
            Self::U => "u",
            Self::V => "v",
            Self::W => "w",
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
            Self::Digit0 => "0",
            Self::Digit1 => "1",
            Self::Digit2 => "2",
            Self::Digit3 => "3",
            Self::Digit4 => "4",
            Self::Digit5 => "5",
            Self::Digit6 => "6",
            Self::Digit7 => "7",
            Self::Digit8 => "8",
            Self::Digit9 => "9",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
        }
    }
}

/// Configurable modifier + key binding for toggling overlay mouse click-through (`Ctrl+Shift+T` by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassHotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: PassHotkeyKey,
}

impl Default for PassHotkey {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: true,
            key: PassHotkeyKey::T,
        }
    }
}

impl PassHotkey {
    /// Parses a modified shortcut string such as `"Ctrl+Shift+T"` or `"Ctrl+Alt+F10"`.
    /// Requires at least one modifier (`Ctrl`, `Alt`, `Shift`) so bare Dota 2 keys are never hijacked.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut key = None;
        for part in raw.trim().trim_matches('"').split('+') {
            let tok = part.trim();
            if tok.is_empty() {
                return None;
            }
            match tok.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                _ => {
                    if key.is_some() {
                        return None;
                    }
                    key = Some(PassHotkeyKey::parse(tok)?);
                }
            }
        }
        let key = key?;
        if !(ctrl || alt || shift) {
            return None;
        }
        Some(Self {
            ctrl,
            alt,
            shift,
            key,
        })
    }

    /// Formats the canonical UI/config string (e.g. `"Ctrl+Shift+T"`).
    pub fn format_label(self) -> String {
        let mut parts = Vec::with_capacity(4);
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(self.key.label());
        parts.join("+")
    }

    /// Formats the XDG GlobalShortcuts `preferred_trigger` syntax (e.g. `"CTRL+SHIFT+t"`).
    pub fn to_portal_trigger(self) -> String {
        let mut parts = Vec::with_capacity(4);
        if self.ctrl {
            parts.push("CTRL");
        }
        if self.alt {
            parts.push("ALT");
        }
        if self.shift {
            parts.push("SHIFT");
        }
        parts.push(self.key.xdg_key());
        parts.join("+")
    }
}

/// Persisted UI onboarding, overlay, language, hotkey, and STRATZ token state stored in `ui_config.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct UiConfig {
    pub onboarding_dismissed: bool,
    pub stratz_api_token: Option<String>,
    pub stratz_token_exp: Option<u64>,
    pub overlay_mode: bool,
    pub auto_overlay_with_dota: bool,
    pub overlay_geometry: OverlayGeometry,
    pub build_overlay_geometry: OverlayGeometry,
    pub overlay_opacity: f32,
    pub ui_language: UiLanguage,
    pub pass_hotkey: PassHotkey,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            onboarding_dismissed: false,
            stratz_api_token: None,
            stratz_token_exp: None,
            overlay_mode: false,
            auto_overlay_with_dota: true,
            overlay_geometry: OverlayGeometry::default(),
            build_overlay_geometry: OverlayGeometry::default_build(),
            overlay_opacity: DEFAULT_OVERLAY_OPACITY,
            ui_language: UiLanguage::default(),
            pass_hotkey: PassHotkey::default(),
        }
    }
}

fn resolve_stratz_exp(token: Option<&str>, explicit_exp: Option<u64>) -> Option<u64> {
    explicit_exp.or_else(|| token.and_then(|t| parse_stratz_jwt_claims(t, 0).ok().map(|i| i.exp)))
}

fn is_stratz_exp_expired(exp: Option<u64>, now_unix_secs: u64) -> bool {
    exp.is_some_and(|e| e <= now_unix_secs)
}

fn decode_base64url(input: &str) -> Result<Vec<u8>, String> {
    let clean = input.trim_end_matches('=');
    if clean.len() % 4 == 1 {
        return Err("invalid base64url segment length".into());
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
            _ => return Err("invalid base64url character".into()),
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

fn find_json_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\"");
    let pos = json.find(&needle)?;
    let rest = &json[pos + needle.len()..];
    let colon = rest.find(':')?;
    Some(rest[colon + 1..].trim_start())
}

fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let rest = find_json_field(json, key)?;
    let inner = rest.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

fn extract_json_u64(json: &str, key: &str) -> Option<u64> {
    let rest = find_json_field(json, key)?;
    if let Some(inner) = rest.strip_prefix('"') {
        let end = inner.find('"')?;
        return inner[..end].trim().parse::<u64>().ok();
    }
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<u64>().ok()
    }
}

/// Decodes and validates a 3-segment STRATZ JWT payload in pure Rust without external crates.
pub fn parse_stratz_jwt_claims(token: &str, now_unix_secs: u64) -> Result<StratzTokenInfo, String> {
    let trimmed = token.trim();
    let mut parts = trimmed.split('.');
    let hdr = parts.next().unwrap_or("");
    let payload_b64 = parts.next().unwrap_or("");
    let sig = parts.next().unwrap_or("");
    if hdr.is_empty() || payload_b64.is_empty() || sig.is_empty() || parts.next().is_some() {
        return Err("STRATZ token must be a 3-segment JWT".into());
    }

    let payload_bytes = decode_base64url(payload_b64)?;
    let payload = std::str::from_utf8(&payload_bytes)
        .map_err(|_| "STRATZ JWT payload is not valid UTF-8".to_string())?;

    let iss = extract_json_string(payload, "iss")
        .ok_or_else(|| "STRATZ JWT missing 'iss' claim".to_string())?;
    if !iss.contains("stratz.com") {
        return Err("STRATZ JWT 'iss' claim must contain 'stratz.com'".into());
    }

    let exp = extract_json_u64(payload, "exp")
        .ok_or_else(|| "STRATZ JWT missing numeric 'exp' claim".to_string())?;
    if exp <= now_unix_secs {
        return Err("STRATZ token is expired".into());
    }
    let iat = extract_json_u64(payload, "iat");
    let steam_id64 = extract_json_u64(payload, "SteamId");
    let steam32_id = steam_id64.and_then(normalize_steam32_id);
    let days_remaining = ((exp - now_unix_secs) / 86_400) as u32;

    Ok(StratzTokenInfo {
        steam_id64,
        steam32_id,
        exp,
        iat,
        days_remaining,
    })
}

/// Loads `ui_config.toml` from `config_dir` (returns defaults if missing or unreadable).
pub fn load_ui_config_from_dir(config_dir: &Path) -> UiConfig {
    let path = config_dir.join("ui_config.toml");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return UiConfig::default();
    };
    let mut cfg = UiConfig::default();
    let mut has_build_x = false;
    let mut has_build_y = false;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        match key {
            "onboarding_dismissed" => cfg.onboarding_dismissed = val == "true",
            "stratz_api_token" => {
                let s = val.trim_matches('"').trim();
                if !s.is_empty() {
                    cfg.stratz_api_token = Some(s.to_string());
                }
            }
            "stratz_token_exp" => {
                cfg.stratz_token_exp = val.parse::<u64>().ok();
            }
            "overlay_mode" => cfg.overlay_mode = val == "true",
            "auto_overlay_with_dota" => cfg.auto_overlay_with_dota = val == "true",
            "overlay_x" => {
                if let Ok(x) = val.parse::<f32>() {
                    cfg.overlay_geometry.x = x;
                }
            }
            "overlay_y" => {
                if let Ok(y) = val.parse::<f32>() {
                    cfg.overlay_geometry.y = y;
                }
            }
            "overlay_width" => {
                if let Ok(w) = val.parse::<f32>() {
                    cfg.overlay_geometry.width = w;
                }
            }
            "overlay_height" => {
                if let Ok(h) = val.parse::<f32>() {
                    cfg.overlay_geometry.height = h;
                }
            }
            "build_overlay_x" => {
                if let Ok(x) = val.parse::<f32>() {
                    cfg.build_overlay_geometry.x = x;
                    has_build_x = true;
                }
            }
            "build_overlay_y" => {
                if let Ok(y) = val.parse::<f32>() {
                    cfg.build_overlay_geometry.y = y;
                    has_build_y = true;
                }
            }
            "build_overlay_width" => {
                if let Ok(w) = val.parse::<f32>() {
                    cfg.build_overlay_geometry.width = w;
                }
            }
            "build_overlay_height" => {
                if let Ok(h) = val.parse::<f32>() {
                    cfg.build_overlay_geometry.height = h;
                }
            }
            "overlay_click_through" => cfg.overlay_geometry.click_through = val == "true",
            "overlay_opacity" => {
                if let Ok(op) = val.parse::<f32>() {
                    cfg.overlay_opacity = AppSession::sanitize_overlay_opacity(op);
                }
            }
            "ui_language" => {
                if let Some(lang) = UiLanguage::parse(val) {
                    cfg.ui_language = lang;
                }
            }
            "pass_hotkey" => {
                if let Some(hk) = PassHotkey::parse(val) {
                    cfg.pass_hotkey = hk;
                }
            }
            _ => {}
        }
    }
    if !has_build_x {
        cfg.build_overlay_geometry.x = cfg.overlay_geometry.x;
    }
    if !has_build_y {
        cfg.build_overlay_geometry.y = cfg.overlay_geometry.y;
    }
    cfg.build_overlay_geometry.click_through = cfg.overlay_geometry.click_through;
    if cfg.stratz_token_exp.is_none() {
        cfg.stratz_token_exp = resolve_stratz_exp(cfg.stratz_api_token.as_deref(), None);
    }
    cfg
}

/// Persists `ui_config.toml` atomically into `config_dir`, creating an owner-only (`0600`) file on Unix before writing.
pub fn save_ui_config_to_dir(config_dir: &Path, cfg: &UiConfig) -> Result<(), String> {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let target_path = config_dir.join("ui_config.toml");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_path = config_dir.join(format!(
        ".ui_config.toml.tmp.{}.{nanos}",
        std::process::id()
    ));

    let mut body = format!("onboarding_dismissed = {}\n", cfg.onboarding_dismissed);
    body.push_str(&format!("overlay_mode = {}\n", cfg.overlay_mode));
    body.push_str(&format!(
        "auto_overlay_with_dota = {}\n",
        cfg.auto_overlay_with_dota
    ));
    body.push_str(&format!("overlay_x = {}\n", cfg.overlay_geometry.x));
    body.push_str(&format!("overlay_y = {}\n", cfg.overlay_geometry.y));
    body.push_str(&format!("overlay_width = {}\n", cfg.overlay_geometry.width));
    body.push_str(&format!(
        "overlay_height = {}\n",
        cfg.overlay_geometry.height
    ));
    body.push_str(&format!(
        "overlay_click_through = {}\n",
        cfg.overlay_geometry.click_through
    ));
    body.push_str(&format!(
        "build_overlay_x = {}\n",
        cfg.build_overlay_geometry.x
    ));
    body.push_str(&format!(
        "build_overlay_y = {}\n",
        cfg.build_overlay_geometry.y
    ));
    body.push_str(&format!(
        "build_overlay_width = {}\n",
        cfg.build_overlay_geometry.width
    ));
    body.push_str(&format!(
        "build_overlay_height = {}\n",
        cfg.build_overlay_geometry.height
    ));
    body.push_str(&format!(
        "overlay_opacity = {:.2}\n",
        AppSession::sanitize_overlay_opacity(cfg.overlay_opacity)
    ));
    body.push_str(&format!(
        "ui_language = \"{}\"\n",
        cfg.ui_language.as_config_str()
    ));
    body.push_str(&format!(
        "pass_hotkey = \"{}\"\n",
        cfg.pass_hotkey.format_label()
    ));
    if let Some(token) = cfg.stratz_api_token.as_deref().filter(|t| !t.is_empty()) {
        let escaped = token.replace('"', "");
        body.push_str(&format!("stratz_api_token = \"{escaped}\"\n"));
    }
    if let Some(exp) = cfg.stratz_token_exp {
        body.push_str(&format!("stratz_token_exp = {exp}\n"));
    }

    let write_res: Result<(), std::io::Error> = (|| {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)?;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            f
        };
        #[cfg(not(unix))]
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;

        file.write_all(body.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err.to_string());
    }

    if let Err(err) = std::fs::rename(&tmp_path, &target_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err.to_string());
    }
    Ok(())
}
