use crate::domain::{BuildStep, ItemId, ItemPhase};
use crate::ports::ItemMetaRepository;

/// Returns `true` when `target` is directly in `owned_items`, superseded by a higher-tier boots
/// item, or consumed as a direct or transitive recipe component of an owned upgraded item.
pub fn is_item_owned_or_upgraded(
    repo: &dyn ItemMetaRepository,
    target: ItemId,
    owned_items: &[ItemId],
) -> bool {
    if owned_items.contains(&target) {
        return true;
    }
    if is_early_boots(target) && owned_items.iter().any(|&id| is_late_boots(id)) {
        return true;
    }
    owned_items
        .iter()
        .any(|&owned_id| item_contains_component(repo, owned_id, target, 0))
}

fn item_contains_component(
    repo: &dyn ItemMetaRepository,
    parent: ItemId,
    target: ItemId,
    depth: usize,
) -> bool {
    // Bound recursion cleanly against malformed or cyclic custom item catalogs.
    if depth >= 6 {
        return false;
    }
    let Some(entry) = repo.item_catalog(parent) else {
        return false;
    };
    for &comp in &entry.components {
        if comp == parent {
            continue;
        }
        if comp == target || item_contains_component(repo, comp, target, depth + 1) {
            return true;
        }
    }
    false
}

/// Computes the active `NEXT` primary step index using inventory progression and timing so
/// in-progress or out-of-order purchases stay eligible while obsolete Early/Core steps are
/// skipped in late game or fast-paced matches.
pub fn compute_next_unowned_index(
    steps: &[BuildStep],
    owned_items: &[ItemId],
    game_time_secs: i32,
) -> Option<usize> {
    let game_mins = (game_time_secs.max(0) as f32) / 60.0;
    let owned_core_or_late = steps
        .iter()
        .filter(|s| s.owned && matches!(s.phase, ItemPhase::Core | ItemPhase::Late))
        .count();
    let owned_late = steps
        .iter()
        .filter(|s| s.owned && s.phase == ItemPhase::Late)
        .count();
    let has_unowned_late = steps.iter().any(|s| !s.owned && s.phase == ItemPhase::Late);
    let has_unowned_core_or_late = steps
        .iter()
        .any(|s| !s.owned && matches!(s.phase, ItemPhase::Core | ItemPhase::Late));
    let has_brown_boots = owned_items.contains(&ItemId(29));

    let is_obsolete_for_next = |step: &BuildStep| -> bool {
        let only_generic_brown_boots =
            is_early_boots(step.item_id) && step.owned_components == 1 && has_brown_boots;
        let has_specific_component = step.owned_components > 0 && !only_generic_brown_boots;
        if has_specific_component {
            return false;
        }
        match step.phase {
            ItemPhase::Early => {
                let progressed = owned_core_or_late >= 2
                    || (owned_core_or_late >= 1 && game_mins >= step.avg_time_mins + 15.0);
                let late_complete = !has_unowned_core_or_late && owned_late >= 1;
                progressed && (has_unowned_core_or_late || late_complete)
            }
            ItemPhase::Core => {
                has_unowned_late
                    && (owned_late >= 2
                        || (owned_late >= 1 && game_mins >= step.avg_time_mins + 18.0))
            }
            _ => false,
        }
    };

    steps
        .iter()
        .position(|s| !s.owned && !is_obsolete_for_next(s))
}

pub(crate) fn count_owned_components(
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

#[inline]
pub(crate) const fn is_early_boots(item_id: ItemId) -> bool {
    matches!(item_id.0, 50 | 63 | 180 | 214)
}

#[inline]
pub(crate) const fn is_late_boots(item_id: ItemId) -> bool {
    matches!(item_id.0, 48 | 220 | 231 | 931)
}

#[inline]
pub(crate) const fn is_any_boots(item_id: ItemId) -> bool {
    is_early_boots(item_id) || is_late_boots(item_id)
}

#[inline]
pub(crate) const fn phase_slot(phase: ItemPhase) -> usize {
    match phase {
        ItemPhase::Starting => 0,
        ItemPhase::Early => 1,
        ItemPhase::Core => 2,
        ItemPhase::Late => 3,
        ItemPhase::Situational => 4,
    }
}

#[inline]
pub(crate) const fn is_early_only_aura_component(item_id: ItemId) -> bool {
    matches!(item_id.0, 86 | 88 | 94)
}

#[inline]
pub(crate) const fn is_excluded_basic_subcomponent(item_id: ItemId) -> bool {
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
pub(crate) const fn is_valuable_stepping_stone_id(item_id: ItemId) -> bool {
    matches!(
        item_id.0,
        24 | 51 | 52 | 53 | 54 | 55 | 58 | 79 | 129 | 149 | 162 | 170 | 185 | 236 | 259
    )
}
