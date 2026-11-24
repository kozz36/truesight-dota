# Architecture & Implementation Specification: `truesight-dota`

## 1. Executive Summary

`truesight-dota` is a native Rust real-time 5v5 draft analysis and probabilistic hero recommendation engine for Dota 2. It replicates and improves upon the probabilistic draft insights of Overwolf's *DotaPlus* and *STRATZ+* while eliminating Chromium/Electron memory overhead and enforcing a strict **Hexagonal Architecture (Ports & Adapters)**.

## 2. Solution Architecture & Hexagonal Boundaries

```
+-----------------------------------------------------------------------+
|                        DRIVING ADAPTERS                               |
|  - Native Desktop GUI (`eframe`/`egui`) & TrueColor 5v5 CLI/TUI       |
|  - BoundedGsiListener (HTTP POST 127.0.0.1:4001, 64 KiB cap, q=16)    |
|  - [Phase 2] VisionCaptureAdapter (Top-bar 10-slot pHash matcher)     |
+-----------------------------------+-----------------------------------+
                                    |
                                    v  (Ports: DraftEventSource)
+-----------------------------------------------------------------------+
|                     HEXAGON CORE (`truesight-core`)                   |
|  - Domain: HeroId(u16), Role(Pos 1..5), Bracket(8 Medals), AttackType |
|  - Math Engine v2:                                                    |
|      * Log-Odds Expected Winrate & Empirical Bayes Shrinkage          |
|      * Strictly Antisymmetric 5v5 Counter & Zero-Mean Synergy Grids   |
|      * 7-Day Patch Momentum, Comfort Recency Decay & Comp Balance     |
+-----------------------------------+-----------------------------------+
                                    |
                                    v  (Ports: StatsRepository)
+-----------------------------------------------------------------------+
|                         DRIVEN ADAPTERS                               |
|  - SeededMetaRepository (127 Heroes, 16,002 Pairs, 8 Individual Medals)|
|  - LocalCacheManager (~/.local/share/truesight-dota/cache, 24h/1h TTL)|
+-----------------------------------------------------------------------+
```

### Crate Responsibilities

| Crate | Role | I/O & Concurrency Contract |
|-------|------|----------------------------|
| `crates/truesight-core` | Pure domain model, mathematical 5v5 draft engine (`math.rs`), post-draft item-build recommender (`item_math.rs`), patch `7.40` `CounterTrait` tables, and port traits (`StatsRepository`, `ItemMetaRepository`, `DraftEventSource`). | Zero I/O, zero network, zero async dependencies. `#![forbid(unsafe_code)]`. |
| `crates/truesight-adapters` | Infrastructure implementations for `StatsRepository` & `ItemMetaRepository` (`SeededMetaRepository`, `SeededItemMetaRepository`, `item_catalog_data.rs`, `LocalCacheManager` with 24h Meta/Item TTL and 1h Player TTL) and `DraftEventSource` (`BoundedGsiListener`, `VisionCaptureAdapter`). | Bounded TCP read (`MAX_GSI_PAYLOAD_BYTES = 64 KiB`), bounded `BoundedDraftQueue` (`capacity = 16`) with causal coalescing & overflow reconciliation, 6-second HTTP sync timeout, explicit shutdown on `Drop`. |
| `crates/truesight-app` | User-facing native `eframe`/`egui` v4 Draft Oracle desktop GUI (default) + Picks/Build compact overlay (`overlay.rs`, `overlay_build.rs`, `build_bar.rs`, `onboarding.rs`, `hotkey.rs`, **ES / EN / RU** `i18n.rs`), interactive TrueColor 5v5 CLI/TUI (`--cli`), live sync (`--sync`), and `--demo` mode. | Separates deterministic state transitions (`AppSession`) from GUI/TUI rendering and handles `BrokenPipe` cleanly. Embeds binary RGB atlases (`hero_atlas.bin`, `hero_alt_atlas.bin`, `item_atlas.bin`) directly into `egui::ColorImage`. Additive `gui` Cargo feature (`--no-default-features` builds pure CLI with 0 external crates). |

## 3. Mathematical Formulation (Engine v2)

### 3.1 8-Medal Base Meta Winrate & 7-Day Patch Momentum

Each hero profile stores empirical winrates across all **8 individual Dota 2 medals** (`Herald`, `Guardian`, `Crusader`, `Archon`, `Legend`, `Ancient`, `Divine`, `Immortal`) plus a 7-day momentum term $\Delta T_i \in [-0.025, +0.025]$ derived from `pub_pick_trend` and `pub_win_trend`:

$$W_{i, r, b} = \text{clamp}\left( W_{i, b} + \text{offset}_r + 0.45 \cdot \Delta T_i,\; 0.05,\; 0.95 \right)$$

### 3.2 Cohort-Calibrated Antisymmetric Counters & Double-Centered Synergies

To guarantee exact mathematical consistency when swapping teams ($P(A \text{ vs } B) + P(B \text{ vs } A) = 1.0$), the $127 \times 127$ interaction matrix enforces:

1. **Strict Counter Antisymmetry** across every medal $b$:
   $$\Delta C_b(A, B) = \tau_b \cdot \Delta C_0(A, B) + 0.22 \cdot (d_{A, b} - d_{B, b}) = -\Delta C_b(B, A)$$
   where $\tau_b \in [0.86, 1.12]$ is the medal execution sharpness factor and $d_{i, b} = W_{i, b} - \bar{W}_i$ is Hero $i$'s medal skill-scaling shift.
2. **Zero-Mean Double-Centered Synergy Symmetry**:
   $$\Delta S_b(A, B) = \tau_b \cdot \Delta S_0(A, B) + 0.075 \cdot (d_{A, b} + d_{B, b}) = \Delta S_b(B, A)$$
3. **Pure Row Averages**:
   - `Avg Counter` in row $i$ is the exact arithmetic mean $\frac{1}{|E|} \sum_{j \in E} \Delta C_b(A_i, E_j)$.
   - `Avg Synergy` in row $i$ is the exact arithmetic mean $\frac{1}{|A|-1} \sum_{k \neq i} \Delta S_b(A_i, A_k)$.

### 3.3 $\sqrt{N}$ Variance Normalization & Structural Composition Balance

Because 25 pairwise counters and 10 pairwise synergies occur inside a single correlated 5v5 match, team-wide sums are normalized by $\sqrt{N}$:

$$E_{\text{meta}} = \frac{1}{\sqrt{5}} \left( \sum_{i \in A} (W_{A_i} - 0.5) - \sum_{j \in E} (W_{E_j} - 0.5) \right)$$

$$E_{\text{counter}} = \frac{1}{\sqrt{|A|\cdot|E|}} \sum_{i \in A, j \in E} \Delta C_b(A_i, E_j), \quad E_{\text{synergy}} = \frac{1}{\sqrt{\binom{|A|}{2}}} \sum_{i<k} \Delta S_b(A_i, A_k) - \frac{1}{\sqrt{\binom{|E|}{2}}} \sum_{j<l} \Delta S_b(E_j, E_l)$$

In addition, `evaluate_team_composition` evaluates `AttackType` (`Melee` vs `Ranged`), `has_disable`, and `has_initiation` to penalize all-melee (`-1.8%`) or zero-catch (`-1.5%`) drafts and reward balanced frontline + disable lineups (`+0.5%`), yielding $E_{\text{comp}} = C_{\text{allies}} - C_{\text{enemies}}$.

### 3.4 Logit Win Probability & Personal Comfort Engine v3

$$\text{Logit}(P_{\text{win}}) = 3.2 \cdot \left( w_{\text{meta}} E_{\text{meta}} + w_{\text{counter}} E_{\text{counter}} + w_{\text{synergy}} E_{\text{synergy}} + 0.85 \cdot E_{\text{comp}} + \frac{w_{\text{comfort}}}{\sqrt{5}} E_{\text{comfort}} \right)$$

`recommend_heroes` evaluates the exact simulated 5v5 draft with each candidate placed in `target_role.index()`, ensuring `Projected Win%` is **100% identical** to the header `ALLIES WIN PROBABILITY` upon picking.

**Personal Comfort Engine v3 (`HeroProfile::comfort_delta`)**:
- Rather than relying on a single `last_played` timestamp over career totals (which allows one recent match to revive years-old matches), `LocalCacheManager` and `OPENDOTA_HERO_SNAPSHOT` compute per-match exponentially time-decayed effective games $G_{\text{eff}} = \sum_k e^{-\Delta t_k / 75\text{d}}$ and blended effective wins $W_{\text{eff}}$ across `/api/players/{account_id}/matches` (`player_matches.json`) and `/heroes` (`player_heroes.json`).
- Heroes with $G_{\text{eff}} < 0.15$ (e.g., unplayed for ~10 months) decay cleanly to `0.00%`.
- Active heroes combine empirical-Bayes performance ($m = 16$ at $50\%$) with an active practice volume mastery bonus:
  $$\Delta_{\text{comfort}} = \text{clamp}\!\left(\frac{W_{\text{eff}} + 8.0}{G_{\text{eff}} + 16.0} - 0.50 + \mathbb{I}(\widehat{WR}_{\text{eff}} \ge 0.50 \land G_{\text{eff}} \ge 0.50) \cdot 0.0065 \ln(1 + G_{\text{eff}}),\; -0.05,\; +0.05\right)$$

## 4. Database Synchronization, Account Resolution & Vision Capture

`LocalCacheManager` (`crates/truesight-adapters/src/sync.rs`) manages `~/.local/share/truesight-dota/cache/`:
- **Meta Snapshot (`opendota_hero_stats.json`)**: **24-hour TTL (`86,400 s`)**, matching OpenDota's daily rolling aggregation window.
- **Per-Account Player Comfort & Match History (`players/{account_id}/player_heroes.json` + `player_matches.json`)**: **1-hour TTL (`3,600 s`)**, capturing both lifetime totals and the latest 500 matches played by the linked Steam32 account.
- **3-Tier Steam32 Account Resolution**: `(1)` Explicit override (`TRUESIGHT_STEAM32_ID`, `~/.config/truesight-dota/config.toml`, `--account-id`, or the GUI **STEAM ACCOUNT LINK** modal) $\to$ `(2)` Dota 2 GSI `"player.steamid"` (`steam64 - 76561197960265728`) $\to$ `(3)` local Steam `loginusers.vdf` (`"MostRecent" "1"`). Unlinked users evaluate cleanly against neutral `+0.00%` personal comfort defaults.
- **Optional STRATZ Empirical `with` Synergy**: When `STRATZ_API_TOKEN` is configured, overlays empirical GraphQL `with` synergy weights in `SeededMetaRepository` while preserving counter antisymmetry and double-centered synergy symmetry.
- **GSI Faction & Vision Capture (`crates/truesight-adapters/src/vision.rs`)**: `BoundedGsiListener` extracts `"player.team_name"` (`radiant` | `dire`) into `DraftEvent::FactionDetected` and `DraftState.ally_faction` while keeping `P(Allies) + P(Enemies) = 1.0`. `VisionCaptureAdapter` classifies the 10 top-bar portraits against the embedded 127-hero RGB atlas (`crates/truesight-adapters/assets/hero_atlas.bin`) and maps Radiant (`0..4`) vs Dire (`5..9`) onto `Side::Allies` / `Side::Enemies`.
- **Non-blocking Startup & Manual Sync**: Stale caches trigger a background refresh thread at startup without delaying GUI startup; users can also force an immediate refresh via the **`[Sync DB]`** button in the GUI or `truesight-dota --sync` in the terminal. When switching allies or replacing a slot while Build view is open, `AppSession::request_item_build_refresh_if_uncached()` starts (or queues via `pending_item_build_refresh`) an item-stats refresh and drains across active-sync cancellation/account switches.

### 4.1 Post-Draft Item Build Engine & Live GSI Inventory Ownership
- **Timed Progression & Counter Filtering (`crates/truesight-core/src/item_math.rs`)**: `recommend_item_build` separates `Starting` items, timed build progression grouped into `Early` (`EARLY`), `Core` (`CORE`), and `Late` (`LATE`) phases (`MAX_PRIMARY_BUILD_STEPS = 6`), mutually exclusive early boots (`Power Treads`, `Phase Boots`, `Arcane Boots`, `Tranquil Boots`) collapsed into one `Early` boot step + selectable variants, and role-filtered `Optional / Counters` driven by enemy patch `7.40` `CounterTrait` coverage. Owned partial components add `x/y` progress, score bonus, and timing pull, while owned counters remain visible under `Optional` without recommending unowned off-role items.
- **Causal Hero-Bound GSI Inventory (`truesight-core::ports`, `truesight-adapters::gsi`, `AppSession`)**: `parse_gsi_json_events` tags `DraftEvent::InventoryUpdated` with the payload's `hero.name` `hero_slug: Option<String>`. `BoundedDraftQueue::record_and_push_gsi_events()` stamps `latest_local_hero` onto items-only updates (`hero_slug: None`), `push_into_inner()` evicts stale queued `InventoryUpdated` snapshots when the local hero changes, and `GsiStateSnapshot::push_reconcile_events()` emits `LocalPlayerHeroDetected` strictly before any matching `InventoryUpdated` after overflow/critical-event eviction. Inside `AppSession`, `owned_items_owner` binds inventory exclusively to `InventoryUpdated`, invalidating stale purchases on hero switches and scoping `active_build_owned_items()` away from browsed teammates or cleared-slot fallback allies.
- **UI & Split Responsive Overlay (`crates/truesight-app/src/gui/`)**: `build_bar.rs` renders the main-window 3-column phased build and `Optional / Counters`; `overlay.rs` and `overlay_build.rs` maintain independent persisted Picks (`280x160` min) and Build (`420x168` default, `220x104` min) geometries, framing `EARLY > CORE > LATE`, inline `START` chips, overlaid timings, `NEXT`/`OK`/`x/y` badges, framed `OPT · CNT` bar, and click-through `PASS` toggled globally via `Ctrl+Shift+P` (`hotkey.rs`).

## 5. Toolchain & Validation Matrix

| Parameter | Value | Description |
|-----------|-------|-------------|
| `edition` | `2024` | Workspace Rust edition |
| `rust-version` (MSRV) | `1.85` | Minimum supported Rust version declared in manifests |
| `selected_toolchain` | `rustc 1.99.1 (58cf8f9c3 2026-10-13)` | Pinned via `rust-toolchain.toml` on CachyOS Linux (`x86_64-unknown-linux-gnu`) |
| `TRUESIGHT_GSI_ADDR` | `127.0.0.1:4001` (default) | Local bind address for Dota 2 Game State Integration HTTP POST payloads |

See [`docs/roadmap-and-tasks.md`](roadmap-and-tasks.md) for the complete `T1`–`T16` Phase 1 and `T17`–`T22` Phase 2 task log.
