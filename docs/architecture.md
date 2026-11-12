# Architecture & Implementation Specification: `truesight-dota`

## 1. Executive Summary

`truesight-dota` is a native Rust real-time draft analysis and hero recommendation system for Dota 2. It replicates and improves upon the probabilistic draft insights of Overwolf's *DotaPlus* and *STRATZ+* while eliminating Chromium/Electron memory overhead and enforcing a clean **Hexagonal Architecture (Ports & Adapters)**.

## 2. Solution Architecture & Hexagonal Boundaries

```
+-----------------------------------------------------------------------+
|                        DRIVING ADAPTERS                               |
|  - Interactive CLI / TrueColor 5v5 Matrix & Native Desktop UI         |
|  - BoundedGsiListener (HTTP POST 127.0.0.1:4001, 64 KiB cap, q=16)    |
|  - [Phase 2] VisionCaptureAdapter (Top-bar 10-slot pHash matcher)     |
+-----------------------------------+-----------------------------------+
                                    |
                                    v  (Ports: DraftEventSource)
+-----------------------------------------------------------------------+
|                     HEXAGON CORE (`truesight-core`)                   |
|  - Domain: HeroId(u16), Role, Bracket, DraftState (Copy, 0 heap)      |
|  - Math Engine:                                                       |
|      * Log-Odds Expected Winrate & Empirical Bayes Shrinkage          |
|      * 5v5 Counter Matrix (Allies vs Enemies) & Synergy Matrix        |
|      * O(N) Candidate Simulator & Personal Comfort Weighting          |
+-----------------------------------+-----------------------------------+
                                    |
                                    v  (Ports: StatsRepository)
+-----------------------------------------------------------------------+
|                         DRIVEN ADAPTERS                               |
|  - SeededMetaRepository (High-MMR STRATZ / OpenDota baseline)         |
|  - Local Snapshot / GraphQL Sync Adapter                              |
+-----------------------------------------------------------------------+
```

### Crate Responsibilities

| Crate | Role | I/O & Concurrency Contract |
|-------|------|----------------------------|
| `crates/truesight-core` | Pure domain model, mathematical 5v5 engine, and port traits (`StatsRepository`, `DraftEventSource`). | Zero I/O, zero network, zero async dependencies. `#![forbid(unsafe_code)]`. |
| `crates/truesight-adapters` | Infrastructure implementations for `StatsRepository` (`SeededMetaRepository` with all 127 OpenDota heroes + player `127255741` comfort pool) and `DraftEventSource`. | Bounded TCP read (`MAX_GSI_PAYLOAD_BYTES = 64 KiB`), bounded `mpsc::sync_channel(16)` backpressure, explicit shutdown on `Drop`. |
| `crates/truesight-app` | User-facing native `eframe`/`egui` desktop GUI (`--gui`), interactive TrueColor 5v5 CLI/TUI, and `--demo` verification mode. | Separates deterministic state transitions (`AppSession`) from GUI/TUI rendering and handles `BrokenPipe` cleanly. Additive `gui` Cargo feature (`--no-default-features` builds pure CLI with 0 external crates). |

## 3. Mathematical Formulation

### 3.1 Log-Odds Expected Head-to-Head & Synergy Baseline

To avoid double-counting a hero's base patch strength when evaluating pairwise matchups, `truesight-core` computes the expected head-to-head winrate $E_{\text{vs}}(A, B)$ and expected joint team winrate $E_{\text{with}}(A, B)$ from each hero's primary-role meta winrate ($W_A, W_B$):

$$E_{\text{vs}}(A, B) = \frac{W_A (1 - W_B)}{W_A (1 - W_B) + (1 - W_A) W_B}$$

$$E_{\text{with}}(A, B) = \frac{W_A W_B}{W_A W_B + (1 - W_A)(1 - W_B)}$$

### 3.2 Empirical Bayes Shrinkage

Raw pairwise winrates with small sample sizes are shrunk toward the expected baseline using $m = 50$ virtual prior matches:

$$\widehat{W}_{\text{vs}}(A, B) = \frac{\text{wins}_{\text{vs}} + m \cdot E_{\text{vs}}(A, B)}{\text{matches}_{\text{vs}} + m}$$

### 3.3 5v5 Green / Red Counter and Synergy Cells

Each cell in the 5v5 matrix represents the isolated interaction delta:

$$\Delta C(A_i, E_j) = \widehat{W}_{\text{vs}}(A_i, E_j) - E_{\text{vs}}(A_i, E_j)$$

$$\Delta S(A_i, A_k) = \widehat{W}_{\text{with}}(A_i, A_k) - E_{\text{with}}(A_i, A_k)$$

- **Positive values ($\Delta > 0$)** render in **Green**: the hero outperforms its base meta expectation against that enemy or alongside that ally.
- **Negative values ($\Delta < 0$)** render in **Red**: the hero underperforms its base meta expectation in that pairing.

### 3.4 Draft Win Probability & Hero Recommendation

All edges are combined in Logit space and mapped through the logistic sigmoid $\sigma(x) = (1 + e^{-x})^{-1}$:

$$\text{Logit}(P_{\text{win}}) = 4.0 \cdot \left( w_{\text{meta}} \sum \Delta M + w_{\text{counter}} \sum_{i,j} \Delta C(A_i, E_j) + w_{\text{synergy}} \Delta S_{\text{net}} + w_{\text{comfort}} \Delta P_{\text{comfort}} \right)$$

## 4. Prerequisites & Toolchain Matrix

| Parameter | Value | Description |
|-----------|-------|-------------|
| `edition` | `2024` | Workspace Rust edition |
| `rust-version` (MSRV) | `1.85` | Minimum supported Rust version declared in manifests |
| `selected_toolchain` | `rustc 1.99.1 (58cf8f9c3 2026-10-13)` | Pinned via `rust-toolchain.toml` on CachyOS Linux (`x86_64-unknown-linux-gnu`) |
| `TRUESIGHT_GSI_ADDR` | `127.0.0.1:4001` (default) | Local bind address for Dota 2 Game State Integration HTTP POST payloads |

## 5. Implementation & Usage Guide

### Step 1: Run Unit & Integration Tests
```bash
cargo test --workspace
```

### Step 2: Run Live 5v5 Demonstration
```bash
cargo run -p truesight-app -- --demo
```

### Step 3: Interactive Draft Analysis
```bash
cargo run -p truesight-app
```
Inside the prompt:
```text
ally 3 magnus
enemy 1 medusa
enemy 2 storm spirit
role 1
```

## 6. Validation & Sanity Check

- [x] Rejected duplicate hero picks leave `DraftState` bit-for-bit unchanged.
- [x] Out-of-range slots (`slot >= 5`) return `DomainError::InvalidSlot` without panic.
- [x] `BoundedGsiListener` caps HTTP bodies at 64 KiB and drops excess bursts via `try_send` when the 16-event queue is full.
- [x] Early stdout pipe closure (`--demo | head -n 5`) exits cleanly with code `0` and no panic backtrace.

## 7. Rollback Plan

1. Revert individual work-unit commits on `feat/truesight-dota-foundation` (`git revert <commit>`).
2. Remove `/data/games/SteamLibrary/steamapps/common/dota 2 beta/game/dota/cfg/gamestate_integration/gamestate_integration_truesight.cfg` if GSI监听 is no longer desired.

## 8. Maintenance & Phase 2 Extension (Screen Capture Vision Adapter)

Because Valve suppresses the `"draft"` block in GSI for active players in Ranked All Pick, Phase 2 adds a `VisionCaptureAdapter` implementing `DraftEventSource`:
1. Listen for `DraftEvent::GameRulesStateChanged { state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION", .. }`.
2. Capture only the top 10 hero portrait rectangles at 1 Hz via PipeWire/X11.
3. Compute 64-bit perceptual hashes (`pHash`) against the 126 hero portrait templates and emit `DraftEvent::PickDetected { side, slot, hero_id, role }`.
