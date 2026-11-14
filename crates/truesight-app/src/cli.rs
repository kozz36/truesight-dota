use crate::session::AppSession;
use truesight_adapters::sync::normalize_steam32_id;
use truesight_core::domain::{Bracket, Faction, Role, Side, TEAM_SIZE};

pub(crate) const ANSI_RESET: &str = "\x1b[0m";
pub(crate) const ANSI_BOLD: &str = "\x1b[1m";
pub(crate) const ANSI_DIM: &str = "\x1b[2m";
pub(crate) const ANSI_GREEN: &str = "\x1b[38;2;46;204;113m";
pub(crate) const ANSI_RED: &str = "\x1b[38;2;231;76;60m";
pub(crate) const ANSI_CYAN: &str = "\x1b[38;2;52;152;219m";
pub(crate) const ANSI_GOLD: &str = "\x1b[38;2;241;196;15m";

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

pub fn parse_account_id_arg(args: &[String]) -> Result<Option<Option<u64>>, String> {
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
