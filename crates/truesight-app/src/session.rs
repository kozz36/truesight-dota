use std::path::PathBuf;
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_adapters::sync::{
    BracketConfig, LocalCacheManager, config_file_path, load_app_config, load_app_config_from,
    save_bracket_config_to,
};
use truesight_adapters::vision::{BoundedVisionWorker, CaptureAvailability, VisionCaptureMode};
use truesight_core::domain::{
    Bracket, DraftPick, DraftState, Faction, HeroId, Role, Side, TEAM_SIZE,
};
use truesight_core::math::{
    DraftAnalysis5v5, HeroRecommendation, MatchupMatrix, ModelWeights, evaluate_draft_5v5,
    recommend_heroes,
};
use truesight_core::ports::{DraftEvent, DraftEventSource, StatsRepository};

pub(crate) struct SyncCompletion {
    generation: u64,
    account_id: Option<u64>,
    result: Result<String, String>,
}

type BackgroundSyncRunner =
    Arc<dyn Fn(LocalCacheManager, bool) -> Result<String, String> + Send + Sync>;

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
    pub sync_in_flight: bool,
    pub last_sync_notice: Option<String>,
    sync_generation: u64,
    sync_rx: Option<Receiver<SyncCompletion>>,
    sync_workers: Vec<std::thread::JoinHandle<()>>,
    sync_runner: BackgroundSyncRunner,
    last_rules_state: Option<String>,
    vision_reset_pending: bool,
    vision_slots: [[Option<HeroId>; TEAM_SIZE]; 2],
    vision_owned: Vec<(Side, HeroId)>,
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
        cache_mgr: LocalCacheManager,
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
        let db_status = cache_mgr.freshness_badge(LocalCacheManager::now_unix());
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
            sync_in_flight: false,
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
        })
    }

    /// Enables saving manual bracket changes to the OS `config.toml` path for interactive runs.
    pub fn enable_default_config_persistence(&mut self) {
        self.config_path = Some(config_file_path());
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
            return true;
        };

        if done.generation != self.sync_generation
            || done.account_id != self.cache_mgr.player_account_id
        {
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
        true
    }

    pub fn sync_db(&mut self, force: bool) -> Result<String, String> {
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
        self.vision_reset_pending = true;
        self.vision_slots = [[None; TEAM_SIZE]; 2];
        self.vision_owned.clear();
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

        let pick = Some(DraftPick {
            hero_id,
            role: Role::ALL.get(slot).copied(),
        });
        self.side_picks_mut(side)[slot] = pick;
        self.side_locked_mut(side)[slot] = true;
        self.sync_local_player_target_role();
    }

    /// Clears `(side, slot)`, unlocks the slot, and suppresses vision re-adding that hero until reset.
    pub fn clear_slot_manual(&mut self, side: Side, slot: usize) {
        if slot >= TEAM_SIZE {
            return;
        }
        let old = self.side_picks_mut(side)[slot].take();
        self.side_locked_mut(side)[slot] = false;
        if let Some(pick) = old {
            if !self.manually_dismissed.contains(&pick.hero_id) {
                self.manually_dismissed.push(pick.hero_id);
            }
            if side == Side::Allies && self.local_player_hero == Some(pick.hero_id) {
                self.local_player_hero = None;
            }
        }
    }

    pub fn apply_event(&mut self, event: DraftEvent) {
        match event {
            DraftEvent::GameRulesStateChanged {
                state_name,
                game_time_secs,
            } => {
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
                self.last_rules_state = Some(state_name);
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
                    self.local_player_hero = Some(id);
                    self.apply_side_pick(Side::Allies, 0, id, None);
                    self.sync_local_player_target_role();
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
                        self.local_player_hero = Some(id);
                        self.sync_local_player_target_role();
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
        }
    }

    // Reconcile a whole poll before removing identities: a hero can move to another
    // physical portrait in the same batch, including when all role slots are full.
    // Missing events are not removals (death/occlusion grace belongs to the adapter).
    pub(crate) fn apply_vision_events(&mut self, events: Vec<DraftEvent>) {
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
        }
        changed
    }

    /// Switches the user's faction and atomically swaps Allied/Enemy picks and slot locks.
    pub fn set_ally_faction(&mut self, faction: Faction) {
        if self.draft.ally_faction == faction {
            return;
        }
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
        self.sync_local_player_target_role();
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
}
