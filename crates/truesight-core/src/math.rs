use crate::domain::{
    Bracket, BuildStep, BuildVariant, COUNTER_TRAIT_TABLE_PATCH, CounterTrait, DomainError,
    DraftPick, DraftState, HeroId, HeroItemStat, HeroProfile, ItemId, ItemPhase, MAX_HEROES,
    PairStats, RecommendedBuild, Role, TEAM_SIZE, counter_items_for_trait, hero_counter_traits,
};
use crate::ports::{ItemMetaRepository, PortError};

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
        let smoothed_vs =
            bayesian_winrate(stats.vs_wins, stats.vs_matches, expected_vs, prior_matches);
        let counter_delta = smoothed_vs - expected_vs;

        let expected_with = expected_synergy_wr(wr_a, wr_b);
        let smoothed_with = bayesian_winrate(
            stats.with_wins,
            stats.with_matches,
            expected_with,
            prior_matches,
        );
        let synergy_delta = smoothed_with - expected_with;

        self.cells[ia][ib] = MatchupCell {
            counter_delta,
            synergy_delta,
            vs_matches: stats.vs_matches,
            with_matches: stats.with_matches,
        };
        Ok(())
    }

    pub fn set_calibrated_cell(
        &mut self,
        hero_a: HeroId,
        hero_b: HeroId,
        cell: MatchupCell,
    ) -> Result<(), DomainError> {
        let ia = hero_a.as_index();
        let ib = hero_b.as_index();
        if ia >= MAX_HEROES {
            return Err(DomainError::HeroOutOfBounds(hero_a));
        }
        if ib >= MAX_HEROES {
            return Err(DomainError::HeroOutOfBounds(hero_b));
        }
        self.cells[ia][ib] = cell;
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
                    let sum: f32 = h.primary_roles.iter().map(|&r| h.meta_winrate(r)).sum();
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
    if den <= f32::EPSILON {
        0.50
    } else {
        prod / den
    }
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
    /// Pure arithmetic mean of `counter_grid[ally_slot][..]` against picked enemies.
    pub ally_counter_avg: [Option<f32>; TEAM_SIZE],
    /// Pure arithmetic mean of `ally_synergy_grid[ally_slot][..]` with picked allies.
    pub ally_synergy_avg: [Option<f32>; TEAM_SIZE],
    /// Net contribution per allied slot (meta edge + avg counter + avg synergy).
    pub ally_slot_net_delta: [Option<f32>; TEAM_SIZE],
    /// Net contribution per enemy slot (from enemy perspective).
    pub enemy_slot_net_delta: [Option<f32>; TEAM_SIZE],
    pub total_meta_edge: f32,
    pub total_counter_edge: f32,
    pub total_synergy_edge: f32,
    pub ally_synergy_norm: f32,
    pub enemy_synergy_norm: f32,
    pub total_composition_edge: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeroRecommendation {
    pub hero_id: HeroId,
    pub role: Role,
    /// Exact projected Allied win probability `[0.0, 1.0]` if this hero is picked into the role slot.
    pub projected_win_probability: f32,
    /// Net win probability gain vs the baseline draft (with the target role slot vacant).
    pub net_probability_delta: f32,
    /// Base meta winrate in the requested role (`0.532` = `53.2%`).
    pub base_meta_wr: f32,
    /// Average counter delta against already picked enemies (matches `ally_counter_avg` when picked).
    pub counter_delta_sum: f32,
    /// Average synergy delta with already picked allies (matches `ally_synergy_avg` when picked).
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
    let mut ally_counter_avg = [None; TEAM_SIZE];
    let mut ally_synergy_avg = [None; TEAM_SIZE];
    let mut ally_slot_net_delta = [None; TEAM_SIZE];
    let mut enemy_slot_net_delta = [None; TEAM_SIZE];

    let mut total_meta_edge = 0.0_f32;
    let mut counter_pair_count = 0_u32;
    let mut ally_synergy_count = 0_u32;
    let mut enemy_synergy_count = 0_u32;
    let mut raw_counter_sum = 0.0_f32;
    let mut raw_ally_synergy_sum = 0.0_f32;
    let mut raw_enemy_synergy_sum = 0.0_f32;

    // 1. Base Meta Edge + Per-Slot Initialization
    let mut ally_picked_count = 0_u32;
    let mut enemy_picked_count = 0_u32;

    for (i, slot) in draft.allies.iter().enumerate() {
        if let Some(pick) = slot {
            ally_picked_count += 1;
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
            enemy_picked_count += 1;
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
    let enemy_norm = (enemy_picked_count.max(1)) as f32;
    let ally_norm = (ally_picked_count.max(1)) as f32;

    for (i, ally_opt) in draft.allies.iter().enumerate() {
        let Some(ally) = ally_opt else { continue };
        let mut row_sum = 0.0_f32;
        let mut row_cnt = 0_u32;
        for (j, enemy_opt) in draft.enemies.iter().enumerate() {
            let Some(enemy) = enemy_opt else { continue };
            let delta = matrix.cell(ally.hero_id, enemy.hero_id).counter_delta;
            counter_grid[i][j] = Some(delta);
            row_sum += delta;
            row_cnt += 1;
            raw_counter_sum += delta;
            counter_pair_count += 1;
            if let Some(net) = &mut ally_slot_net_delta[i] {
                *net += delta / enemy_norm;
            }
            if let Some(enemy_net) = &mut enemy_slot_net_delta[j] {
                *enemy_net -= delta / ally_norm;
            }
        }
        if row_cnt > 0 {
            ally_counter_avg[i] = Some(row_sum / row_cnt as f32);
        }
    }

    // 3. Allied Synergy Matrix
    let mut ally_syn_row_sum = [0.0_f32; TEAM_SIZE];
    let mut ally_syn_row_cnt = [0_u32; TEAM_SIZE];

    for i in 0..TEAM_SIZE {
        let Some(ally_a) = draft.allies[i] else {
            continue;
        };
        for k in (i + 1)..TEAM_SIZE {
            let Some(ally_b) = draft.allies[k] else {
                continue;
            };
            let syn = matrix.cell(ally_a.hero_id, ally_b.hero_id).synergy_delta;
            ally_synergy_grid[i][k] = Some(syn);
            ally_synergy_grid[k][i] = Some(syn);
            ally_syn_row_sum[i] += syn;
            ally_syn_row_cnt[i] += 1;
            ally_syn_row_sum[k] += syn;
            ally_syn_row_cnt[k] += 1;
            raw_ally_synergy_sum += syn;
            ally_synergy_count += 1;
            if let Some(net_a) = &mut ally_slot_net_delta[i] {
                *net_a += syn / (ally_norm - 1.0).max(1.0);
            }
            if let Some(net_b) = &mut ally_slot_net_delta[k] {
                *net_b += syn / (ally_norm - 1.0).max(1.0);
            }
        }
    }
    for i in 0..TEAM_SIZE {
        if ally_syn_row_cnt[i] > 0 {
            ally_synergy_avg[i] = Some(ally_syn_row_sum[i] / ally_syn_row_cnt[i] as f32);
        }
    }

    // 4. Enemy Synergy Subtraction
    for j in 0..TEAM_SIZE {
        let Some(enemy_a) = draft.enemies[j] else {
            continue;
        };
        for l in (j + 1)..TEAM_SIZE {
            let Some(enemy_b) = draft.enemies[l] else {
                continue;
            };
            let syn = matrix.cell(enemy_a.hero_id, enemy_b.hero_id).synergy_delta;
            raw_enemy_synergy_sum += syn;
            enemy_synergy_count += 1;
            if let Some(net_a) = &mut enemy_slot_net_delta[j] {
                *net_a += syn / (enemy_norm - 1.0).max(1.0);
            }
            if let Some(net_b) = &mut enemy_slot_net_delta[l] {
                *net_b += syn / (enemy_norm - 1.0).max(1.0);
            }
        }
    }

    // Normalize team-wide edges by √N (correlated 5v5 interactions in a single match)
    // so a full 25-pair counter matrix scales by √25 = 5 instead of raw 25x summation.
    total_meta_edge /= 2.236_068; // √5
    let total_counter_edge = if counter_pair_count > 0 {
        raw_counter_sum / (counter_pair_count as f32).sqrt()
    } else {
        0.0
    };
    let ally_syn_norm = if ally_synergy_count > 0 {
        raw_ally_synergy_sum / (ally_synergy_count as f32).sqrt()
    } else {
        0.0
    };
    let enemy_syn_norm = if enemy_synergy_count > 0 {
        raw_enemy_synergy_sum / (enemy_synergy_count as f32).sqrt()
    } else {
        0.0
    };
    let total_synergy_edge = ally_syn_norm - enemy_syn_norm;
    let ally_comp = evaluate_team_composition(&draft.allies, matrix);
    let enemy_comp = evaluate_team_composition(&draft.enemies, matrix);
    let total_composition_edge = ally_comp - enemy_comp;

    // Personal comfort contribution for the user's selected role slot (`draft.target_role`)
    let target_slot = draft.target_role.index();
    let player_comfort_edge = draft.allies[target_slot]
        .and_then(|p| matrix.hero(p.hero_id))
        .map(|h| h.comfort_delta())
        .unwrap_or(0.0);

    // Combine normalized edges in logit space (scale factor 3.2 maps realistic draft edges into [32%, 68%])
    let combined_logit = 3.2
        * (weights.meta_weight * total_meta_edge
            + weights.counter_weight * total_counter_edge
            + weights.synergy_weight * total_synergy_edge
            + 0.85 * total_composition_edge
            + (weights.comfort_weight / 2.236_068) * player_comfort_edge);

    let ally_win_probability = sigmoid(combined_logit);

    DraftAnalysis5v5 {
        ally_win_probability,
        counter_grid,
        ally_synergy_grid,
        ally_counter_avg,
        ally_synergy_avg,
        ally_slot_net_delta,
        enemy_slot_net_delta,
        total_meta_edge,
        total_counter_edge,
        total_synergy_edge,
        ally_synergy_norm: ally_syn_norm,
        enemy_synergy_norm: enemy_syn_norm,
        total_composition_edge,
    }
}

pub fn evaluate_team_composition(
    picks: &[Option<DraftPick>; TEAM_SIZE],
    matrix: &MatchupMatrix,
) -> f32 {
    use crate::domain::AttackType;

    let mut picked = 0_u32;
    let mut melee = 0_u32;
    let mut ranged = 0_u32;
    let mut disables = 0_u32;
    let mut initiators = 0_u32;

    for pick in picks.iter().flatten() {
        if let Some(h) = matrix.hero(pick.hero_id) {
            picked += 1;
            match h.attack_type {
                AttackType::Melee => melee += 1,
                AttackType::Ranged => ranged += 1,
            }
            if h.has_disable {
                disables += 1;
            }
            if h.has_initiation {
                initiators += 1;
            }
        }
    }

    if picked < 3 {
        return 0.0;
    }

    let mut score = 0.0_f32;
    if melee == 5 {
        score -= 0.018;
    } else if melee == 4 && picked >= 4 {
        score -= 0.007;
    } else if ranged == 5 {
        score -= 0.008;
    }

    if picked >= 4 && disables == 0 {
        score -= 0.015;
    } else if picked == 5 && disables == 1 {
        score -= 0.005;
    }

    if picked >= 4 && (1..=3).contains(&melee) && disables >= 2 && initiators >= 1 {
        score += 0.005;
    }

    score
}

/// Evaluates all available heroes for the target slot (`draft.target_role.index()`)
/// against the draft *without* that slot's current occupant, so `Projected Win%`
/// and `Draft Gain` match the exact 5v5 evaluation when picked into that slot.
pub fn recommend_heroes(
    draft: &DraftState,
    matrix: &MatchupMatrix,
    weights: &ModelWeights,
    top_k: usize,
) -> Vec<HeroRecommendation> {
    let role = draft.target_role;
    let target_slot = role.index();

    // Baseline is the draft with `target_slot` vacant so replacing an already-filled
    // slot still compares against the 4 remaining allies + 5 enemies.
    let mut base_draft = *draft;
    base_draft.allies[target_slot] = None;
    let baseline = evaluate_draft_5v5(&base_draft, matrix, weights);

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
        let mut enemy_cnt = 0_u32;
        for enemy in draft.enemies.iter().flatten() {
            counter_delta_sum += matrix.cell(profile.id, enemy.hero_id).counter_delta;
            enemy_cnt += 1;
        }
        if enemy_cnt > 0 {
            counter_delta_sum /= enemy_cnt as f32;
        }

        let mut synergy_delta_sum = 0.0_f32;
        let mut ally_cnt = 0_u32;
        for (idx, ally_opt) in draft.allies.iter().enumerate() {
            if idx == target_slot {
                continue;
            }
            if let Some(ally) = ally_opt {
                synergy_delta_sum += matrix.cell(profile.id, ally.hero_id).synergy_delta;
                ally_cnt += 1;
            }
        }
        if ally_cnt > 0 {
            synergy_delta_sum /= ally_cnt as f32;
        }

        let comfort_delta = profile.comfort_delta();

        let mut simulated = *draft;
        simulated.allies[target_slot] = Some(DraftPick {
            hero_id: profile.id,
            role: Some(role),
        });
        let sim_eval = evaluate_draft_5v5(&simulated, matrix, weights);
        let projected_win_probability = sim_eval.ally_win_probability;
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
            .then_with(|| b.comfort_delta.total_cmp(&a.comfort_delta))
            .then_with(|| a.hero_id.cmp(&b.hero_id))
    });
    recommendations.truncate(top_k);
    recommendations
}

#[inline]
fn default_role_for_slot(slot: usize) -> Role {
    Role::ALL.get(slot).copied().unwrap_or(Role::Carry)
}

/// Score proximity threshold for attaching same-phase alternative items as build variants.
pub const ITEM_VARIANT_SCORE_EPSILON: f32 = 0.08;

/// Maximum number of primary progression steps in a recommended `[item 1] > [item 2] > ...` bar.
const MAX_PRIMARY_BUILD_STEPS: usize = 6;

/// Pure, deterministic item-build recommender combining empirical role/bracket item stats,
/// component completion state, and patch-versioned enemy counter-trait bonuses.
pub fn recommend_item_build(
    repo: &dyn ItemMetaRepository,
    hero_id: HeroId,
    role: Role,
    bracket: Bracket,
    enemies: &[Option<HeroId>; TEAM_SIZE],
    owned_items: &[ItemId],
    game_time_secs: i32,
) -> Result<RecommendedBuild, PortError> {
    let raw_stats = repo.hero_item_stats(hero_id, role, bracket)?;

    // Deduplicate by ItemId (keeping highest match_count) and exclude zero-cost drops / recipes.
    let mut deduped: Vec<HeroItemStat> = Vec::with_capacity(raw_stats.len());
    for stat in raw_stats {
        if let Some(entry) = repo.item_catalog(stat.item_id)
            && ((entry.cost == 0 && entry.is_consumable_drop) || entry.is_recipe)
        {
            continue;
        }
        if let Some(existing) = deduped.iter_mut().find(|s| s.item_id == stat.item_id) {
            if stat.match_count > existing.match_count
                || (stat.match_count == existing.match_count && stat.win_count > existing.win_count)
            {
                *existing = stat;
            }
        } else {
            deduped.push(stat);
        }
    }

    let mut starting: Vec<HeroItemStat> = Vec::new();
    let mut progression_stats: Vec<HeroItemStat> = Vec::new();
    for stat in deduped {
        if stat.phase == ItemPhase::Starting {
            starting.push(stat);
        } else {
            progression_stats.push(stat);
        }
    }

    starting.sort_by(|a, b| {
        b.match_count
            .cmp(&a.match_count)
            .then_with(|| a.item_id.cmp(&b.item_id))
    });
    let starting_items: Vec<ItemId> = starting.into_iter().map(|s| s.item_id).collect();

    // Score each progression candidate deterministically.
    let mut scored: Vec<BuildStep> = progression_stats
        .into_iter()
        .map(|stat| {
            let owned = owned_items.contains(&stat.item_id);
            let (owned_components, total_components) =
                count_owned_components(repo, stat.item_id, owned_items);
            let component_bonus = if !owned && total_components > 0 {
                0.06 * (owned_components as f32 / total_components as f32)
            } else {
                0.0
            };

            let (matching_enemy_traits, counter_trait) =
                evaluate_enemy_counter_traits(stat.item_id, enemies);
            let counter_bonus = 0.09 * (matching_enemy_traits as f32);

            let shrunk_wr =
                (stat.win_count as f32 + 30.0 * 0.50) / (stat.match_count as f32 + 30.0);
            let volume_score = 0.02 * (1.0 + stat.match_count as f32).ln();
            let score = shrunk_wr + volume_score + component_bonus + counter_bonus;

            BuildStep {
                item_id: stat.item_id,
                phase: stat.phase,
                avg_time_mins: stat.avg_time_mins,
                score,
                owned,
                owned_components,
                total_components,
                counter_trait,
                variants: Vec::new(),
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.avg_time_mins.total_cmp(&b.avg_time_mins))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });

    // Select primary steps (always keeping owned items, capping at MAX_PRIMARY_BUILD_STEPS
    // with up to 2 per phase on the first pass so the bar spans Early -> Core -> Late).
    let mut is_primary = vec![false; scored.len()];
    let mut primary_count = 0_usize;
    let mut phase_counts = [0_usize; 5];

    for (idx, step) in scored.iter().enumerate() {
        if step.owned {
            is_primary[idx] = true;
            primary_count += 1;
            phase_counts[phase_slot(step.phase)] += 1;
        }
    }
    for (idx, step) in scored.iter().enumerate() {
        if primary_count >= MAX_PRIMARY_BUILD_STEPS {
            break;
        }
        if is_primary[idx] {
            continue;
        }
        let slot = phase_slot(step.phase);
        if phase_counts[slot] < 2 {
            is_primary[idx] = true;
            primary_count += 1;
            phase_counts[slot] += 1;
        }
    }
    for (idx, _) in scored.iter().enumerate() {
        if primary_count >= MAX_PRIMARY_BUILD_STEPS {
            break;
        }
        if !is_primary[idx] {
            is_primary[idx] = true;
            primary_count += 1;
        }
    }

    let mut steps: Vec<BuildStep> = Vec::with_capacity(primary_count);
    let mut remainder: Vec<BuildStep> =
        Vec::with_capacity(scored.len().saturating_sub(primary_count));
    for (idx, step) in scored.into_iter().enumerate() {
        if is_primary[idx] {
            steps.push(step);
        } else {
            remainder.push(step);
        }
    }

    // Attach same-phase alternative items within ITEM_VARIANT_SCORE_EPSILON of each step's score.
    for step in &mut steps {
        let mut variants: Vec<BuildVariant> = remainder
            .iter()
            .filter(|cand| {
                cand.phase == step.phase
                    && (step.score - cand.score).abs() <= ITEM_VARIANT_SCORE_EPSILON
            })
            .map(|cand| BuildVariant {
                item_id: cand.item_id,
                score: cand.score,
                avg_time_mins: cand.avg_time_mins,
                counter_trait: cand.counter_trait,
            })
            .collect();
        variants.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.item_id.cmp(&b.item_id))
        });
        step.variants = variants;
    }

    // Order primary build sequence by effective timing window.
    let game_mins = (game_time_secs.max(0) as f32) / 60.0;
    steps.sort_by(|a, b| {
        effective_step_time(a, game_mins)
            .total_cmp(&effective_step_time(b, game_mins))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });

    let next_unowned_index = steps.iter().position(|s| !s.owned);

    Ok(RecommendedBuild {
        target_patch: COUNTER_TRAIT_TABLE_PATCH,
        hero_id,
        role,
        bracket,
        starting_items,
        steps,
        next_unowned_index,
    })
}

fn count_owned_components(
    repo: &dyn ItemMetaRepository,
    item_id: ItemId,
    owned_items: &[ItemId],
) -> (usize, usize) {
    let Some(entry) = repo.item_catalog(item_id) else {
        return (0, 0);
    };
    let total = entry.components.len();
    if total == 0 || owned_items.is_empty() {
        return (0, total);
    }
    let mut remaining: Vec<ItemId> = owned_items.to_vec();
    let mut owned_count = 0_usize;
    for comp in &entry.components {
        if let Some(pos) = remaining.iter().position(|id| id == comp) {
            remaining.swap_remove(pos);
            owned_count += 1;
        }
    }
    (owned_count, total)
}

fn evaluate_enemy_counter_traits(
    item_id: ItemId,
    enemies: &[Option<HeroId>; TEAM_SIZE],
) -> (u32, Option<CounterTrait>) {
    let mut count = 0_u32;
    let mut primary: Option<CounterTrait> = None;
    for enemy_id in enemies.iter().flatten() {
        for &trait_kind in hero_counter_traits(*enemy_id) {
            if counter_items_for_trait(trait_kind).contains(&item_id) {
                count += 1;
                if primary.is_none() {
                    primary = Some(trait_kind);
                }
            }
        }
    }
    (count, primary)
}

#[inline]
fn effective_step_time(step: &BuildStep, game_mins: f32) -> f32 {
    let counter_pull = if step.counter_trait.is_some() {
        2.0
    } else {
        0.0
    };
    let component_pull = if !step.owned && step.owned_components > 0 {
        1.5
    } else {
        0.0
    };
    let overdue_pull = if !step.owned && step.avg_time_mins + 5.0 < game_mins {
        1.0
    } else {
        0.0
    };
    step.avg_time_mins - counter_pull - component_pull - overdue_pull
}

#[inline]
const fn phase_slot(phase: ItemPhase) -> usize {
    match phase {
        ItemPhase::Starting => 0,
        ItemPhase::Early => 1,
        ItemPhase::Core => 2,
        ItemPhase::Late => 3,
        ItemPhase::Situational => 4,
    }
}
