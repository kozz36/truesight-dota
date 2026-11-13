# Engineering Task Plan & Statistical Roadmap: `truesight-dota`

## 1. Executive Summary

This document tracks the formal engineering work units (`T1`–`T16`), statistical invariants, TDD verification gates, and data synchronization schedule for `truesight-dota`.

## 2. Work-Unit Execution Matrix (`T1`–`T16`)

| Task ID | Scope & Deliverable | Verification Gate | Status |
|---------|---------------------|-------------------|--------|
| **T1** | Cargo workspace & `truesight-core` (`HeroId`, `Role`, `Bracket`, `DraftState`, Logit/Bayesian 5v5 math engine, ports). | `cargo test -p truesight-core` | Completed (`c05e077`) |
| **T2** | `truesight-adapters` (`SeededMetaRepository`, bounded `BoundedGsiListener` with 64 KiB cap & `sync_channel(16)`). | `cargo test -p truesight-adapters` | Completed (`7991cbd`) |
| **T3** | `truesight-app` interactive TrueColor 5v5 CLI/TUI, initial `README.md` & `docs/architecture.md`. | `cargo test -p truesight-app` | Completed (`932c2f5`) |
| **T4** | Full 127-hero OpenDota catalog (`opendota_hero_stats.json`) + player `127255741` comfort history (`player_127255741_heroes.json`). | 127-hero count & comfort tests | Completed (`878603f`) |
| **T5** | Native `eframe`/`egui` desktop GUI sharing `AppSession` with CLI/TUI via additive `gui` Cargo feature. | `--all-features` & `--no-default-features` | Completed (`cfd19ec`) |
| **T6** | Pin `rust-toolchain.toml` to `1.99.1`, install `~/.local/bin/truesight-dota`, `.desktop` launcher, and Dota 2 GSI `.cfg`. | Binary & GSI smoke check | Completed (`2799255`) |
| **T7** | Full $127 \times 127$ (`15,966` empirical pairs) OpenDota matchup & synergy matrix, independent 10-slot pickers, bottom `Suggested Heroes` section. | Pair coverage & slot baseline tests | Completed (`c4e7853`) |
| **T8** | Cohort-consistent baseline winrate calibration inside `/heroes/{id}/matchups` and $\sqrt{N}$ variance scaling across correlated 5v5 pairs. | Win probability calibration check | Completed (`4407718`) |
| **T9** | Strict counter antisymmetry $\Delta C(A, B) = -\Delta C(B, A)$, double-centered zero-mean synergy $\Delta S(A, B) = \Delta S(B, A)$, separated `Avg Counter`/`Avg Synergy` columns, modern Pos 1–5 role taxonomy, and exact `Projected Win%` identity. | Team-swap symmetry $P(A)+P(B)=100\%$ | Completed (`66bd697`) |
| **T10** | Split `Bracket` into all 8 individual Dota 2 medals (`Herald`..`Immortal`), dynamic per-medal counter/synergy scaling ($\tau_b, d_{i,b}$), and auto-focused click-to-type slot picker. | Red-Green TDD 8-medal & continuity tests | Completed (`5d9c533`) |
| **T11** | Statistical Engine v2: 7-day patch momentum (`pub_pick_trend`/`pub_win_trend`), exponential comfort recency decay (`last_played`), structural team composition balance (`AttackType::Melee/Ranged`, `Disabler`, `Initiator`), and explicit `A:/E:` synergy breakdown. | Red-Green TDD v2 engine test | Completed (`155aaf3`) |
| **T12** | Local Database Cache Manager (`~/.local/share/truesight-dota/cache/`) with **24h Meta TTL** and **1h Player TTL**, background startup auto-refresh, GUI `[Sync DB]` button + freshness badge, CLI `--sync`/`sync`, and ASCII `[v]` slot glyph fix. | Red-Green TDD cache TTL & override test | Completed (`f369e45`) |
| **T13** | Formal update of `docs/roadmap-and-tasks.md`, `docs/architecture.md`, and `README.md`. | Structural readback & doc consistency | Completed (`fffe119`) |
| **T14** | Personal Comfort Engine v3: per-match exponential recency decay ($\tau = 75\text{d}$, floor `0.0`) over `/api/players/127255741/matches` (`player_matches.json`) + active practice volume mastery bonus $\ln(1 + G_{\text{eff}})$ + target-role comfort edge in `evaluate_draft_5v5`. | Red-Green TDD Comfort v3 core & adapter tests | Completed (`4f8ffcc`) |
| **T15** | Update `docs/architecture.md`, `docs/roadmap-and-tasks.md`, `README.md`, and `odd/tasks/truesight-dota-foundation.md` for Comfort Engine v3. | Structural readback & doc consistency | Completed (`d3bac92`) |
| **T16** | Create `AGENTS.md` (implementation & TDD testing policy), product domain skill `skills/truesight-dota-domain/SKILL.md`, bundled Rust architecture skill `skills/rust-architect/SKILL.md` + `.atl/skill-registry.md`, and root `LICENSE` (`Apache-2.0`). | Skill style guide & workspace test check | Completed (`c626364`) |

## 3. Data Sources & Synchronization Cadence

| Endpoint | Source Cadence | Local Cache TTL | Cache File (`~/.local/share/truesight-dota/cache/`) |
|----------|----------------|-----------------|-----------------------------------------------------|
| `https://api.opendota.com/api/heroStats` | Rolling daily aggregation (updates once every 24h UTC) | **24 hours (`86,400 s`)** | `opendota_hero_stats.json` |
| `https://api.opendota.com/api/players/127255741/heroes` | 5–15 minutes after each public match completes | **1 hour (`3,600 s`)** | `player_heroes.json` |
| `https://api.opendota.com/api/players/127255741/matches?limit=500` | 5–15 minutes after each public match completes | **1 hour (`3,600 s`)** | `player_matches.json` |
| `https://api.opendota.com/api/heroes/{id}/matchups` | Rolling patch window (`< 0.05%` daily drift outside major patch day) | Embedded calibrated $127 \times 127$ table (`16,002` pairs) | Compiled into `snapshot_pairs.rs` |

### Synchronization Modes
1. **Non-Blocking Startup Auto-Sync**: On launch, `truesight-dota` loads the local cache (or embedded snapshot) in `< 2 ms` and spawns a background thread if either TTL (`24h` Meta / `1h` Player) has expired.
2. **GUI One-Click Refresh**: Clicking **`[Sync DB]`** in the top-right header forces an immediate fetch of `heroStats` and player comfort history and reloads the active `MatchupMatrix`.
3. **Headless CLI Sync**: Running `truesight-dota --sync` (or typing `sync` inside `truesight-dota --cli`) refreshes the cache from terminal or cron/systemd timers.

## 4. Phase 2 Roadmap (Next Milestones)

1. **STRATZ GraphQL Empirical `with` Synergy Adapter**: Optionally query `heroVsHeroMatchup.advantage.with` when a `STRATZ_API_TOKEN` is configured in the environment.
2. **PipeWire / X11 Top-Bar Vision Adapter (`VisionCaptureAdapter`)**: Automatically detect all 10 picked heroes during Ranked All Pick by hashing the top-bar hero portraits at 1 Hz whenever GSI reports `DOTA_GAMERULES_STATE_HERO_SELECTION`.
