#[cfg(feature = "gui")]
mod gui;

use std::io::{self, BufRead, Write};
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_adapters::sync::{LocalCacheManager, normalize_steam32_id};
use truesight_adapters::vision::BoundedVisionWorker;
use truesight_core::domain::{Bracket, DraftPick, DraftState, Faction, HeroId, Role, Side, TEAM_SIZE};
use truesight_core::math::{
    DraftAnalysis5v5, HeroRecommendation, MatchupMatrix, ModelWeights, evaluate_draft_5v5,
    recommend_heroes,
};
use truesight_core::ports::{DraftEvent, DraftEventSource, StatsRepository};

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_GREEN: &str = "\x1b[38;2;46;204;113m";
const ANSI_RED: &str = "\x1b[38;2;231;76;60m";
const ANSI_CYAN: &str = "\x1b[38;2;52;152;219m";
const ANSI_GOLD: &str = "\x1b[38;2;241;196;15m";

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
        self.cache_mgr.set_account_id(account_id, explicit);
        self.reload_repository();
    }

    pub fn set_account_auto(&mut self) {
        self.cache_mgr.set_auto_account();
        self.reload_repository();
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

pub fn render_dashboard_string(session: &AppSession, color: bool) -> String {
    let (analysis, recs) = session.evaluate();
    let mut out = String::with_capacity(4096);

    let (b, r, g, rd, c, y, d) = if color {
        (
            ANSI_BOLD, ANSI_RESET, ANSI_GREEN, ANSI_RED, ANSI_CYAN, ANSI_GOLD, ANSI_DIM,
        )
    } else {
        ("", "", "", "", "", "", "")
    };

    let ally_pct = analysis.ally_win_probability * 100.0;
    let enemy_pct = 100.0 - ally_pct;
    let win_color = if ally_pct >= 50.0 { g } else { rd };

    out.push_str(&format!(
        "{b}========================================================================================{r}\n"
    ));
    out.push_str(&format!(
        "{b} TRUESIGHT-DOTA · 5v5 LIVE DRAFT ORACLE{r}   | Bracket: {c}{}{r} | Target: {y}{}{r} | Side: {d}{}{r}\n",
        session.draft.bracket.label(),
        session.draft.target_role.label(),
        session.draft.ally_faction.label()
    ));
    out.push_str(&format!(
        " GSI State: {d}{}{r}   |   {d}{}{r}\n",
        session.last_gsi_state, session.db_status
    ));
    out.push_str(&format!(
        "{b}----------------------------------------------------------------------------------------{r}\n"
    ));
    out.push_str(&format!(
        " {b}WIN PROBABILITY:{r}  Allies {win_color}{b}{ally_pct:5.1}%{r}  vs  Enemies {b}{enemy_pct:5.1}%{r}   \
         (Meta: {:+5.1}% | Counter: {:+5.1}% | Synergy: {:+5.1}% [A:{:+.1}%/E:{:+.1}%] | Comp: {:+5.1}%)\n",
        analysis.total_meta_edge * 100.0,
        analysis.total_counter_edge * 100.0,
        analysis.total_synergy_edge * 100.0,
        analysis.ally_synergy_norm * 100.0,
        analysis.enemy_synergy_norm * 100.0,
        analysis.total_composition_edge * 100.0,
    ));
    out.push_str(&format!(
        "{b}========================================================================================{r}\n\n"
    ));

    // 5v5 Counter Matrix Header
    out.push_str(&format!(
        "{b} [5v5 COUNTER MATRIX — Allies (Rows) vs Enemies (Columns)]{r}\n"
    ));
    out.push_str(&format!("{:<22}", " ALLY \\ ENEMY"));
    for j in 0..TEAM_SIZE {
        let col_title = session.draft.enemies[j]
            .and_then(|p| session.matrix.hero(p.hero_id))
            .map(|h| truncate_name(&h.display_name, 10))
            .unwrap_or_else(|| format!("Enemy #{}", j + 1));
        out.push_str(&format!("| {:^10} ", col_title));
    }
    out.push_str("| Avg Cntr \n");
    out.push_str(
        "----------------------+------------+------------+------------+------------+------------+-----------\n",
    );

    for i in 0..TEAM_SIZE {
        let ally_label = session.draft.allies[i]
            .and_then(|p| session.matrix.hero(p.hero_id))
            .map(|h| format!("A{}: {}", i + 1, truncate_name(&h.display_name, 17)))
            .unwrap_or_else(|| format!("A{}: [Empty Slot]", i + 1));
        out.push_str(&format!(" {:<21}", ally_label));

        for j in 0..TEAM_SIZE {
            if let Some(delta) = analysis.counter_grid[i][j] {
                let pct = delta * 100.0;
                let cell_color = if pct > 0.15 {
                    g
                } else if pct < -0.15 {
                    rd
                } else {
                    d
                };
                out.push_str(&format!("| {cell_color}{pct:+9.2}%{r} "));
            } else {
                out.push_str(&format!("| {d}{:^10}{r} ", "·"));
            }
        }

        if let Some(avg_c) = analysis.ally_counter_avg[i] {
            let pct = avg_c * 100.0;
            let nc = if pct >= 0.0 { g } else { rd };
            out.push_str(&format!("| {nc}{b}{pct:+8.2}%{r}\n"));
        } else {
            out.push_str(&format!("| {d}{:^9}{r}\n", "—"));
        }
    }

    // Allied Synergy Summary
    out.push_str(&format!("\n{b} [ALLIED SYNERGY PAIRS]{r}\n "));
    let mut any_syn = false;
    for i in 0..TEAM_SIZE {
        for k in (i + 1)..TEAM_SIZE {
            if let Some(syn) = analysis.ally_synergy_grid[i][k] {
                any_syn = true;
                let name_a = session.draft.allies[i]
                    .and_then(|p| session.matrix.hero(p.hero_id))
                    .map(|h| h.display_name.as_str())
                    .unwrap_or("?");
                let name_b = session.draft.allies[k]
                    .and_then(|p| session.matrix.hero(p.hero_id))
                    .map(|h| h.display_name.as_str())
                    .unwrap_or("?");
                let pct = syn * 100.0;
                let sc = if pct >= 0.0 { g } else { rd };
                out.push_str(&format!("{name_a} + {name_b}: {sc}{pct:+.2}%{r}   "));
            }
        }
    }
    if !any_syn {
        out.push_str(&format!("{d}Pick at least 2 allied heroes to see pairwise synergy deltas.{r}"));
    }
    out.push_str("\n\n");

    // Top Hero Recommendations
    out.push_str(&format!(
        "{b} [TOP RECOMMENDED PICKS FOR {}]{r}\n",
        session.draft.target_role.label()
    ));
    out.push_str(
        "  #  Hero                 | Proj Win% | Draft Gain | Meta WR  | vs Enemy | w/ Allies | Comfort \n",
    );
    out.push_str(
        " -------------------------+-----------+------------+----------+----------+-----------+---------\n",
    );

    for (rank, rec) in recs.iter().enumerate() {
        let name = session
            .matrix
            .hero(rec.hero_id)
            .map(|h| h.display_name.as_str())
            .unwrap_or("Unknown");
        let gain_pct = rec.net_probability_delta * 100.0;
        let cnt_pct = rec.counter_delta_sum * 100.0;
        let syn_pct = rec.synergy_delta_sum * 100.0;
        let com_pct = rec.comfort_delta * 100.0;

        let gc = if gain_pct >= 0.0 { g } else { rd };
        let cc = if cnt_pct >= 0.0 { g } else { rd };
        let sc = if syn_pct >= 0.0 { g } else { rd };

        out.push_str(&format!(
            " {:>2}. {:<20} | {:>8.2}% | {gc}{b}{:>9.2}%{r} | {:>7.2}% | {cc}{:>7.2}%{r} | {sc}{:>8.2}%{r} | {:>+6.2}%\n",
            rank + 1,
            truncate_name(name, 20),
            rec.projected_win_probability * 100.0,
            gain_pct,
            rec.base_meta_wr * 100.0,
            cnt_pct,
            syn_pct,
            com_pct,
        ));
    }

    out
}

fn truncate_name(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max_len.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn execute_side_pick_command(
    session: &mut AppSession,
    side: Side,
    parts: &[&str],
) -> Result<(), String> {
    let usage = match side {
        Side::Allies => "Usage: ally <1..5> <hero_name>",
        Side::Enemies => "Usage: enemy <1..5> <hero_name>",
    };
    if parts.len() < 3 {
        return Err(usage.into());
    }
    let slot = parts[1]
        .parse::<usize>()
        .map_err(|_| "Slot must be 1..5".to_string())?;
    if !(1..=5).contains(&slot) {
        return Err("Slot must be between 1 and 5".into());
    }
    let query = parts[2..].join(" ");
    if query == "none" || query == "-" {
        session.clear_slot_manual(side, slot - 1);
    } else {
        let hero_id = session
            .find_hero(&query)
            .ok_or_else(|| format!("Hero not found: '{query}'"))?;
        session.set_slot_manual(side, slot - 1, hero_id);
    }
    Ok(())
}

pub fn execute_command(session: &mut AppSession, line: &str) -> Result<bool, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let Some(&cmd) = parts.first() else {
        return Ok(true);
    };

    match cmd {
        "q" | "quit" | "exit" => return Ok(false),
        "reset" | "clear" => {
            session.reset_draft();
        }
        "sync" => {
            let _ = session.sync_db(true)?;
        }
        "role" => {
            let Some(&r_str) = parts.get(1) else {
                return Err("Usage: role <1..5>".into());
            };
            session.draft.target_role = match r_str {
                "1" | "carry" => Role::Carry,
                "2" | "mid" => Role::Mid,
                "3" | "off" | "offlane" => Role::Offlane,
                "4" | "soft" => Role::SoftSupport,
                "5" | "hard" | "sup" => Role::HardSupport,
                _ => return Err("Role must be 1..5".into()),
            };
        }
        "bracket" | "medal" => {
            let Some(&b_str) = parts.get(1) else {
                return Err(
                    "Usage: bracket <herald|guardian|crusader|archon|legend|ancient|divine|immortal>"
                        .into(),
                );
            };
            let bracket = match b_str.to_ascii_lowercase().as_str() {
                "1" | "herald" => Bracket::Herald,
                "2" | "guardian" => Bracket::Guardian,
                "3" | "crusader" => Bracket::Crusader,
                "4" | "archon" => Bracket::Archon,
                "5" | "legend" => Bracket::Legend,
                "6" | "ancient" => Bracket::Ancient,
                "7" | "divine" => Bracket::Divine,
                "8" | "immortal" => Bracket::Immortal,
                _ => {
                    return Err(
                        "Bracket must be 1..8 or herald..immortal".into(),
                    );
                }
            };
            session.set_bracket(bracket);
        }
        "faction" => {
            let Some(&f_str) = parts.get(1) else {
                return Err("Usage: faction <radiant|dire>".into());
            };
            session.draft.ally_faction = match f_str.to_ascii_lowercase().as_str() {
                "radiant" | "r" => Faction::Radiant,
                "dire" | "d" => Faction::Dire,
                _ => return Err("Faction must be radiant or dire".into()),
            };
        }
        "account" | "steam" => {
            let Some(&a_str) = parts.get(1) else {
                return Err("Usage: account <steam32|steam64|auto|none>".into());
            };
            let lower = a_str.to_ascii_lowercase();
            if lower == "auto" {
                session.set_account_auto();
            } else if lower == "none" || lower == "unlink" || lower == "unlinked" {
                session.set_account_id(None, true);
            } else {
                let raw = a_str
                    .parse::<u64>()
                    .map_err(|_| "Account ID must be a numeric Steam32/Steam64, 'auto', or 'none'".to_string())?;
                let s32 = normalize_steam32_id(raw)
                    .ok_or_else(|| "Invalid Steam32/Steam64 account ID".to_string())?;
                session.set_account_id(Some(s32), true);
            }
        }
        "ally" | "a" => execute_side_pick_command(session, Side::Allies, &parts)?,
        "enemy" | "e" => execute_side_pick_command(session, Side::Enemies, &parts)?,
        "swap" => {
            if parts.len() != 4 {
                return Err("Usage: swap <ally|enemy> <1..5> <1..5>".into());
            }
            let side = match parts[1].to_ascii_lowercase().as_str() {
                "ally" | "allies" | "a" => Side::Allies,
                "enemy" | "enemies" | "e" => Side::Enemies,
                _ => return Err("Side must be ally or enemy".into()),
            };
            let a = parts[2]
                .parse::<usize>()
                .map_err(|_| "Slot must be 1..5".to_string())?;
            let b = parts[3]
                .parse::<usize>()
                .map_err(|_| "Slot must be 1..5".to_string())?;
            if !(1..=5).contains(&a) || !(1..=5).contains(&b) {
                return Err("Slot must be between 1 and 5".into());
            }
            session.swap_slots(side, a - 1, b - 1);
        }
        "ban" | "b" => {
            if parts.len() < 2 {
                return Err("Usage: ban <hero_name>".into());
            }
            let query = parts[1..].join(" ");
            let hero_id = session
                .find_hero(&query)
                .ok_or_else(|| format!("Hero not found: '{query}'"))?;
            session
                .draft
                .toggle_ban(hero_id)
                .map_err(|e| e.to_string())?;
        }
        _ => {
            return Err(
                "Commands: ally <1..5> <hero> | enemy <1..5> <hero> | swap <ally|enemy> <1..5> <1..5> | ban <hero> | role <1..5> | bracket <medal> | faction <radiant|dire> | account <id|auto|none> | sync | reset | quit"
                    .into(),
            );
        }
    }
    Ok(true)
}

fn parse_account_id_arg(args: &[String]) -> Result<Option<Option<u64>>, String> {
    let mut i = 1_usize;
    while i < args.len() {
        let val = if args[i] == "--account-id" {
            args.get(i + 1).map(String::as_str)
        } else if let Some(rest) = args[i].strip_prefix("--account-id=") {
            Some(rest)
        } else {
            None
        };
        if let Some(raw_str) = val {
            let trimmed = raw_str.trim();
            if trimmed.eq_ignore_ascii_case("none") || trimmed.eq_ignore_ascii_case("unlinked") {
                return Ok(Some(None));
            }
            if trimmed.eq_ignore_ascii_case("auto") {
                return Ok(None);
            }
            let raw = trimmed
                .parse::<u64>()
                .map_err(|_| format!("Invalid --account-id value: '{trimmed}'"))?;
            let s32 = normalize_steam32_id(raw)
                .ok_or_else(|| format!("Out-of-range Steam ID: '{trimmed}'"))?;
            return Ok(Some(Some(s32)));
        }
        if args[i] == "--account-id" {
            return Err("Missing value after --account-id".into());
        }
        i += 1;
    }
    Ok(None)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut session = match AppSession::new() {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(io::stderr(), "Failed to initialize truesight-dota: {e}");
            std::process::exit(1);
        }
    };
    match parse_account_id_arg(&args) {
        Ok(Some(opt_id)) => session.set_account_id(opt_id, true),
        Ok(None) => {}
        Err(e) => {
            let _ = writeln!(io::stderr(), "{e}");
            std::process::exit(1);
        }
    }

    if args.iter().any(|a| a == "--sync") {
        match session.sync_db(true) {
            Ok(msg) => {
                let _ = writeln!(io::stdout(), "{msg} ({})", session.db_status);
                return;
            }
            Err(e) => {
                let _ = writeln!(io::stderr(), "Sync failed: {e}");
                std::process::exit(1);
            }
        }
    }

    if args.iter().any(|a| a == "--demo") {
        let _ = execute_command(&mut session, "ally 3 magnus");
        let _ = execute_command(&mut session, "ally 5 crystal maiden");
        let _ = execute_command(&mut session, "enemy 1 medusa");
        let _ = execute_command(&mut session, "enemy 2 storm spirit");
        let _ = execute_command(&mut session, "enemy 3 axe");
        let _ = execute_command(&mut session, "role 1");
        let no_color = args.iter().any(|a| a == "--no-color");
        if writeln!(io::stdout(), "{}", render_dashboard_string(&session, !no_color)).is_err() {
            std::process::exit(0);
        }
        return;
    }

    // Background non-blocking auto-sync when cache TTL (24h meta / 1h player) has expired
    {
        let now = LocalCacheManager::now_unix();
        if session.cache_mgr.is_meta_stale(now) || session.cache_mgr.is_player_stale(now) {
            let bg_mgr = session.cache_mgr.clone();
            std::thread::spawn(move || {
                let _ = bg_mgr.sync_from_opendota(false);
            });
        }
    }

    let gsi_addr = std::env::var("TRUESIGHT_GSI_ADDR").unwrap_or_else(|_| "127.0.0.1:4001".into());
    let mut gsi_listener = BoundedGsiListener::bind(&gsi_addr).ok();
    if let Some(ref l) = gsi_listener {
        session.last_gsi_state = format!("LISTENING on http://{}", l.local_addr());
    }
    let mut vision_worker = BoundedVisionWorker::spawn();

    #[cfg(feature = "gui")]
    {
        let force_cli = args.iter().any(|a| a == "--cli" || a == "--tui");
        if !force_cli {
            if let Err(e) = gui::run_gui(session, gsi_listener, vision_worker) {
                let _ = writeln!(io::stderr(), "GUI error: {e}");
                std::process::exit(1);
            }
            return;
        }
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        session.poll_live_sources(gsi_listener.as_mut(), vision_worker.as_mut());

        if write!(
            stdout,
            "\x1b[2J\x1b[H{}\n{ANSI_BOLD}truesight>{ANSI_RESET} (ally <1..5> <hero> | enemy <1..5> <hero> | role <1..5> | reset | quit): ",
            render_dashboard_string(&session, true)
        )
        .is_err()
            || stdout.flush().is_err()
        {
            break;
        }

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => match execute_command(&mut session, &line) {
                Ok(true) => {}
                Ok(false) => break,
                Err(msg) => {
                    let _ = writeln!(stdout, "{ANSI_RED}Error: {msg}{ANSI_RESET}");
                }
            },
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppSession, execute_command, render_dashboard_string};

    #[test]
    fn interactive_commands_update_5v5_table_and_recommendations() {
        let mut session = AppSession::new().unwrap();
        assert!(execute_command(&mut session, "ally 3 magnus").unwrap());
        assert!(execute_command(&mut session, "enemy 1 medusa").unwrap());
        assert!(execute_command(&mut session, "enemy 2 storm").unwrap());
        assert!(execute_command(&mut session, "role 1").unwrap());

        let rendered = render_dashboard_string(&session, false);
        assert!(rendered.contains("TRUESIGHT-DOTA · 5v5 LIVE DRAFT ORACLE"));
        assert!(rendered.contains("A3: Magnus"));
        assert!(rendered.contains("Medusa"));
        assert!(rendered.contains("Storm Spi"));
        assert!(rendered.contains("1. Anti-Mage"));

        assert!(execute_command(&mut session, "faction dire").unwrap());
        assert_eq!(session.draft.ally_faction, truesight_core::domain::Faction::Dire);
        session.apply_event(truesight_core::ports::DraftEvent::FactionDetected {
            faction: truesight_core::domain::Faction::Radiant,
        });
        assert_eq!(session.draft.ally_faction, truesight_core::domain::Faction::Radiant);

        assert!(execute_command(&mut session, "account none").unwrap());
        assert_eq!(session.cache_mgr.player_account_id, None);
        assert!(session.db_status.contains("Player: unlinked"));

        assert!(execute_command(&mut session, "account 76561198061460213").unwrap());
        assert_eq!(session.cache_mgr.player_account_id, Some(101_194_485));
        assert!(session.db_status.contains("Player #101194485"));

        // Explicit account override must not be overwritten by GSI SteamID
        session.apply_event(truesight_core::ports::DraftEvent::PlayerSteamIdDetected {
            steam32: 222_333_444,
        });
        assert_eq!(session.cache_mgr.player_account_id, Some(101_194_485));

        // In Auto mode, GSI SteamID updates active player account
        assert!(execute_command(&mut session, "account auto").unwrap());
        session.apply_event(truesight_core::ports::DraftEvent::PlayerSteamIdDetected {
            steam32: 222_333_444,
        });
        assert_eq!(session.cache_mgr.player_account_id, Some(222_333_444));
    }

    #[test]
    fn bracket_command_and_slot_search_filter_work_across_all_eight_medals() {
        let mut session = AppSession::new().unwrap();
        assert!(execute_command(&mut session, "ally 1 faceless void").unwrap());
        assert!(execute_command(&mut session, "ally 2 invoker").unwrap());
        assert!(execute_command(&mut session, "ally 3 magnus").unwrap());
        assert!(execute_command(&mut session, "ally 4 spirit breaker").unwrap());
        assert!(execute_command(&mut session, "ally 5 crystal maiden").unwrap());
        assert!(execute_command(&mut session, "enemy 1 medusa").unwrap());
        assert!(execute_command(&mut session, "enemy 2 storm spirit").unwrap());
        assert!(execute_command(&mut session, "enemy 3 axe").unwrap());
        assert!(execute_command(&mut session, "enemy 4 shadow demon").unwrap());
        assert!(execute_command(&mut session, "enemy 5 abaddon").unwrap());

        assert!(execute_command(&mut session, "bracket herald").unwrap());
        let (eval_herald, _) = session.evaluate();

        assert!(execute_command(&mut session, "bracket immortal").unwrap());
        let (eval_immortal, _) = session.evaluate();

        assert!(
            (eval_herald.total_counter_edge - eval_immortal.total_counter_edge).abs() > 0.001,
            "Switching bracket via command must update Counter Edge"
        );
        assert!(
            (eval_herald.total_synergy_edge - eval_immortal.total_synergy_edge).abs() > 0.0002,
            "Switching bracket via command must update Synergy Edge"
        );

        let matches = session.filter_heroes_for_slot(truesight_core::domain::Side::Allies, 1, "inv");
        assert!(!matches.is_empty());
        assert_eq!(matches[0].1, "Invoker");
    }

    #[test]
    fn pick_detected_assigns_roles_by_affinity_preserves_user_locks_and_tracks_local_hero() {
        use truesight_core::domain::{Faction, HeroId, Role, Side};
        use truesight_core::ports::DraftEvent;

        let mut session = AppSession::new().unwrap();
        session.set_account_id(Some(111_222_333), true);

        // Live GSI local player Night Stalker (60) auto-places into Pos 3 Offlane and sets target_role
        session.apply_event(DraftEvent::LocalPlayerHeroDetected {
            hero_slug: "npc_dota_hero_night_stalker".into(),
        });
        assert_eq!(session.local_player_hero, Some(HeroId(60)));
        assert_eq!(session.draft.allies[2].unwrap().hero_id, HeroId(60));
        assert_eq!(session.draft.allies[2].unwrap().role, Some(Role::Offlane));
        assert_eq!(session.draft.target_role, Role::Offlane);

        // Feeding Allied replay top-bar lobby order assigns optimal P1..P5 roles rather than lobby slot indices
        for (slot, hid) in [(0, 67_u16), (1, 60), (2, 27), (3, 64), (4, 128)] {
            session.apply_event(DraftEvent::PickDetected {
                side: Side::Allies,
                slot,
                hero_id: HeroId(hid),
                role: Role::ALL.get(slot).copied(),
            });
        }
        assert_eq!(session.draft.allies[0].unwrap().hero_id, HeroId(67)); // Spectre -> P1 Carry
        assert_eq!(session.draft.allies[1].unwrap().hero_id, HeroId(128)); // Snapfire -> P2 Mid
        assert_eq!(session.draft.allies[2].unwrap().hero_id, HeroId(60)); // Night Stalker -> P3 Offlane
        assert_eq!(session.draft.allies[3].unwrap().hero_id, HeroId(27)); // Shadow Shaman -> P4
        assert_eq!(session.draft.allies[4].unwrap().hero_id, HeroId(64)); // Jakiro -> P5
        assert_eq!(session.draft.target_role, Role::Offlane);

        // Enemy top-bar picks: Slark=93, Viper=47, Tusk=100, Beastmaster=38, Clinkz=56
        for (slot, hid) in [(0, 93_u16), (1, 47), (2, 100), (3, 38), (4, 56)] {
            session.apply_event(DraftEvent::PickDetected {
                side: Side::Enemies,
                slot,
                hero_id: HeroId(hid),
                role: Role::ALL.get(slot).copied(),
            });
        }
        assert_eq!(session.draft.enemies[0].unwrap().hero_id, HeroId(93));
        let viper_slot = session
            .draft
            .enemies
            .iter()
            .position(|p| p.map(|d| d.hero_id) == Some(HeroId(47)))
            .expect("Viper placed on Enemies");
        assert!(viper_slot == 1 || viper_slot == 2);

        // User manually swaps Viper to P5 (slot 4) and Beastmaster to P3 (slot 2)
        session.swap_slots(Side::Enemies, viper_slot, 4);
        let bm_slot = session
            .draft
            .enemies
            .iter()
            .position(|p| p.map(|d| d.hero_id) == Some(HeroId(38)))
            .expect("Beastmaster placed on Enemies");
        if bm_slot != 2 {
            session.swap_slots(Side::Enemies, bm_slot, 2);
        }
        assert!(session.enemy_locked_slots[4]);
        assert!(session.enemy_locked_slots[2]);
        assert_eq!(session.draft.enemies[4].unwrap().hero_id, HeroId(47));
        assert_eq!(session.draft.enemies[4].unwrap().role, Some(Role::HardSupport));
        assert_eq!(session.draft.enemies[2].unwrap().hero_id, HeroId(38));
        assert_eq!(session.draft.enemies[2].unwrap().role, Some(Role::Offlane));

        // Subsequent vision PickDetected frames must respect locked slots
        session.apply_event(DraftEvent::PickDetected {
            side: Side::Enemies,
            slot: 1,
            hero_id: HeroId(47),
            role: Some(Role::Mid),
        });
        assert_eq!(session.draft.enemies[4].unwrap().hero_id, HeroId(47));
        assert_eq!(session.draft.enemies[2].unwrap().hero_id, HeroId(38));

        // Clearing Spectre (67) removes it and suppresses re-adding from subsequent vision polls until reset
        session.clear_slot_manual(Side::Allies, 0);
        assert!(session.draft.allies[0].is_none());
        session.apply_event(DraftEvent::PickDetected {
            side: Side::Allies,
            slot: 0,
            hero_id: HeroId(67),
            role: Some(Role::Carry),
        });
        assert!(session.draft.allies[0].is_none());

        // Replay PlayerHeroMapped matches linked Steam32 ID to faction, local hero, and role
        session.apply_event(DraftEvent::PlayerHeroMapped {
            steam32: 111_222_333,
            faction: Faction::Dire,
            hero_slug: "npc_dota_hero_night_stalker".into(),
        });
        assert_eq!(session.draft.ally_faction, Faction::Dire);
        assert_eq!(session.local_player_hero, Some(HeroId(60)));
        assert_eq!(session.draft.target_role, Role::Offlane);
    }
}
