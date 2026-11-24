use crate::domain::{
    Bracket, BuildStep, BuildVariant, CounterTrait, HeroId, HeroItemStat, ItemId, ItemPhase,
    OptionalBuildItem, RecommendedBuild, Role, TEAM_SIZE, all_referenced_counter_item_ids,
    counter_items_for_trait, counter_trait_table_patch, hero_counter_traits,
    is_counter_item_eligible_for_role,
};
use crate::ports::{ItemMetaRepository, PortError};

/// Score proximity threshold for attaching same-phase alternative items as build variants.
pub const ITEM_VARIANT_SCORE_EPSILON: f32 = 0.08;

/// Slightly wider score proximity threshold for attaching Late-game variant options.
const LATE_VARIANT_SCORE_EPSILON: f32 = 0.12;

/// Maximum average-purchase-time difference (minutes) for clustering same-phase variants.
const ITEM_VARIANT_TIME_WINDOW_MINS: f32 = 6.0;

/// Tight timing window (minutes) for collapsing two competing items in an unfilled phase slot.
const COMPETING_SLOT_TIME_WINDOW_MINS: f32 = 1.5;

/// Tight score window for collapsing two competing items in an unfilled phase slot.
const COMPETING_SLOT_SCORE_EPSILON: f32 = 0.05;

/// Minimum timing separation (minutes) before a later parent upgrade to preserve a valuable stepping-stone item.
const SUBCOMPONENT_SEPARATION_MINS: f32 = 4.0;

/// Minimum phase-leader match-count ratio required to promote a secondary Early/Core primary step.
const EARLY_CORE_LEADER_MIN_SHARE: f32 = 0.10;

/// Minimum phase-leader match-count ratio required to promote an extra Late primary step.
const LATE_LEADER_MIN_SHARE: f32 = 0.04;

/// Score continuity bonus awarded to Late upgrades whose prerequisite component is in the build.
const UPGRADE_CHAIN_BONUS: f32 = 0.035;

/// Maximum number of primary progression steps in a recommended `[item 1] > [item 2] > ...` bar.
const MAX_PRIMARY_BUILD_STEPS: usize = 8;

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
                if !owned
                    && (is_excluded_basic_subcomponent(stat.item_id)
                        || (matches!(stat.phase, ItemPhase::Core | ItemPhase::Late)
                            && is_early_only_aura_component(stat.item_id)))
                {
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

    // Suppress unowned stepping-stone sub-components when a viable parent upgrade exists in the
    // same phase or within SUBCOMPONENT_SEPARATION_MINS; preserve valuable stepping stones
    // (e.g. Yasha / Crystalys / Ultimate Orb) bought >= 4m before a later-phase parent.
    let timed_snapshot: Vec<HeroItemStat> = timed_core_stats.clone();
    timed_core_stats.retain(|stat| {
        if owned_items.contains(&stat.item_id) {
            return true;
        }
        if is_excluded_basic_subcomponent(stat.item_id)
            || (matches!(stat.phase, ItemPhase::Core | ItemPhase::Late)
                && is_early_only_aura_component(stat.item_id))
        {
            return false;
        }
        if !is_valuable_stepping_stone_id(stat.item_id) {
            return true;
        }
        !timed_snapshot.iter().any(|parent| {
            parent.item_id != stat.item_id
                && (owned_items.contains(&parent.item_id)
                    || parent.match_count.saturating_mul(4) >= stat.match_count)
                && (parent.phase == stat.phase
                    || (parent.avg_time_mins - stat.avg_time_mins) < SUBCOMPONENT_SEPARATION_MINS)
                && repo
                    .item_catalog(parent.item_id)
                    .is_some_and(|entry| entry.components.contains(&stat.item_id))
        })
    });

    // Apply a tiny-sample noise floor when sufficient match volume exists so rare 100%-WR
    // late luxuries do not displace high-sample core builds, while protecting explicit Late
    // upgrade chains (e.g. Skull Basher -> Abyssal Blade, Eul's -> Wind Waker).
    let mut protected_upgrade_ids: Vec<ItemId> = Vec::new();
    let max_mc = timed_core_stats
        .iter()
        .map(|s| s.match_count)
        .max()
        .unwrap_or(0);
    if max_mc >= 50 {
        let min_mc = ((max_mc as f32 * 0.025).ceil() as u32).clamp(8, 25);
        let upgrade_floor = (min_mc / 2).clamp(5, 12);
        let viable_parent_bases: Vec<ItemId> = timed_core_stats
            .iter()
            .filter(|s| owned_items.contains(&s.item_id) || s.match_count >= min_mc)
            .map(|s| s.item_id)
            .collect();
        for stat in &timed_core_stats {
            if stat.phase == ItemPhase::Late
                && stat.item_id.0 != 271
                && stat.match_count >= upgrade_floor
                && repo.item_catalog(stat.item_id).is_some_and(|entry| {
                    entry.components.iter().any(|comp| {
                        viable_parent_bases.contains(comp)
                            && *comp != stat.item_id
                            && !is_early_boots(*comp)
                            && comp.0 != 29
                    })
                })
            {
                protected_upgrade_ids.push(stat.item_id);
            }
        }
        let retained_count = timed_core_stats
            .iter()
            .filter(|s| {
                owned_items.contains(&s.item_id)
                    || s.match_count >= min_mc
                    || protected_upgrade_ids.contains(&s.item_id)
            })
            .count();
        if retained_count >= 3 {
            timed_core_stats.retain(|s| {
                owned_items.contains(&s.item_id)
                    || s.match_count >= min_mc
                    || protected_upgrade_ids.contains(&s.item_id)
            });
        }
    } else {
        let all_timed_ids: Vec<ItemId> = timed_core_stats.iter().map(|s| s.item_id).collect();
        for stat in &timed_core_stats {
            if stat.phase == ItemPhase::Late
                && stat.item_id.0 != 271
                && repo.item_catalog(stat.item_id).is_some_and(|entry| {
                    entry.components.iter().any(|comp| {
                        all_timed_ids.contains(comp)
                            && *comp != stat.item_id
                            && !is_early_boots(*comp)
                            && comp.0 != 29
                    })
                })
            {
                protected_upgrade_ids.push(stat.item_id);
            }
        }
    }

    struct ScoredCandidate {
        step: BuildStep,
        match_count: u32,
        is_protected_upgrade: bool,
    }

    // Score each timed progression candidate (`Early`, `Core`, `Late`) deterministically.
    let mut scored: Vec<ScoredCandidate> = timed_core_stats
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
            let is_protected_upgrade = protected_upgrade_ids.contains(&stat.item_id);
            let upgrade_bonus = if is_protected_upgrade {
                UPGRADE_CHAIN_BONUS
            } else {
                0.0
            };

            let shrunk_wr =
                (stat.win_count as f32 + 60.0 * 0.50) / (stat.match_count as f32 + 60.0);
            let volume_score = 0.06 * (1.0 + stat.match_count as f32).ln();
            let score = shrunk_wr + volume_score + component_bonus + counter_bonus + upgrade_bonus;

            let step_phase = if is_early_boots(stat.item_id) && stat.avg_time_mins <= 16.0 {
                ItemPhase::Early
            } else {
                stat.phase
            };
            ScoredCandidate {
                step: BuildStep {
                    item_id: stat.item_id,
                    phase: step_phase,
                    avg_time_mins: stat.avg_time_mins,
                    score,
                    owned,
                    owned_components,
                    total_components,
                    counter_trait,
                    variants: Vec::new(),
                },
                match_count: stat.match_count,
                is_protected_upgrade,
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.step
            .score
            .total_cmp(&a.step.score)
            .then_with(|| a.step.avg_time_mins.total_cmp(&b.step.avg_time_mins))
            .then_with(|| a.step.item_id.cmp(&b.step.item_id))
    });

    let mut phase_max_mc = [0_u32; 5];
    let mut late_nonfiller_total = 0_usize;
    for cand in &scored {
        let slot = phase_slot(cand.step.phase);
        phase_max_mc[slot] = phase_max_mc[slot].max(cand.match_count);
    }
    for cand in &scored {
        if cand.step.phase == ItemPhase::Late
            && (cand.step.owned
                || cand.is_protected_upgrade
                || !is_phase_filler(
                    cand.step.phase,
                    cand.match_count,
                    phase_max_mc[phase_slot(cand.step.phase)],
                ))
        {
            late_nonfiller_total += 1;
        }
    }

    // Classify mutually exclusive boots and competing tight-window alternatives BEFORE
    // filling phase primary caps (Early: 2, Core: 2, Late: 4), while preserving distinct
    // sequential phase progression items and protected Late builds.
    let mut clustered: Vec<ScoredCandidate> = Vec::with_capacity(scored.len());
    let mut unowned_candidates: Vec<ScoredCandidate> = Vec::with_capacity(scored.len());
    for cand in scored {
        if cand.step.owned {
            clustered.push(cand);
        } else {
            unowned_candidates.push(cand);
        }
    }

    for cand in unowned_candidates {
        let matches_boot = |c: &ScoredCandidate| {
            is_early_boots(c.step.item_id)
                || (is_any_boots(c.step.item_id)
                    && (c.step.phase == cand.step.phase || c.step.phase == ItemPhase::Early))
        };
        let slot = phase_slot(cand.step.phase);
        let cand_is_filler = !cand.is_protected_upgrade
            && is_phase_filler(cand.step.phase, cand.match_count, phase_max_mc[slot]);
        let existing_in_phase = clustered
            .iter()
            .filter(|c| {
                c.step.phase == cand.step.phase
                    && (c.step.owned
                        || c.is_protected_upgrade
                        || !is_phase_filler(
                            c.step.phase,
                            c.match_count,
                            phase_max_mc[phase_slot(c.step.phase)],
                        ))
            })
            .count();
        let phase_cap = phase_primary_cap(cand.step.phase);

        let cand_is_early_boot = is_early_boots(cand.step.item_id)
            || (cand.step.phase == ItemPhase::Early && is_any_boots(cand.step.item_id));

        let matched = if cand_is_early_boot && clustered.iter().any(matches_boot) {
            clustered.iter_mut().find(|c| matches_boot(c))
        } else if cand.step.phase == ItemPhase::Late
            && is_late_boots(cand.step.item_id)
            && clustered
                .iter()
                .any(|c| c.step.phase == ItemPhase::Late && is_late_boots(c.step.item_id))
        {
            clustered
                .iter_mut()
                .find(|c| c.step.phase == ItemPhase::Late && is_late_boots(c.step.item_id))
        } else if cand.step.item_id.0 == 271 && clustered.iter().any(|c| c.step.item_id.0 == 108) {
            clustered.iter_mut().find(|c| c.step.item_id.0 == 108)
        } else if cand_is_filler
            || (cand.step.phase == ItemPhase::Late
                && late_nonfiller_total >= 3
                && existing_in_phase < phase_cap)
        {
            None
        } else if existing_in_phase < phase_cap {
            clustered.iter_mut().find(|c| {
                c.step.phase == cand.step.phase
                    && (c.step.score - cand.step.score).abs() <= COMPETING_SLOT_SCORE_EPSILON
                    && (c.step.avg_time_mins - cand.step.avg_time_mins).abs()
                        <= COMPETING_SLOT_TIME_WINDOW_MINS
            })
        } else {
            let eps = variant_epsilon(cand.step.phase);
            clustered.iter_mut().find(|c| {
                c.step.phase == cand.step.phase
                    && (c.step.score - cand.step.score).abs() <= eps
                    && (c.step.avg_time_mins - cand.step.avg_time_mins).abs()
                        <= ITEM_VARIANT_TIME_WINDOW_MINS
            })
        };
        if let Some(primary) = matched {
            primary.step.variants.push(BuildVariant {
                item_id: cand.step.item_id,
                score: cand.step.score,
                avg_time_mins: cand.step.avg_time_mins,
                counter_trait: cand.step.counter_trait,
            });
        } else {
            clustered.push(cand);
        }
    }

    // Select primary steps (always keeping owned items, capping at MAX_PRIMARY_BUILD_STEPS
    // with per-phase budgets Early: 2, Core: 2, Late: 4 while blocking low-share filler).
    let mut is_primary = vec![false; clustered.len()];
    let mut primary_count = 0_usize;
    let mut phase_counts = [0_usize; 5];

    for (idx, cand) in clustered.iter().enumerate() {
        if cand.step.owned {
            is_primary[idx] = true;
            primary_count += 1;
            phase_counts[phase_slot(cand.step.phase)] += 1;
        }
    }
    for (idx, cand) in clustered.iter().enumerate() {
        if primary_count >= MAX_PRIMARY_BUILD_STEPS {
            break;
        }
        if is_primary[idx] {
            continue;
        }
        let slot = phase_slot(cand.step.phase);
        let cap = phase_primary_cap(cand.step.phase);
        if phase_counts[slot] >= cap {
            continue;
        }
        if phase_counts[slot] >= 1
            && !cand.is_protected_upgrade
            && is_phase_filler(cand.step.phase, cand.match_count, phase_max_mc[slot])
        {
            continue;
        }
        is_primary[idx] = true;
        primary_count += 1;
        phase_counts[slot] += 1;
    }

    let mut steps: Vec<BuildStep> = Vec::with_capacity(primary_count);
    let mut remainder: Vec<ScoredCandidate> =
        Vec::with_capacity(clustered.len().saturating_sub(primary_count));
    for (idx, cand) in clustered.into_iter().enumerate() {
        if is_primary[idx] {
            steps.push(cand.step);
        } else {
            remainder.push(cand);
        }
    }

    // Attach remaining unchosen candidates as variants, prioritizing direct upgrade-chain parents
    // (e.g. Skull Basher -> Abyssal Blade) and closest-score steps within phase epsilon.
    for cand in remainder {
        let eps = variant_epsilon(cand.step.phase);
        let cand_components = repo
            .item_catalog(cand.step.item_id)
            .map(|e| e.components)
            .unwrap_or_default();

        let direct_parent_idx = steps.iter().enumerate().find_map(|(i, step)| {
            (step.phase == cand.step.phase
                && cand_components.contains(&step.item_id)
                && (step.score - cand.step.score).abs() <= eps + 0.06)
                .then_some(i)
        });

        let target_idx = direct_parent_idx.or_else(|| {
            let best_with_room = steps
                .iter()
                .enumerate()
                .filter(|(_, step)| {
                    step.phase == cand.step.phase
                        && step.variants.len() < MAX_VARIANTS_PER_STEP
                        && (step.score - cand.step.score).abs() <= eps
                })
                .min_by(|(_, a), (_, b)| {
                    (a.score - cand.step.score)
                        .abs()
                        .total_cmp(&(b.score - cand.step.score).abs())
                })
                .map(|(i, _)| i);
            best_with_room.or_else(|| {
                steps
                    .iter()
                    .enumerate()
                    .filter(|(_, step)| {
                        step.phase == cand.step.phase && (step.score - cand.step.score).abs() <= eps
                    })
                    .min_by(|(_, a), (_, b)| {
                        (a.score - cand.step.score)
                            .abs()
                            .total_cmp(&(b.score - cand.step.score).abs())
                    })
                    .map(|(i, _)| i)
            })
        });

        if let Some(i) = target_idx {
            steps[i].variants.push(BuildVariant {
                item_id: cand.step.item_id,
                score: cand.step.score,
                avg_time_mins: cand.step.avg_time_mins,
                counter_trait: cand.step.counter_trait,
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
const fn is_early_boots(item_id: ItemId) -> bool {
    matches!(item_id.0, 50 | 63 | 180 | 214)
}

#[inline]
const fn is_late_boots(item_id: ItemId) -> bool {
    matches!(item_id.0, 48 | 220 | 231 | 931)
}

#[inline]
const fn is_any_boots(item_id: ItemId) -> bool {
    is_early_boots(item_id) || is_late_boots(item_id)
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

#[inline]
const fn phase_primary_cap(phase: ItemPhase) -> usize {
    match phase {
        ItemPhase::Late => 4,
        _ => 2,
    }
}

#[inline]
const fn variant_epsilon(phase: ItemPhase) -> f32 {
    match phase {
        ItemPhase::Late => LATE_VARIANT_SCORE_EPSILON,
        _ => ITEM_VARIANT_SCORE_EPSILON,
    }
}

#[inline]
fn is_phase_filler(phase: ItemPhase, match_count: u32, phase_max_mc: u32) -> bool {
    if phase_max_mc < 50 {
        return false;
    }
    let min_share = match phase {
        ItemPhase::Late => LATE_LEADER_MIN_SHARE,
        _ => EARLY_CORE_LEADER_MIN_SHARE,
    };
    (match_count as f32) < (phase_max_mc as f32) * min_share
}

#[inline]
const fn is_early_only_aura_component(item_id: ItemId) -> bool {
    matches!(item_id.0, 86 | 88 | 94)
}

#[inline]
const fn is_excluded_basic_subcomponent(item_id: ItemId) -> bool {
    matches!(
        item_id.0,
        2..=10
            | 12..=23
            | 25..=29
            | 31
            | 32
            | 34
            | 56
            | 57
            | 59..=61
            | 67
            | 69
            | 240
            | 1122
            | 1125
            | 1575
            | 1802
            | 1847..=1849
            | 1872
    )
}

#[inline]
const fn is_valuable_stepping_stone_id(item_id: ItemId) -> bool {
    matches!(
        item_id.0,
        24 | 51 | 52 | 53 | 54 | 55 | 58 | 79 | 129 | 149 | 162 | 170 | 185 | 236 | 259
    )
}
