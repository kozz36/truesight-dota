use crate::domain::{
    Bracket, BuildStep, BuildVariant, CounterTrait, HeroId, HeroItemStat, ItemId, ItemPhase,
    OptionalBuildItem, RecommendedBuild, Role, TEAM_SIZE, all_referenced_counter_item_ids,
    counter_items_for_trait, counter_trait_table_patch, hero_counter_traits,
    is_counter_item_eligible_for_role,
};
use crate::ports::{ItemMetaRepository, PortError};

/// Score proximity threshold for attaching same-phase alternative items as build variants.
pub const ITEM_VARIANT_SCORE_EPSILON: f32 = 0.08;

/// Maximum average-purchase-time difference (minutes) for clustering same-phase variants.
const ITEM_VARIANT_TIME_WINDOW_MINS: f32 = 6.0;

/// Maximum number of primary progression steps in a recommended `[item 1] > [item 2] > ...` bar.
const MAX_PRIMARY_BUILD_STEPS: usize = 6;

/// Maximum number of same-phase alternative variants shown beneath one primary step.
const MAX_VARIANTS_PER_STEP: usize = 3;

/// Maximum number of optional/counter items surfaced in the separate Optional section.
const MAX_OPTIONAL_BUILD_ITEMS: usize = 6;

/// Pure, deterministic item-build recommender combining empirical role/bracket item stats,
/// component completion state, and patch-versioned enemy counter-trait bonuses.
///
/// Timed core progression (`Early`, `Core`, `Late`) is separated strictly from role-filtered
/// enemy counter-trait suggestions (`optional_items`) so situational items like Gem/Dust/Pipe
/// never displace core hero builds or leak carry/offlane items onto unrelated roles.
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
    let mut timed_core_stats: Vec<HeroItemStat> = Vec::new();
    let mut situational_stats: Vec<HeroItemStat> = Vec::new();
    for stat in deduped {
        let owned = owned_items.contains(&stat.item_id);
        match stat.phase {
            ItemPhase::Starting => starting.push(stat),
            ItemPhase::Early | ItemPhase::Core | ItemPhase::Late => {
                // Detection items must never displace timed progression steps; route role-eligible
                // or already-owned ones to Optional/Counters, and keep owned role-ineligible
                // counters under Optional instead of dropping them from inventory visibility.
                if matches!(stat.item_id.0, 30 | 40 | 43) {
                    if owned || is_counter_item_eligible_for_role(stat.item_id, role) {
                        situational_stats.push(HeroItemStat {
                            phase: ItemPhase::Situational,
                            ..stat
                        });
                    }
                    continue;
                }
                let is_counter_item = all_referenced_counter_item_ids().contains(&stat.item_id);
                if is_counter_item && !is_counter_item_eligible_for_role(stat.item_id, role) {
                    if owned {
                        situational_stats.push(HeroItemStat {
                            phase: ItemPhase::Situational,
                            ..stat
                        });
                    }
                    continue;
                }
                timed_core_stats.push(stat);
            }
            ItemPhase::Situational => situational_stats.push(stat),
        }
    }

    starting.sort_by(|a, b| {
        b.match_count
            .cmp(&a.match_count)
            .then_with(|| a.item_id.cmp(&b.item_id))
    });
    let starting_items: Vec<ItemId> = starting.into_iter().take(5).map(|s| s.item_id).collect();

    // Score each timed progression candidate (`Early`, `Core`, `Late`) deterministically.
    let mut scored: Vec<BuildStep> = timed_core_stats
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

    // Classify same-phase alternatives into variants BEFORE applying MAX_PRIMARY_BUILD_STEPS,
    // so competing same-phase alternatives collapse into 1 primary step + compact variants.
    let mut clustered: Vec<BuildStep> = Vec::with_capacity(scored.len());
    let mut unowned_candidates: Vec<BuildStep> = Vec::with_capacity(scored.len());
    for step in scored {
        if step.owned {
            clustered.push(step);
        } else {
            unowned_candidates.push(step);
        }
    }

    for cand in unowned_candidates {
        if let Some(primary) = clustered.iter_mut().find(|step| {
            step.phase == cand.phase
                && (step.score - cand.score).abs() <= ITEM_VARIANT_SCORE_EPSILON
                && (step.avg_time_mins - cand.avg_time_mins).abs() <= ITEM_VARIANT_TIME_WINDOW_MINS
        }) {
            primary.variants.push(BuildVariant {
                item_id: cand.item_id,
                score: cand.score,
                avg_time_mins: cand.avg_time_mins,
                counter_trait: cand.counter_trait,
            });
        } else {
            clustered.push(cand);
        }
    }

    // Select primary steps (always keeping owned items, capping at MAX_PRIMARY_BUILD_STEPS
    // with up to 2 per timed phase so Early -> Core -> Late columns stay populated).
    let mut is_primary = vec![false; clustered.len()];
    let mut primary_count = 0_usize;
    let mut phase_counts = [0_usize; 5];

    for (idx, step) in clustered.iter().enumerate() {
        if step.owned {
            is_primary[idx] = true;
            primary_count += 1;
            phase_counts[phase_slot(step.phase)] += 1;
        }
    }
    for (idx, step) in clustered.iter().enumerate() {
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
    for (idx, _) in clustered.iter().enumerate() {
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
        Vec::with_capacity(clustered.len().saturating_sub(primary_count));
    for (idx, step) in clustered.into_iter().enumerate() {
        if is_primary[idx] {
            steps.push(step);
        } else {
            remainder.push(step);
        }
    }

    // Attach remaining unchosen primary steps as variants if within ITEM_VARIANT_SCORE_EPSILON.
    for cand in remainder {
        if let Some(step) = steps.iter_mut().find(|step| {
            step.phase == cand.phase
                && (step.score - cand.score).abs() <= ITEM_VARIANT_SCORE_EPSILON
        }) {
            step.variants.push(BuildVariant {
                item_id: cand.item_id,
                score: cand.score,
                avg_time_mins: cand.avg_time_mins,
                counter_trait: cand.counter_trait,
            });
        }
    }

    for step in &mut steps {
        step.variants.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.item_id.cmp(&b.item_id))
        });
        step.variants.truncate(MAX_VARIANTS_PER_STEP);
    }

    // Order primary build sequence by effective timing window.
    let game_mins = (game_time_secs.max(0) as f32) / 60.0;
    steps.sort_by(|a, b| {
        effective_step_time(a, game_mins)
            .total_cmp(&effective_step_time(b, game_mins))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });

    // Build separate role-filtered Optional / Counters section (never shown if no enemy trait matches).
    let mut optional_candidates: Vec<OptionalBuildItem> = Vec::new();
    for stat in situational_stats {
        let already_in_core = starting_items.contains(&stat.item_id)
            || steps.iter().any(|s| {
                s.item_id == stat.item_id || s.variants.iter().any(|v| v.item_id == stat.item_id)
            });
        if already_in_core {
            continue;
        }
        let owned = owned_items.contains(&stat.item_id);
        let (matching_enemy_traits, counter_trait) =
            evaluate_enemy_counter_traits(stat.item_id, enemies);
        if !owned
            && (matching_enemy_traits == 0
                || !is_counter_item_eligible_for_role(stat.item_id, role))
        {
            continue;
        }
        let (owned_components, total_components) =
            count_owned_components(repo, stat.item_id, owned_items);
        let component_bonus = if !owned && total_components > 0 {
            0.06 * (owned_components as f32 / total_components as f32)
        } else {
            0.0
        };
        let shrunk_wr = (stat.win_count as f32 + 30.0 * 0.50) / (stat.match_count as f32 + 30.0);
        let volume_score = 0.02 * (1.0 + stat.match_count as f32).ln();
        let counter_bonus = 0.09 * (matching_enemy_traits as f32);
        let score = shrunk_wr + volume_score + component_bonus + counter_bonus;
        optional_candidates.push(OptionalBuildItem {
            item_id: stat.item_id,
            avg_time_mins: stat.avg_time_mins,
            score,
            owned,
            owned_components,
            total_components,
            counter_trait,
        });
    }
    // Preserve any owned counter item in inventory under Optional/Counters even when omitted
    // from role-filtered repository stats, without promoting unowned off-role purchases.
    for &owned_id in owned_items {
        if !all_referenced_counter_item_ids().contains(&owned_id) {
            continue;
        }
        let already_listed = starting_items.contains(&owned_id)
            || steps
                .iter()
                .any(|s| s.item_id == owned_id || s.variants.iter().any(|v| v.item_id == owned_id))
            || optional_candidates.iter().any(|o| o.item_id == owned_id);
        if already_listed {
            continue;
        }
        if let Some(entry) = repo.item_catalog(owned_id)
            && ((entry.cost == 0 && entry.is_consumable_drop) || entry.is_recipe)
        {
            continue;
        }
        let (matching_enemy_traits, counter_trait) =
            evaluate_enemy_counter_traits(owned_id, enemies);
        let (owned_components, total_components) =
            count_owned_components(repo, owned_id, owned_items);
        let fallback_mins = if game_mins > 0.0 {
            game_mins
        } else {
            match owned_id.0 {
                30 | 40 | 43 => 14.0,
                267 | 210 | 226 | 242 | 90 => 22.0,
                _ => 27.0,
            }
        };
        optional_candidates.push(OptionalBuildItem {
            item_id: owned_id,
            avg_time_mins: fallback_mins,
            score: 0.50 + 0.09 * (matching_enemy_traits as f32),
            owned: true,
            owned_components,
            total_components,
            counter_trait,
        });
    }
    optional_candidates.sort_by(|a, b| {
        b.owned
            .cmp(&a.owned)
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.avg_time_mins.total_cmp(&b.avg_time_mins))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });

    // Keep at most 2 detection items (e.g. Dust + Sentry/Gem) so invisibility doesn't flood Optional.
    let mut invis_count = 0_usize;
    let mut optional_items: Vec<OptionalBuildItem> = Vec::with_capacity(MAX_OPTIONAL_BUILD_ITEMS);
    for item in optional_candidates {
        if optional_items.len() >= MAX_OPTIONAL_BUILD_ITEMS {
            break;
        }
        if item.counter_trait == Some(CounterTrait::Invisibility) && !item.owned {
            if invis_count >= 2 {
                continue;
            }
            invis_count += 1;
        }
        optional_items.push(item);
    }

    let next_unowned_index = steps.iter().position(|s| !s.owned);

    Ok(RecommendedBuild {
        target_patch: counter_trait_table_patch(),
        hero_id,
        role,
        bracket,
        starting_items,
        steps,
        optional_items,
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
