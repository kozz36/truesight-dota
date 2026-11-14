use std::sync::mpsc::{self, Receiver, TryRecvError};
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_adapters::sync::LocalCacheManager;
use truesight_adapters::vision::BoundedVisionWorker;
use truesight_core::domain::{Bracket, DraftPick, DraftState, HeroId, Role, Side, TEAM_SIZE};
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

pub struct AppSession {
    pub draft: DraftState,
    pub matrix: MatchupMatrix,
    pub weights: ModelWeights,
    pub cache_mgr: LocalCacheManager,
    pub db_status: String,
    pub last_gsi_state: String,
    pub ally_locked_slots: [bool; TEAM_SIZE],
    pub enemy_locked_slots: [bool; TEAM_SIZE],
    pub manually_dismissed: Vec<HeroId>,
    pub local_player_hero: Option<HeroId>,
    pub sync_in_flight: bool,
    pub last_sync_notice: Option<String>,
    sync_generation: u64,
    sync_rx: Option<Receiver<SyncCompletion>>,
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

fn search_best_role_assignment(
    matrix: &MatchupMatrix,
    heroes: &[HeroId],
    available_slots: &[usize],
    hero_idx: usize,
    used_mask: u8,
    current_slots: &mut Vec<usize>,
    current_score: f32,
    best_score: &mut f32,
    best_slots: &mut Vec<usize>,
) {
    if hero_idx == heroes.len() {
        if current_score > *best_score {
            *best_score = current_score;
            *best_slots = current_slots.clone();
        }
        return;
    }
    for (pos, &slot) in available_slots.iter().enumerate() {
        if (used_mask & (1 << pos)) != 0 {
            continue;
        }
        let aff = hero_role_affinity(matrix, heroes[hero_idx], Role::ALL[slot]);
        current_slots.push(slot);
        search_best_role_assignment(
            matrix,
            heroes,
            available_slots,
            hero_idx + 1,
            used_mask | (1 << pos),
            current_slots,
            current_score + aff,
            best_score,
            best_slots,
        );
        current_slots.pop();
    }
}

impl AppSession {
    pub fn new() -> Result<Self, String> {
        let cache_mgr = LocalCacheManager::auto_detected();
        let repo = cache_mgr.build_repository();
        let matrix = repo
            .load_matrix(Bracket::Legend)
            .map_err(|e| e.to_string())?;
        let db_status = cache_mgr.freshness_badge(LocalCacheManager::now_unix());
        Ok(Self {
            draft: DraftState::default(),
            matrix,
            weights: ModelWeights::default(),
            cache_mgr,
            db_status,
            last_gsi_state: "IDLE (Waiting for Dota 2 GSI)".into(),
            ally_locked_slots: [false; TEAM_SIZE],
            enemy_locked_slots: [false; TEAM_SIZE],
            manually_dismissed: Vec::new(),
            local_player_hero: None,
            sync_in_flight: false,
            last_sync_notice: None,
            sync_generation: 0,
            sync_rx: None,
        })
    }

    pub fn set_bracket(&mut self, bracket: Bracket) {
        self.draft.bracket = bracket;
        if let Ok(m) = self.cache_mgr.build_repository().load_matrix(bracket) {
            self.matrix = m;
        }
    }

    pub fn reload_repository(&mut self) {
        if let Ok(m) = self
            .cache_mgr
            .build_repository()
            .load_matrix(self.draft.bracket)
        {
            self.matrix = m;
        }
        self.db_status = self.cache_mgr.freshness_badge(LocalCacheManager::now_unix());
    }

    pub fn set_account_id(&mut self, account_id: Option<u64>, explicit: bool) {
        self.sync_generation = self.sync_generation.saturating_add(1);
        self.sync_in_flight = false;
        self.sync_rx = None;
        self.cache_mgr.set_account_id(account_id, explicit);
        self.reload_repository();
    }

    pub fn set_account_auto(&mut self) {
        self.sync_generation = self.sync_generation.saturating_add(1);
        self.sync_in_flight = false;
        self.sync_rx = None;
        self.cache_mgr.set_auto_account();
        self.reload_repository();
    }

    pub fn start_background_sync_with<F>(&mut self, force: bool, task: F) -> bool
    where
        F: FnOnce(LocalCacheManager, bool) -> Result<String, String> + Send + 'static,
    {
        if self.sync_in_flight {
            return false;
        }
        let generation = self.sync_generation;
        let account_id = self.cache_mgr.player_account_id;
        let bg_mgr = self.cache_mgr.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("truesight-sync-worker".into())
            .spawn(move || {
                let result = task(bg_mgr, force);
                let _ = tx.send(SyncCompletion {
                    generation,
                    account_id,
                    result,
                });
            })
            .is_ok();
        if spawned {
            self.sync_in_flight = true;
            self.sync_rx = Some(rx);
            self.db_status = "DB [SYNCING...]".into();
        }
        spawned
    }

    pub fn start_background_sync(&mut self, force: bool) -> bool {
        self.start_background_sync_with(force, |mgr, force| {
            mgr.sync_from_opendota(force).map_err(|e| e.to_string())
        })
    }

    pub fn poll_sync_completion(&mut self) {
        let outcome = match self.sync_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(done)) => Some(Ok(done)),
            Some(Err(TryRecvError::Disconnected)) => Some(Err(())),
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        let Some(outcome) = outcome else {
            return;
        };

        self.sync_in_flight = false;
        self.sync_rx = None;

        let Ok(done) = outcome else {
            self.db_status = format!(
                "{} · sync failed",
                self.cache_mgr.freshness_badge(LocalCacheManager::now_unix())
            );
            return;
        };

        if done.generation != self.sync_generation
            || done.account_id != self.cache_mgr.player_account_id
        {
            return;
        }

        match done.result {
            Ok(msg) => {
                let loaded = self
                    .cache_mgr
                    .build_repository()
                    .load_matrix(self.draft.bracket);
                match loaded {
                    Ok(m) if m.heroes().count() > 0 => {
                        self.matrix = m;
                        self.db_status = self.cache_mgr.freshness_badge(LocalCacheManager::now_unix());
                        self.last_sync_notice = Some(msg);
                    }
                    _ => {
                        self.db_status = format!(
                            "{} · reload failed",
                            self.cache_mgr.freshness_badge(LocalCacheManager::now_unix())
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
                    self.cache_mgr.freshness_badge(LocalCacheManager::now_unix())
                );
                self.last_sync_notice = Some(format!("Sync notice: {e} (using last valid model)"));
            }
        }
    }

    pub fn sync_db(&mut self, force: bool) -> Result<String, String> {
        let msg = self
            .cache_mgr
            .sync_from_opendota(force)
            .map_err(|e| e.to_string())?;
        if let Ok(m) = self
            .cache_mgr
            .build_repository()
            .load_matrix(self.draft.bracket)
        {
            self.matrix = m;
        }
        self.db_status = self.cache_mgr.freshness_badge(LocalCacheManager::now_unix());
        Ok(msg)
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
        matches.into_iter().map(|(_, id, name)| (id, name)).collect()
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

        let mut best_score = f32::NEG_INFINITY;
        let mut best_slots = Vec::with_capacity(unlocked_heroes.len());
        let mut current = Vec::with_capacity(unlocked_heroes.len());
        search_best_role_assignment(
            &self.matrix,
            &unlocked_heroes,
            &unlocked_slots,
            0,
            0,
            &mut current,
            0.0,
            &mut best_score,
            &mut best_slots,
        );

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
            }
            DraftEvent::FactionDetected { faction } => {
                self.draft.ally_faction = faction;
            }
            DraftEvent::PlayerSteamIdDetected { steam32 } => {
                if !self.cache_mgr.explicit_override
                    && self.cache_mgr.player_account_id != Some(steam32)
                {
                    self.set_account_id(Some(steam32), false);
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
                    self.draft.ally_faction = faction;
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
                self.apply_side_pick(side, slot, hero_id, role);
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

    pub fn poll_live_sources(
        &mut self,
        gsi_listener: Option<&mut BoundedGsiListener>,
        vision_worker: Option<&mut BoundedVisionWorker>,
    ) {
        self.poll_sync_completion();
        if let Some(listener) = gsi_listener {
            if let Ok(events) = listener.poll_events() {
                for ev in events {
                    self.apply_event(ev);
                }
            }
        }
        if let Some(worker) = vision_worker {
            worker.set_ally_faction(self.draft.ally_faction);
            if let Ok(events) = worker.poll_events() {
                for ev in events {
                    self.apply_event(ev);
                }
            }
        }
    }

    pub fn evaluate(&self) -> (DraftAnalysis5v5, Vec<HeroRecommendation>) {
        let analysis = evaluate_draft_5v5(&self.draft, &self.matrix, &self.weights);
        let recs = recommend_heroes(&self.draft, &self.matrix, &self.weights, 6);
        (analysis, recs)
    }
}
