# TrueSight Dota (`truesight-dota`)

High-performance, native Rust 5v5 live draft analyzer and probabilistic hero recommender for Dota 2. Built on a strict **Hexagonal Architecture (Ports & Adapters)** with zero runtime bloat, empirical Bayes shrinkage, and log-odds matchup evaluation.

## Quick Path

1. **Run the live 5v5 draft demo**:
   ```bash
   cargo run -p truesight-app -- --demo
   ```
2. **Start the interactive live draft session + Dota 2 GSI listener** (`127.0.0.1:4001`):
   ```bash
   cargo run -p truesight-app
   ```
3. **Run the full deterministic test suite**:
   ```bash
   cargo test --workspace
   ```

## Architectural Highlights

| Area | Decision |
|------|----------|
| **Hexagonal Core (`truesight-core`)** | Zero I/O, zero async, `#![forbid(unsafe_code)]`. Stack-allocated `DraftState` (`Copy`) and dense `MatchupMatrix` (`O(1)` lookup). |
| **Mathematical Engine** | Contrasts role-specific **Base Meta Winrate** against live match picks using **Log-Odds Expected Winrate** and **Empirical Bayes Shrinkage** ($m = 50$ prior matches) to eliminate small-sample noise and double-counting. |
| **5v5 Live Matrix** | Computes cell-by-cell **Counter Deltas** ($\Delta C_{i,j}$, green for positive advantage, red for negative) and **Allied Synergy Deltas** ($\Delta S_{i,k}$). |
| **Bounded Adapters (`truesight-adapters`)** | Implements `StatsRepository` (seeded STRATZ/OpenDota baseline) and `DraftEventSource` (`BoundedGsiListener` with 64 KiB payload cap and bounded `sync_channel(16)` admission control). |
| **Phase 2 Vision Ready** | Screen-capture template matching plugs directly into `DraftEventSource` (`DraftEvent::PickDetected`) without modifying `truesight-core`. |

## Dota 2 Game State Integration (GSI) Setup

Create `/data/games/SteamLibrary/steamapps/common/dota 2 beta/game/dota/cfg/gamestate_integration/gamestate_integration_truesight.cfg`:

```text
"TrueSight Dota Integration v1"
{
    "uri"           "http://127.0.0.1:4001/"
    "timeout"       "5.0"
    "buffer"        "0.1"
    "throttle"      "0.1"
    "heartbeat"     "10.0"
    "data"
    {
        "provider"  "1"
        "map"       "1"
        "player"    "1"
        "hero"      "1"
        "draft"     "1"
    }
}
```

## Verification Checklist

- [x] `cargo test -p truesight-core` — verifies Bayesian shrinkage, log-odds symmetry, 5v5 green/red cells, and recommendation ranking.
- [x] `cargo test -p truesight-adapters` — verifies seeded matchup repository and bounded GSI HTTP listener.
- [x] `cargo test -p truesight-app` — verifies interactive draft commands and 5v5 matrix rendering.

## Next Step

See [`docs/architecture.md`](docs/architecture.md) for the complete mathematical formulation, hexagonal port contracts, and Phase 2 screen-capture adapter specification.
