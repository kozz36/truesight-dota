use crate::domain::{
    Bracket, BuildStep, HeroId, HeroItemStat, ItemId, ItemPhase, Role, TEAM_SIZE,
    all_referenced_counter_item_ids, is_counter_item_eligible_for_role,
};
use crate::item_math::{
    MAX_PRIMARY_BUILD_STEPS, ScoredCandidate, is_phase_filler, phase_primary_cap,
    score_timed_candidate,
};
use crate::item_ownership::{
    is_any_boots, is_early_only_aura_component, is_excluded_basic_subcomponent,
};
use crate::ports::ItemMetaRepository;

#[allow(clippy::too_many_arguments)]
pub(crate) fn backfill_sparse_late_steps(
    repo: &dyn ItemMetaRepository,
    hero_id: HeroId,
    role: Role,
    bracket: Bracket,
    enemies: &[Option<HeroId>; TEAM_SIZE],
    owned_items: &[ItemId],
    starting_items: &[ItemId],
    role_max_mc: u32,
    steps: &mut Vec<BuildStep>,
    remainder: &mut Vec<ScoredCandidate>,
    deferred_aghs_blessing: &mut Option<ScoredCandidate>,
) {
    let surviving_non_boot_late = |cur_steps: &[BuildStep]| {
        cur_steps
            .iter()
            .filter(|s| s.phase == ItemPhase::Late && !is_any_boots(s.item_id) && s.item_id.0 != 29)
            .count()
    };
    if surviving_non_boot_late(steps) >= 2 {
        return;
    }

    let mut prepared_tiers: Vec<(u32, Vec<HeroItemStat>)> = repo
        .hero_item_late_fallback_tiers(hero_id, role, bracket)
        .into_iter()
        .map(|tier| {
            prepare_tier_candidates(
                repo,
                role,
                owned_items,
                starting_items,
                role_max_mc,
                steps,
                remainder,
                tier,
            )
        })
        .collect();

    // Check across already-selected primary steps and every prepared fallback tier so an unowned
    // Aghanim's Blessing in an earlier tier defers cleanly when Scepter appears in a later tier.
    let has_scepter = steps.iter().any(|s| s.item_id.0 == 108)
        || prepared_tiers
            .iter()
            .any(|(_, cands)| cands.iter().any(|s| s.item_id.0 == 108));

    // Extract unowned Aghanim's Blessing (271) globally before quota filling so higher-priority
    // Blessing rows defer under Scepter rather than consuming Late quota.
    if has_scepter && !owned_items.contains(&ItemId(271)) {
        for (_, cands) in &mut prepared_tiers {
            if let Some(pos) = cands.iter().position(|s| s.item_id.0 == 271) {
                let blessing = cands.remove(pos);
                if deferred_aghs_blessing.is_none() {
                    *deferred_aghs_blessing = Some(score_timed_candidate(
                        repo,
                        blessing,
                        enemies,
                        owned_items,
                        false,
                    ));
                }
            }
        }
    }

    // Collect distinct directly-owned fallback items across all prepared tiers (highest priority first)
    // so owned upgrade parents in later tiers stay reserved and are always promoted into steps.
    let mut owned_fallback_stats: Vec<HeroItemStat> = Vec::new();
    for (_, cands) in &prepared_tiers {
        for &stat in cands {
            if owned_items.contains(&stat.item_id)
                && !steps.iter().any(|s| s.item_id == stat.item_id)
                && !owned_fallback_stats
                    .iter()
                    .any(|o| o.item_id == stat.item_id)
            {
                owned_fallback_stats.push(stat);
            }
        }
    }

    for (raw_tier_max_mc, mut deduped) in prepared_tiers {
        if surviving_non_boot_late(steps) >= 2 {
            break;
        }
        deduped.retain(|s| {
            !steps.iter().any(|step| {
                step.item_id == s.item_id || step.variants.iter().any(|v| v.item_id == s.item_id)
            }) && !remainder.iter().any(|r| r.step.item_id == s.item_id)
        });

        deduped.retain(|s| {
            owned_items.contains(&s.item_id)
                || !is_phase_filler(ItemPhase::Late, s.match_count, raw_tier_max_mc)
        });
        if deduped.is_empty() {
            continue;
        }

        let reserved_owned = owned_fallback_stats
            .iter()
            .filter(|o| {
                !steps.iter().any(|s| s.item_id == o.item_id)
                    && !deduped.iter().any(|d| d.item_id == o.item_id)
            })
            .count();

        let mut scored_tier: Vec<ScoredCandidate> = deduped
            .into_iter()
            .map(|stat| score_timed_candidate(repo, stat, enemies, owned_items, false))
            .collect();
        scored_tier.sort_by(|a, b| {
            b.directly_owned
                .cmp(&a.directly_owned)
                .then_with(|| b.step.score.total_cmp(&a.step.score))
                .then_with(|| a.step.avg_time_mins.total_cmp(&b.step.avg_time_mins))
                .then_with(|| a.step.item_id.cmp(&b.step.item_id))
        });

        let mut late_count = steps.iter().filter(|s| s.phase == ItemPhase::Late).count();
        for cand in scored_tier {
            let can_promote = if cand.directly_owned {
                steps.len() < MAX_PRIMARY_BUILD_STEPS
            } else {
                steps.len() + reserved_owned < MAX_PRIMARY_BUILD_STEPS
                    && late_count + reserved_owned < phase_primary_cap(ItemPhase::Late)
            };
            if can_promote {
                steps.push(cand.step);
                late_count += 1;
            } else {
                remainder.push(cand);
            }
        }
    }

    // Guarantee any directly-owned fallback item sitting in an unvisited later tier is promoted
    // into primary steps so upgrade variants (such as Blessing under owned Scepter) attach cleanly.
    for stat in owned_fallback_stats {
        if !steps.iter().any(|s| s.item_id == stat.item_id) && steps.len() < MAX_PRIMARY_BUILD_STEPS
        {
            steps.push(score_timed_candidate(repo, stat, enemies, owned_items, false).step);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_tier_candidates(
    repo: &dyn ItemMetaRepository,
    role: Role,
    owned_items: &[ItemId],
    starting_items: &[ItemId],
    role_max_mc: u32,
    steps: &[BuildStep],
    remainder: &[ScoredCandidate],
    tier: Vec<HeroItemStat>,
) -> (u32, Vec<HeroItemStat>) {
    let mut raw_tier_max_mc = 0_u32;
    let mut deduped: Vec<HeroItemStat> = Vec::with_capacity(tier.len());
    for stat in tier {
        if stat.phase != ItemPhase::Late || is_any_boots(stat.item_id) || stat.item_id.0 == 29 {
            continue;
        }
        if matches!(stat.item_id.0, 30 | 40 | 43) {
            continue;
        }
        if all_referenced_counter_item_ids().contains(&stat.item_id)
            && !is_counter_item_eligible_for_role(stat.item_id, role)
        {
            continue;
        }
        let owned = owned_items.contains(&stat.item_id);
        if !owned
            && (is_excluded_basic_subcomponent(stat.item_id)
                || is_early_only_aura_component(stat.item_id))
        {
            continue;
        }
        if let Some(entry) = repo.item_catalog(stat.item_id)
            && ((entry.cost == 0 && entry.is_consumable_drop) || entry.is_recipe)
        {
            continue;
        }
        // Track pre-deduplication tier sample peak before dropping already-present duplicates
        // so removing e.g. BKB does not collapse the noise floor for remaining rows.
        raw_tier_max_mc = raw_tier_max_mc.max(stat.match_count);
        let already_present = starting_items.contains(&stat.item_id)
            || steps.iter().any(|s| {
                s.item_id == stat.item_id || s.variants.iter().any(|v| v.item_id == stat.item_id)
            })
            || remainder.iter().any(|r| r.step.item_id == stat.item_id);
        if already_present {
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

    let sample_ref_mc = raw_tier_max_mc.max(role_max_mc);
    if sample_ref_mc >= 50 {
        let min_mc = ((sample_ref_mc as f32 * 0.025).ceil() as u32).clamp(8, 25);
        deduped.retain(|s| owned_items.contains(&s.item_id) || s.match_count >= min_mc);
    }
    (raw_tier_max_mc, deduped)
}
