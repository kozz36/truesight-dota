use crate::AppSession;
use truesight_core::domain::{
    AttackType, DraftPick, HeroId, RecommendedBuild, Role, Side, TEAM_SIZE,
};
use truesight_core::math::{DraftAnalysis5v5, MatchupMatrix, recommend_heroes};

// =============================================================================
// Pure Telemetry & Diagnostic View-Models
// =============================================================================

#[derive(Debug, Clone, Copy)]
pub struct SlotBreakdown {
    pub meta: f32,
    pub cnt: f32,
    pub syn: f32,
    pub net: f32,
}

pub fn compute_slot_breakdown(
    session: &AppSession,
    analysis: &DraftAnalysis5v5,
    side: Side,
    slot: usize,
) -> Option<SlotBreakdown> {
    if slot >= TEAM_SIZE {
        return None;
    }
    let default_role = Role::ALL.get(slot).copied().unwrap_or(Role::Carry);
    match side {
        Side::Allies => {
            let pick = session.draft.allies[slot]?;
            let role = pick.role.unwrap_or(default_role);
            let base_wr = session
                .matrix
                .hero(pick.hero_id)
                .map(|h| h.meta_winrate(role))
                .unwrap_or(0.50);
            let meta = base_wr - 0.50;
            let cnt = analysis.ally_counter_avg[slot].unwrap_or(0.0);
            let syn = analysis.ally_synergy_avg[slot].unwrap_or(0.0);
            let net = analysis.ally_slot_net_delta[slot].unwrap_or(meta + cnt + syn);
            Some(SlotBreakdown {
                meta,
                cnt,
                syn,
                net,
            })
        }
        Side::Enemies => {
            let pick = session.draft.enemies[slot]?;
            let role = pick.role.unwrap_or(default_role);
            let base_wr = session
                .matrix
                .hero(pick.hero_id)
                .map(|h| h.meta_winrate(role))
                .unwrap_or(0.50);
            let meta = base_wr - 0.50;
            // Enemy perspective counter average is the negation of the column mean in counter_grid
            let cnt = enemy_counter_col_avg(analysis, slot)
                .map(|col_avg| -col_avg)
                .unwrap_or(0.0);
            let mut syn_sum = 0.0_f32;
            let mut syn_cnt = 0_u32;
            for (l, other_opt) in session.draft.enemies.iter().enumerate() {
                if l == slot {
                    continue;
                }
                if let Some(other) = other_opt {
                    syn_sum += session
                        .matrix
                        .cell(pick.hero_id, other.hero_id)
                        .synergy_delta;
                    syn_cnt += 1;
                }
            }
            let syn = if syn_cnt > 0 {
                syn_sum / syn_cnt as f32
            } else {
                0.0
            };
            let net = analysis.enemy_slot_net_delta[slot].unwrap_or(meta + cnt + syn);
            Some(SlotBreakdown {
                meta,
                cnt,
                syn,
                net,
            })
        }
    }
}

/// Arithmetic mean of column `enemy_slot` in `analysis.counter_grid` (from Ally perspective).
pub fn enemy_counter_col_avg(analysis: &DraftAnalysis5v5, enemy_slot: usize) -> Option<f32> {
    if enemy_slot >= TEAM_SIZE {
        return None;
    }
    let mut sum = 0.0_f32;
    let mut cnt = 0_u32;
    for row in &analysis.counter_grid {
        if let Some(d) = row[enemy_slot] {
            sum += d;
            cnt += 1;
        }
    }
    if cnt > 0 {
        Some(sum / cnt as f32)
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct CompDiagnostic {
    pub picked: u32,
    pub melee: Vec<HeroId>,
    pub ranged: Vec<HeroId>,
    pub disables: Vec<HeroId>,
    pub initiators: Vec<HeroId>,
    pub rule_active: [bool; 6],
    pub score: f32,
}

pub const COMP_RULES: [(&str, f32); 6] = [
    ("5 melee", -0.018),
    ("4 melee (>= 4 picked)", -0.007),
    ("5 ranged", -0.008),
    ("No disable (>= 4 picked)", -0.015),
    ("Only 1 disable (5 picked)", -0.005),
    ("Balanced: 1-3 melee, >= 2 dis, >= 1 init", 0.005),
];

pub fn compute_comp_diagnostic(
    picks: &[Option<DraftPick>; TEAM_SIZE],
    matrix: &MatchupMatrix,
) -> CompDiagnostic {
    let mut picked = 0_u32;
    let mut melee = Vec::with_capacity(TEAM_SIZE);
    let mut ranged = Vec::with_capacity(TEAM_SIZE);
    let mut disables = Vec::with_capacity(TEAM_SIZE);
    let mut initiators = Vec::with_capacity(TEAM_SIZE);

    for pick in picks.iter().flatten() {
        if let Some(h) = matrix.hero(pick.hero_id) {
            picked += 1;
            match h.attack_type {
                AttackType::Melee => melee.push(h.id),
                AttackType::Ranged => ranged.push(h.id),
            }
            if h.has_disable {
                disables.push(h.id);
            }
            if h.has_initiation {
                initiators.push(h.id);
            }
        }
    }

    let mut rule_active = [false; 6];
    let mut score = 0.0_f32;

    if picked >= 3 {
        let m = melee.len() as u32;
        let r = ranged.len() as u32;
        let d = disables.len() as u32;
        let ini = initiators.len() as u32;

        if m == 5 {
            rule_active[0] = true;
            score -= 0.018;
        } else if m == 4 && picked >= 4 {
            rule_active[1] = true;
            score -= 0.007;
        } else if r == 5 {
            rule_active[2] = true;
            score -= 0.008;
        }

        if picked >= 4 && d == 0 {
            rule_active[3] = true;
            score -= 0.015;
        } else if picked == 5 && d == 1 {
            rule_active[4] = true;
            score -= 0.005;
        }

        if picked >= 4 && (1..=3).contains(&m) && d >= 2 && ini >= 1 {
            rule_active[5] = true;
            score += 0.005;
        }
    }

    CompDiagnostic {
        picked,
        melee,
        ranged,
        disables,
        initiators,
        rule_active,
        score,
    }
}

/// Compact summary view-model shared by the always-on-top overlay in draft and post-draft phases.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlaySummary {
    pub is_post_draft: bool,
    pub projected_ally_win_pct: f32,
    pub target_role: Role,
    pub top_suggestions: Vec<(HeroId, String, f32)>,
    pub counter_grid: [[Option<f32>; TEAM_SIZE]; TEAM_SIZE],
    pub build: Option<RecommendedBuild>,
    pub game_time_secs: i32,
}

/// Computes the compact overlay summary from `AppSession`, guaranteeing exact `0.00%` win% parity with the main window.
pub fn compute_overlay_summary(session: &AppSession) -> OverlaySummary {
    let (analysis, _) = session.evaluate();
    let recs = recommend_heroes(&session.draft, &session.matrix, &session.weights, 3);
    let top_suggestions = recs
        .into_iter()
        .take(3)
        .map(|r| {
            let name = session
                .matrix
                .hero(r.hero_id)
                .map(|h| h.display_name.clone())
                .unwrap_or_else(|| format!("Hero #{}", r.hero_id.0));
            (r.hero_id, name, r.projected_win_probability * 100.0)
        })
        .collect();
    let is_post_draft = session.is_post_draft_build_view();
    let build = if is_post_draft {
        session.compute_recommended_build()
    } else {
        None
    };

    OverlaySummary {
        is_post_draft,
        projected_ally_win_pct: analysis.ally_win_probability * 100.0,
        target_role: session.draft.target_role,
        top_suggestions,
        counter_grid: analysis.counter_grid,
        build,
        game_time_secs: session.game_time_secs,
    }
}
