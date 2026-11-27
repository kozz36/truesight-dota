# Engineering Task Plan & Statistical Roadmap: `truesight-dota`

## 1. Executive Summary

This document tracks the formal engineering work units (`T1`–`T16`), statistical invariants, TDD verification gates, and data synchronization schedule for `truesight-dota`.

## 2. Work-Unit Execution Matrix (`T1`–`T16`)

| Task ID | Scope & Deliverable | Verification Gate | Status |
|---------|---------------------|-------------------|--------|
| **T1** | Cargo workspace & `truesight-core` (`HeroId`, `Role`, `Bracket`, `DraftState`, Logit/Bayesian 5v5 math engine, ports). | `cargo test -p truesight-core` | Completed (`c05e077`) |
| **T2** | `truesight-adapters` (`SeededMetaRepository`, bounded `BoundedGsiListener` with 64 KiB cap & `sync_channel(16)`). | `cargo test -p truesight-adapters` | Completed (`7991cbd`) |
| **T3** | `truesight-app` interactive TrueColor 5v5 CLI/TUI, initial `README.md` & `docs/architecture.md`. | `cargo test -p truesight-app` | Completed (`932c2f5`) |
| **T4** | Full 127-hero OpenDota catalog (`opendota_hero_stats.json`) + per-player comfort history (`players/{account_id}/player_heroes.json`). | 127-hero count & comfort tests | Completed (`878603f`) |
| **T5** | Native `eframe`/`egui` desktop GUI sharing `AppSession` with CLI/TUI via additive `gui` Cargo feature. | `--all-features` & `--no-default-features` | Completed (`cfd19ec`) |
| **T6** | Pin `rust-toolchain.toml` to `1.99.1`, install `~/.local/bin/truesight-dota`, `.desktop` launcher, and Dota 2 GSI `.cfg`. | Binary & GSI smoke check | Completed (`2799255`) |
| **T7** | Full $127 \times 127$ (`15,966` empirical pairs) OpenDota matchup & synergy matrix, independent 10-slot pickers, bottom `Suggested Heroes` section. | Pair coverage & slot baseline tests | Completed (`c4e7853`) |
| **T8** | Cohort-consistent baseline winrate calibration inside `/heroes/{id}/matchups` and $\sqrt{N}$ variance scaling across correlated 5v5 pairs. | Win probability calibration check | Completed (`4407718`) |
| **T9** | Strict counter antisymmetry $\Delta C(A, B) = -\Delta C(B, A)$, double-centered zero-mean synergy $\Delta S(A, B) = \Delta S(B, A)$, separated `Avg Counter`/`Avg Synergy` columns, modern Pos 1–5 role taxonomy, and exact `Projected Win%` identity. | Team-swap symmetry $P(A)+P(B)=100\%$ | Completed (`66bd697`) |
| **T10** | Split `Bracket` into all 8 individual Dota 2 medals (`Herald`..`Immortal`), dynamic per-medal counter/synergy scaling ($\tau_b, d_{i,b}$), and auto-focused click-to-type slot picker. | Red-Green TDD 8-medal & continuity tests | Completed (`5d9c533`) |
| **T11** | Statistical Engine v2: 7-day patch momentum (`pub_pick_trend`/`pub_win_trend`), exponential comfort recency decay (`last_played`), structural team composition balance (`AttackType::Melee/Ranged`, `Disabler`, `Initiator`), and explicit `A:/E:` synergy breakdown. | Red-Green TDD v2 engine test | Completed (`155aaf3`) |
| **T12** | Local Database Cache Manager (`~/.local/share/truesight-dota/cache/`) with **24h Meta TTL** and **1h Player TTL**, background startup auto-refresh, GUI `[Sync DB]` button + freshness badge, CLI `--sync`/`sync`, and ASCII `[v]` slot glyph fix. | Red-Green TDD cache TTL & override test | Completed (`f369e45`) |
| **T13** | Formal update of `docs/roadmap-and-tasks.md`, `docs/architecture.md`, and `README.md`. | Structural readback & doc consistency | Completed (`fffe119`) |
| **T14** | Personal Comfort Engine v3: per-match exponential recency decay ($\tau = 75\text{d}$, floor `0.0`) over `/api/players/{account_id}/matches` (`player_matches.json`) + active practice volume mastery bonus $\ln(1 + G_{\text{eff}})$ + target-role comfort edge in `evaluate_draft_5v5`. | Red-Green TDD Comfort v3 core & adapter tests | Completed (`4f8ffcc`) |
| **T15** | Update `docs/architecture.md`, `docs/roadmap-and-tasks.md`, `README.md`, and `odd/tasks/truesight-dota-foundation.md` for Comfort Engine v3. | Structural readback & doc consistency | Completed (`d3bac92`) |
| **T16** | Create `AGENTS.md` (implementation & TDD testing policy), product domain skill `skills/truesight-dota-domain/SKILL.md`, bundled Rust architecture skill `skills/rust-architect/SKILL.md` + `.atl/skill-registry.md`, and root `LICENSE` (`Apache-2.0`). | Skill style guide & workspace test check | Completed (`c626364`) |
| **T16b** | v4 Draft Oracle Desktop GUI ("Gem of Truesight" HUD in `gui.rs`): embedded zero-dep binary RGB atlas (`crates/truesight-app/assets/hero_atlas.bin`, 860 KiB, `32x24` Gem icon + 127 `64x36` hero portraits), Allies/Oracle/Enemies slot breakdowns (`Meta · Cnt · Syn`), `Suggested Picks` with `Comfort` meter, 3-tab `COUNTER`/`SYNERGY`/`COMP` matrix, and Radiant/Dire toggle. | `embedded_hero_atlas_decodes_all_127_heroes_and_gem_icon`, `v4_slot_breakdowns_matrix_averages_and_comp_diagnostics_match_engine`, & `v4_layout_rects_fit_viewport_and_align_cards` | Completed (GUI v4) |

## 3. Data Sources & Synchronization Cadence

| Endpoint | Source Cadence | Local Cache TTL | Cache File (`~/.local/share/truesight-dota/cache/`) |
|----------|----------------|-----------------|-----------------------------------------------------|
| `https://api.opendota.com/api/heroStats` | Rolling daily aggregation (updates once every 24h UTC) | **24 hours (`86,400 s`)** | `opendota_hero_stats.json` |
| `https://api.opendota.com/api/players/{account_id}/heroes` | 5–15 minutes after each public match completes | **1 hour (`3,600 s`)** | `players/{account_id}/player_heroes.json` |
| `https://api.opendota.com/api/players/{account_id}/matches?limit=500` | 5–15 minutes after each public match completes | **1 hour (`3,600 s`)** | `player_matches.json` |
| `https://api.opendota.com/api/heroes/{id}/matchups` | Rolling patch window (`< 0.05%` daily drift outside major patch day) | Embedded calibrated $127 \times 127$ table (`16,002` pairs) | Compiled into `snapshot_pairs.rs` |
| `https://api.opendota.com/api/constants/items` + `/heroes/{id}/itemPopularity` (or STRATZ `itemStartingPurchase` / `itemFullPurchase`) | Patch / rolling purchase statistics | **24 hours (`86,400 s`)** | `item_constants.json`, `hero_item_stats.json` |

### Synchronization Modes
1. **Non-Blocking Startup Auto-Sync**: On launch, `truesight-dota` loads the local cache (or embedded snapshot) in `< 2 ms` and spawns a background thread if either TTL (`24h` Meta / `1h` Player) has expired.
2. **GUI One-Click Refresh**: Clicking **`[Sync DB]`** in the top-right header forces an immediate fetch of `heroStats` and player comfort history and reloads the active `MatchupMatrix`.
3. **Headless CLI Sync**: Running `truesight-dota --sync` (or typing `sync` inside `truesight-dota --cli`) refreshes the cache from terminal or cron/systemd timers.

## 4. Phase 2 Roadmap (`T17`–`T22` — Completed)

Phase 2 automates live faction mapping, top-bar portrait recognition, 3-tier Steam32 account resolution, open-source privacy readiness (`Apache-2.0`), GitHub Actions CI/Release pipelines, and security governance.

| Task ID | Scope & Deliverable | Verification Gate | Status |
|---------|---------------------|-------------------|--------|
| **T17** | **GSI Faction Event (`Radiant` / `Dire` Detection)**: Parse `"player": { "team_name": "radiant" \| "dire" }` in `BoundedGsiListener`, emit `DraftEvent::FactionDetected`, unify `Faction` in `truesight-core`, and preserve the strict `50-50` faction-neutral draft prior (`P(A,B) + P(B,A) = 1.0`). | `gsi_parser_extracts_radiant_and_dire_faction_and_preserves_50_50_neutrality` | Completed (`7083c69`) |
| **T18** | **Top-Bar Vision Adapter (`VisionCaptureAdapter`)**: Relocate shared `hero_atlas.bin` into `truesight-adapters::vision`, classify 10 top-bar hero portrait slots via RGB cell descriptors, and map Radiant (`0..4`) vs Dire (`5..9`) to `Side::Allies` / `Side::Enemies` using `Faction`. | `vision_capture_adapter_recognizes_topbar_10_slots_and_maps_radiant_dire_sides` | Completed (`e84b4a2`) |
| **T19** | **STRATZ Empirical `with` Synergy & 3-Tier Steam32 Resolution**: Optional `STRATZ_API_TOKEN` GraphQL `with` synergy matrix overlay + 3-tier Steam32 resolver (`1. Explicit Override`: GUI modal / `--account-id` / `config.toml` / `TRUESIGHT_STEAM32_ID` $\to$ `2. Dota 2 GSI player.steamid` $\to$ `3. Steam loginusers.vdf MostRecent`) isolating OpenDota caches in `~/.local/share/truesight-dota/cache/players/{account_id}/`. | `steam32_resolver_supports_explicit_override_gsi_steam64_and_loginusers_vdf` & `stratz_synergy_override_updates_matchup_matrix_while_preserving_symmetry` | Completed (`f315c48`) |
| **T20** | **Public Open-Source Readiness (`Apache-2.0`)**: Neutralize `OPENDOTA_HERO_SNAPSHOT` personal fields (`personal_matches = 0`, `personal_wins = 0`, `comfort_decay = 0.0`, default `+0.00%` comfort for new users), remove `DEFAULT_PLAYER_ACCOUNT_ID` & tracked `data/player_<account_id>_*.json` fixtures, and scrub personal ID references from repository sources and docs. | `default_seeded_repository_has_zero_personal_comfort_for_open_source_neutrality` + clean `git grep` audit | Completed (`f706bae`) |
| **T21** | **GitHub Actions CI & Automated Release Pipeline**: Add `.github/workflows/ci.yml` (dual-feature `--all-features --locked` and `--no-default-features --locked` with `-D warnings` on `ubuntu-latest`) and `.github/workflows/release.yml` (tagged `v*` release archive + `SHA256SUMS`). | YAML validation + `RUSTFLAGS="-D warnings"` dual-feature `--locked` run | Completed (`1537e10`) |
| **T22** | **Security, Governance, Documentation & Release Binary**: Add `SECURITY.md`, `CONTRIBUTING.md`, `.github/dependabot.yml`, update architecture/roadmap/README docs, verify release binary install, and merge `feat/truesight-dota-phase2` to `main`. | Dual-feature `--locked` suite + release build & install | Completed |

## 5. Phase 3 Deliverables (`v0.4.0-rc.1` – `v0.4.0`)

| Release / PR | Scope & Deliverable | Verification Gate | Status |
|--------------|---------------------|-------------------|--------|
| **`#75`–`#79` (`v0.4.0-rc.1`)** | Post-draft item-build recommender (`truesight-core::item_math`), patch `7.40` counter-trait tables, STRATZ onboarding & token linking, GSI local `items` parser, `SeededItemMetaRepository`, post-draft Build bar, and compact always-on-top overlay. | Core/adapter/app TDD tests + dual-feature Ubuntu/Windows CI | Completed (`b1a6520`) |
| **`#82`** | Overlay drag/resize state machine (`emilk/egui#7959` Wayland reset), vector HUD icons, `item_atlas.bin` embedding, global `Ctrl+Shift+P` (`hotkey.rs`), and complete **ES / EN / RU** GUI localization (`i18n.rs`). | Dual-feature GUI layout/hotkey/i18n tests + Ubuntu/Windows CI | Completed (`f6db505`) |
| **`#83` (`v0.4.0-rc.2`)** | Separate timed `EARLY > CORE > LATE` progression, early-boot variant grouping, role-filtered `Optional / Counters` (preserving owned off-role counters), expanded 184-icon `item_atlas.bin`, split Picks/Build responsive overlay geometry (`420x168` Build default, inline `START`, framed 3-box phases + `OPT · CNT`), deferred item cache refresh across Build entry/slot edits/vision/account unlink, and hero-bound `DraftEvent::InventoryUpdated` ownership across GSI queue coalescing and overflow reconciliation. | End-to-end GSI queue/overflow + GUI viewport layout + core/app TDD suites + Ubuntu/Windows CI | Completed (`37322b0`) |
| **`#86` (`Closes #84`)** | STRATZ role/bracket item sync resilient to partial OpenDota errors (decoupled `/api/constants/items`, pre-staging player validation, cached `(hero, role)` skip, faction-flip refresh), `4-Late` build engine (`MAX_PRIMARY_BUILD_STEPS = 8`), support late-utility & late boot collapsing, expanded `199`-icon `item_atlas.bin`, and non-swapping informational direct-upgrade chips (`Scepter`/`Blessing` deferral). | Core/adapter/app/GUI TDD suites (`208` all-features / `162` headless) + Ubuntu/Windows CI | Completed (`44bdab2`) |
| **`#88` (`Closes #87`, `v0.4.0`)** | Calibrated dual-pass `InGame` vision matching (`ingame_upper_core_rect` with `INGAME_UPPER_CORE_L1_THRESHOLD = 26.0`), `descriptor_region_stats` + `has_flat_occluded_border` occluded-slot rejection, and `1080p`/`1440p` Dota Plus badge & Visage overwrite regression matrix. | Adapter calibration/negative/overwrite TDD suites (`214` all-features / `168` headless) + Ubuntu/Windows CI | Completed (`d9a0729`) |

## 6. Phase 4 Deliverables (`v0.5.0`–`v0.5.1`)

| Release / PR | Scope & Deliverable | Verification Gate | Status |
|--------------|---------------------|-------------------|--------|
| **`#93` (`Closes #89`)** | Native Linux Wayland `zwlr_layer_shell_v1` Always-On-Top overlay presenter (`wayland_layer/`), cross-monitor drag anchoring, view-tagged geometry updates, and fallback `eframe` viewport routing. | Wayland/viewport backend routing & geometry TDD suites + Ubuntu/Windows CI | Completed (`99bde52`) |
| **`#97`, `#100`, `#101`** | Ranked `2-2-1` `PickPhase` top-bar portrait calibration (`x`-offsets & badge-free upper core) plus 5 Automaton Persona portraits in `hero_alt_atlas.bin` (`49` total variants across `30` heroes). | Adapter portrait & calibration TDD suites + Ubuntu/Windows CI | Completed (`c9d0eef`, `d50a740`, `b38a3ed`) |
| **`#98`, `#99` (`Closes #94`, `#91`)** | Gemstone faceted card surfaces (`surfaces.rs`), translucent frosted-glass overlay (`DEFAULT_OVERLAY_OPACITY`), and Windows overlay resize vibration fix. | GUI glass alpha rasterizer & viewport resize TDD suites + Ubuntu/Windows CI | Completed (`c3be6c4`, `dc3dcb5`) |
| **`#102` (`Closes #95`)** | Interactive `P1`–`P5` ally build selector (local hero invariant), overlay ally/enemy swap button with automatic GSI faction restoration, and centered overlay matrix/phase tables. | App/GUI selector, swap & centering TDD suites + Ubuntu/Windows CI | Completed (`e719a26`) |
| **`#104`** | Smooth Wayland overlay resize frame pacing (`ResizePacer::on_configured` reconciliation) and rasterizer fast-paths (SIMD/scanline opacity & tile caching). | Wayland resize pacer & rasterizer TDD suites + Ubuntu/Windows CI | Completed (`bdb64c9`) |
| **`#103` (`Closes #90`)** | Configurable click-through pass-through hotkey (`Ctrl+Shift+T` default) persisted in `ui_config.toml`, Options shortcut recorder with global/in-app capture guard, unmodified-only `F8` visibility toggle, and KDE Wayland XDG Portal `ConfigureShortcuts` re-bind button. | Hotkey & Options GUI TDD suites + Ubuntu/Windows CI | Completed (`3a41dac`) |
| **`#105`** | Automatic compact overlay lifecycle synced to passive Dota 2 process detection (`dota_process.rs` / `dota_watch.rs`), persisted `auto_overlay_with_dota` Options toggle, manual pin precedence, clean `click_through` persistence on exit, and Wayland output-hotplug zero-output wait & recovery. | Adapter/Session/Wayland hotplug TDD suites + Ubuntu/Windows CI | Completed (`8b2bea3`) |
| **`#106` (`v0.5.1`)** | Fixed solid/opaque Windows compact overlay background by upgrading `eframe` / `egui` / `epaint` `0.32 -> 0.37` (MSRV `1.95`), explicitly pinning `eframe::Renderer::Glow` so Windows WGL/DWM executes upstream transparent viewport handling (`emilk/egui#8423`, `#8616`), implementing transparent `App::clear_color`, disabling native drop shadow (`.with_has_shadow(false)`), and consuming `TexturesDelta::clear()`. | Core/adapter/app GUI tests (`269` all-features / `179` headless) + Ubuntu/Windows CI | Completed (`96394f7`) |
