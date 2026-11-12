# TrueSight Dota (`truesight-dota`)

High-performance, native Rust 5v5 live draft analyzer and probabilistic hero recommender for Dota 2. Built on a strict **Hexagonal Architecture (Ports & Adapters)** with zero runtime bloat, empirical Bayes shrinkage, strictly antisymmetric $127 \times 127$ matchup matrices, 8-medal scaling, and automatic OpenDota cache synchronization.

## Quick Path

1. **Launch the Native Desktop GUI** (or from your CachyOS/KDE App Launcher as **TrueSight Dota**):
   ```bash
   truesight-dota
   ```
2. **Force an immediate database sync from OpenDota** (`heroStats` 24h TTL + Player `127255741` 1h TTL):
   ```bash
   truesight-dota --sync
   ```
3. **Run the live 5v5 draft CLI/TUI demo**:
   ```bash
   truesight-dota --demo
   ```
4. **Start the interactive CLI/TUI draft session in terminal** (`127.0.0.1:4001`):
   ```bash
   truesight-dota --cli
   ```
5. **Run the full deterministic test suite**:
   ```bash
   cargo test --workspace --all-features && cargo test --workspace --no-default-features
   ```

## Key Features & Statistical Guarantees

| Capability | Implementation |
|------------|----------------|
| **Hexagonal Core (`truesight-core`)** | Zero I/O, zero async, `#![forbid(unsafe_code)]`. Stack-allocated `DraftState` (`Copy`) and dense `MatchupMatrix` (`O(1)` lookup across all 127 heroes and 16,002 pairs). |
| **8 Individual Dota 2 Medals** | Instant switching across `Herald`, `Guardian`, `Crusader`, `Archon`, `Legend`, `Ancient`, `Divine`, and `Immortal`, dynamically scaling meta winrates, counter sharpness ($\tau_b$), and pairwise synergies. |
| **Mathematical Invariants** | Strict counter antisymmetry ($\Delta C(A,B) = -\Delta C(B,A)$), zero-mean double-centered synergy ($\Delta S(A,B) = \Delta S(B,A)$), $\sqrt{N}$ variance normalization, and exact identity between `Projected Win%` and header `ALLIES WIN PROBABILITY`. |
| **Engine v2 Signals** | Incorporates **7-day patch momentum** (`pub_pick_trend`/`pub_win_trend`), **exponential comfort recency decay** (`last_played`), and **structural team composition balance** (`Melee`/`Ranged` ratio + `Disabler`/`Initiator` coverage). |
| **Auto-Sync Local Cache (`24h` / `1h` TTL)** | Stores cached OpenDota snapshots in `~/.local/share/truesight-dota/cache/` with automatic background refresh on startup (`24h` Meta TTL, `1h` Player TTL) plus a **`[Sync DB]`** button in the GUI. |
| **Hands-Free Slot Picker** | Click any Ally (`A1..A5`) or Enemy (`E1..E5`) slot button once, type the hero name directly with auto-focus, and press `Enter` to assign. |

## Interactive CLI Commands (`truesight-dota --cli`)

```text
ally <1..5> <hero>      # Assign hero to Ally slot A1..A5 (or 'none' to clear)
enemy <1..5> <hero>     # Assign hero to Enemy slot E1..E5 (or 'none' to clear)
ban <hero>              # Toggle hero ban
role <1..5>             # Set target recommendation role (Pos 1..5)
bracket <medal>         # Switch medal: herald | guardian | crusader | archon | legend | ancient | divine | immortal
sync                    # Force live refresh from OpenDota API into ~/.local/share/truesight-dota/cache/
reset                   # Clear current draft
quit                    # Exit session
```

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — Hexagonal architecture, Engine v2 mathematical equations, and GSI / Vision port specifications.
- [`docs/roadmap-and-tasks.md`](docs/roadmap-and-tasks.md) — Formal engineering task plan (`T1`–`T13`), TDD verification gates, and data sync schedule.
