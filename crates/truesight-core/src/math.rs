use crate::domain::{
    DomainError, DraftPick, DraftState, HeroId, HeroProfile, MAX_HEROES, PairStats, Role, TEAM_SIZE,
};

/// Default empirical Bayes prior strength (virtual matches at 50%/expected winrate).
pub const DEFAULT_BAYESIAN_PRIOR_MATCHES: f32 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelWeights {
    pub meta_weight: f32,
    pub counter_weight: f32,
    pub synergy_weight: f32,
    pub comfort_weight: f32,
    pub prior_matches: f32,
}

impl Default for ModelWeights {
    fn default() -> Self {
        Self {
            meta_weight: 1.0,
            counter_weight: 1.25,
            synergy_weight: 0.90,
            comfort_weight: 0.65,
            prior_matches: DEFAULT_BAYESIAN_PRIOR_MATCHES,
        }
    }
}

/// Precomputed cell in the dense O(1) hero interaction matrix.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MatchupCell {
    /// Positive means Hero A counters Hero B above their baseline expectation.
    pub counter_delta: f32,
    /// Positive means Hero A and Hero B perform better together than expected independently.
    pub synergy_delta: f32,
    pub vs_matches: u32,
    pub with_matches: u32,
}

/// Dense `MAX_HEROES x MAX_HEROES` lookup matrix for zero-allocation 5v5 evaluation.
#[derive(Debug, Clone)]
pub struct MatchupMatrix {
    heroes: Vec<Option<HeroProfile>>,
    cells: Box<[[MatchupCell; MAX_HEROES]; MAX_HEROES]>,
}

impl Default for MatchupMatrix {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchupMatrix {
    pub fn new() -> Self {
        Self {
            heroes: (0..MAX_HEROES).map(|_| None).collect(),
            cells: Box::new([[MatchupCell::default(); MAX_HEROES]; MAX_HEROES]),
        }
    }

    pub fn upsert_hero(&mut self, profile: HeroProfile) -> Result<(), DomainError> {
        let idx = profile.id.as_index();
        if idx >= MAX_HEROES {
            return Err(DomainError::HeroOutOfBounds(profile.id));
        }
        self.heroes[idx] = Some(profile);
        Ok(())
    }

    #[inline]
    pub fn hero(&self, id: HeroId) -> Option<&HeroProfile> {
        self.heroes.get(id.as_index()).and_then(Option::as_ref)
    }

    pub fn heroes(&self) -> impl Iterator<Item = &HeroProfile> {
        self.heroes.iter().flatten()
    }

    /// Sets pairwise stats between `hero_a` and `hero_b` and computes empirical-Bayes
    /// normalized counter and synergy deltas (STRATZ / Dotabuff expected-winrate baseline).
    pub fn set_pair_stats(
        &mut self,
        hero_a: HeroId,
        hero_b: HeroId,
        stats: PairStats,
        prior_matches: f32,
    ) -> Result<(), DomainError> {
        let ia = hero_a.as_index();
        let ib = hero_b.as_index();
        if ia >= MAX_HEROES {
            return Err(DomainError::HeroOutOfBounds(hero_a));
        }
        if ib >= MAX_HEROES {
            return Err(DomainError::HeroOutOfBounds(hero_b));
        }
        let wr_a = self.hero_best_wr(hero_a);
        let wr_b = self.hero_best_wr(hero_b);

        let expected_vs = expected_head_to_head_wr(wr_a, wr_b);
        let smoothed_vs = bayesian_winrate(stats.vs_wins, stats.vs_matches, expected_vs, prior_matches);
        let counter_delta = smoothed_vs - expected_vs;

        let expected_with = expected_synergy_wr(wr_a, wr_b);
        let smoothed_with =
            bayesian_winrate(stats.with_wins, stats.with_matches, expected_with, prior_matches);
        let synergy_delta = smoothed_with - expected_with;

        self.cells[ia][ib] = MatchupCell {
            counter_delta,
            synergy_delta,
            vs_matches: stats.vs_matches,
            with_matches: stats.with_matches,
        };
        Ok(())
    }

    #[inline]
    pub fn cell(&self, hero_a: HeroId, hero_b: HeroId) -> MatchupCell {
        let ia = hero_a.as_index();
        let ib = hero_b.as_index();
        if ia < MAX_HEROES && ib < MAX_HEROES {
            self.cells[ia][ib]
        } else {
            MatchupCell::default()
        }
    }

    fn hero_best_wr(&self, id: HeroId) -> f32 {
        self.hero(id)
            .map(|h| {
                if h.primary_roles.is_empty() {
                    h.role_winrates
                        .iter()
                        .copied()
                        .fold(0.50_f32, f32::max)
                        .clamp(0.05, 0.95)
                } else {
                    let sum: f32 = h
                        .primary_roles
                        .iter()
                        .map(|&r| h.meta_winrate(r))
                        .sum();
                    (sum / h.primary_roles.len() as f32).clamp(0.05, 0.95)
                }
            })
            .unwrap_or(0.50)
    }
}

/// Log-odds expected head-to-head winrate of Hero A against Hero B given their base winrates.
#[inline]
pub fn expected_head_to_head_wr(wr_a: f32, wr_b: f32) -> f32 {
    let a = wr_a.clamp(0.05, 0.95);
    let b = wr_b.clamp(0.05, 0.95);
    let num = a * (1.0 - b);
    let den = num + (1.0 - a) * b;
    if den <= f32::EPSILON { 0.50 } else { num / den }
}

/// Log-odds expected joint winrate when Hero A and Hero B are on the same team.
#[inline]
pub fn expected_synergy_wr(wr_a: f32, wr_b: f32) -> f32 {
    let a = wr_a.clamp(0.05, 0.95);
    let b = wr_b.clamp(0.05, 0.95);
    let prod = a * b;
    let den = prod + (1.0 - a) * (1.0 - b);
    if den <= f32::EPSILON { 0.50 } else { prod / den }
}

/// Empirical Bayes shrinkage toward `prior_wr`.
#[inline]
pub fn bayesian_winrate(wins: u32, matches: u32, prior_wr: f32, prior_matches: f32) -> f32 {
    let m = prior_matches.max(1.0);
    (wins as f32 + m * prior_wr) / (matches as f32 + m)
}

#[inline]
pub fn logit(p: f32) -> f32 {
    let clamped = p.clamp(0.001, 0.999);
    (clamped / (1.0 - clamped)).ln()
}

#[inline]
pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Complete 5v5 draft evaluation output rendered directly by the UI / TUI table.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftAnalysis5v5 {
    /// Probability `[0.0, 1.0]` that the Allied draft wins the match.
    pub ally_win_probability: f32,
    /// `counter_grid[ally_slot][enemy_slot]` = normalized counter delta in `[-1.0, 1.0]`.
    /// Green when `> 0.0`, Red when `< 0.0`.
    pub counter_grid: [[Option<f32>; TEAM_SIZE]; TEAM_SIZE],
    /// `ally_synergy_grid[ally_a][ally_b]` = normalized synergy delta between allies.
    pub ally_synergy_grid: [[Option<f32>; TEAM_SIZE]; TEAM_SIZE],
    /// Net contribution per allied slot (meta edge + sum of counters + sum of synergies).
    pub ally_slot_net_delta: [Option<f32>; TEAM_SIZE],
    /// Net contribution per enemy slot (from enemy perspective).
    pub enemy_slot_net_delta: [Option<f32>; TEAM_SIZE],
    pub total_meta_edge: f32,
    pub total_counter_edge: f32,
    pub total_synergy_edge: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeroRecommendation {
    pub hero_id: HeroId,
    pub role: Role,
    /// Projected Allied win probability `[0.0, 1.0]` if this hero is picked now.
    pub projected_win_probability: f32,
    /// Net win probability gain vs current draft state.
    pub net_probability_delta: f32,
    /// Base meta winrate in the requested role (`0.532` = `53.2%`).
    pub base_meta_wr: f32,
    /// Sum of counter deltas against already picked enemies (green/red).
    pub counter_delta_sum: f32,
    /// Sum of synergy deltas with already picked allies (green/red).
    pub synergy_delta_sum: f32,
    /// Player personal comfort delta (`0.0` if no personal games recorded).
    pub comfort_delta: f32,
}

pub fn evaluate_draft_5v5(
    draft: &DraftState,
    matrix: &MatchupMatrix,
    weights: &ModelWeights,
) -> DraftAnalysis5v5 {
    let mut counter_grid = [[None; TEAM_SIZE]; TEAM_SIZE];
    let mut ally_synergy_grid = [[None; TEAM_SIZE]; TEAM_SIZE];
    let mut ally_slot_net_delta = [None; TEAM_SIZE];
    let mut enemy_slot_net_delta = [None; TEAM_SIZE];

    let mut total_meta_edge = 0.0_f32;
    let mut total_counter_edge = 0.0_f32;
    let mut total_synergy_edge = 0.0_f32;

    // 1. Base Meta Edge + Per-Slot Initialization
    for (i, slot) in draft.allies.iter().enumerate() {
        if let Some(pick) = slot {
            let role = pick.role.unwrap_or_else(|| default_role_for_slot(i));
            let base_wr = matrix
                .hero(pick.hero_id)
                .map(|h| h.meta_winrate(role))
                .unwrap_or(0.50);
            let meta_delta = base_wr - 0.50;
            total_meta_edge += meta_delta;
            ally_slot_net_delta[i] = Some(meta_delta);
        }
    }

    for (j, slot) in draft.enemies.iter().enumerate() {
        if let Some(pick) = slot {
            let role = pick.role.unwrap_or_else(|| default_role_for_slot(j));
            let base_wr = matrix
                .hero(pick.hero_id)
                .map(|h| h.meta_winrate(role))
                .unwrap_or(0.50);
            let meta_delta = base_wr - 0.50;
            total_meta_edge -= meta_delta;
            enemy_slot_net_delta[j] = Some(meta_delta);
        }
    }

    // 2. 5v5 Counter Matrix (Allies vs Enemies)
    for (i, ally_opt) in draft.allies.iter().enumerate() {
        let Some(ally) = ally_opt else { continue };
        for (j, enemy_opt) in draft.enemies.iter().enumerate() {
            let Some(enemy) = enemy_opt else { continue };
            let delta = matrix.cell(ally.hero_id, enemy.hero_id).counter_delta;
            counter_grid[i][j] = Some(delta);
            total_counter_edge += delta;
            if let Some(net) = &mut ally_slot_net_delta[i] {
                *net += delta;
            }
            if let Some(enemy_net) = &mut enemy_slot_net_delta[j] {
                *enemy_net -= delta;
            }
        }
    }

    // 3. Allied Synergy Matrix
    for i in 0..TEAM_SIZE {
        let Some(ally_a) = draft.allies[i] else { continue };
        for k in (i + 1)..TEAM_SIZE {
            let Some(ally_b) = draft.allies[k] else { continue };
            let syn = matrix.cell(ally_a.hero_id, ally_b.hero_id).synergy_delta;
            ally_synergy_grid[i][k] = Some(syn);
            ally_synergy_grid[k][i] = Some(syn);
            total_synergy_edge += syn;
            if let Some(net_a) = &mut ally_slot_net_delta[i] {
                *net_a += 0.5 * syn;
            }
            if let Some(net_b) = &mut ally_slot_net_delta[k] {
                *net_b += 0.5 * syn;
            }
        }
    }

    // 4. Enemy Synergy Subtraction
    for j in 0..TEAM_SIZE {
        let Some(enemy_a) = draft.enemies[j] else { continue };
        for l in (j + 1)..TEAM_SIZE {
            let Some(enemy_b) = draft.enemies[l] else { continue };
            let syn = matrix.cell(enemy_a.hero_id, enemy_b.hero_id).synergy_delta;
            total_synergy_edge -= syn;
            if let Some(net_a) = &mut enemy_slot_net_delta[j] {
                *net_a += 0.5 * syn;
            }
            if let Some(net_b) = &mut enemy_slot_net_delta[l] {
                *net_b += 0.5 * syn;
            }
        }
    }

    // Combine edges in logit space (scale factor 4.0 converts linear WR delta around 0.5 to log-odds)
    let combined_logit = 4.0
        * (weights.meta_weight * total_meta_edge
            + weights.counter_weight * total_counter_edge
            + weights.synergy_weight * total_synergy_edge);

    let ally_win_probability = sigmoid(combined_logit);

    DraftAnalysis5v5 {
        ally_win_probability,
        counter_grid,
        ally_synergy_grid,
        ally_slot_net_delta,
        enemy_slot_net_delta,
        total_meta_edge,
        total_counter_edge,
        total_synergy_edge,
    }
}

/// Evaluates all available heroes for the first empty allied slot (or `draft.target_role`)
/// and returns them sorted by highest projected match win probability.
pub fn recommend_heroes(
    draft: &DraftState,
    matrix: &MatchupMatrix,
    weights: &ModelWeights,
    top_k: usize,
) -> Vec<HeroRecommendation> {
    let baseline = evaluate_draft_5v5(draft, matrix, weights);
    let target_slot = draft
        .allies
        .iter()
        .position(Option::is_none)
        .unwrap_or(draft.target_role.index());
    let role = draft.target_role;

    let mut recommendations = Vec::new();

    for profile in matrix.heroes() {
        if draft.is_unavailable(profile.id) {
            continue;
        }
        if !profile.primary_roles.is_empty() && !profile.primary_roles.contains(&role) {
            continue;
        }

        let base_meta_wr = profile.meta_winrate(role);

        let mut counter_delta_sum = 0.0_f32;
        for enemy in draft.enemies.iter().flatten() {
            counter_delta_sum += matrix.cell(profile.id, enemy.hero_id).counter_delta;
        }

        let mut synergy_delta_sum = 0.0_f32;
        for (idx, ally_opt) in draft.allies.iter().enumerate() {
            if idx == target_slot {
                continue;
            }
            if let Some(ally) = ally_opt {
                synergy_delta_sum += matrix.cell(profile.id, ally.hero_id).synergy_delta;
            }
        }

        let comfort_delta = if profile.personal_matches > 0 {
            let personal_wr = bayesian_winrate(
                profile.personal_wins,
                profile.personal_matches,
                0.50,
                15.0,
            );
            (personal_wr - 0.50).clamp(-0.12, 0.12)
        } else {
            0.0
        };

        let mut simulated = *draft;
        simulated.allies[target_slot] = Some(DraftPick {
            hero_id: profile.id,
            role: Some(role),
        });
        let sim_eval = evaluate_draft_5v5(&simulated, matrix, weights);
        let logit_with_comfort =
            logit(sim_eval.ally_win_probability) + 4.0 * weights.comfort_weight * comfort_delta;
        let projected_win_probability = sigmoid(logit_with_comfort);
        let net_probability_delta = projected_win_probability - baseline.ally_win_probability;

        recommendations.push(HeroRecommendation {
            hero_id: profile.id,
            role,
            projected_win_probability,
            net_probability_delta,
            base_meta_wr,
            counter_delta_sum,
            synergy_delta_sum,
            comfort_delta,
        });
    }

    recommendations.sort_by(|a, b| {
        b.projected_win_probability
            .total_cmp(&a.projected_win_probability)
            .then_with(|| a.hero_id.cmp(&b.hero_id))
    });
    recommendations.truncate(top_k);
    recommendations
}

#[inline]
fn default_role_for_slot(slot: usize) -> Role {
    Role::ALL.get(slot).copied().unwrap_or(Role::Carry)
}
