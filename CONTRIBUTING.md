# Contributing to TrueSight Dota

Thank you for contributing to `truesight-dota` (`Apache-2.0`).

## Architecture Overview

`truesight-dota` follows strict **Hexagonal (Ports & Adapters) Architecture**:

- `crates/truesight-core`: Pure domain types, deterministic statistical draft math (`ModelWeights`, Comfort v3, 8-medal bracket modeling), and port traits. Must remain 100% free of filesystem, network, and external dependencies.
- `crates/truesight-adapters`: OpenDota/STRATZ sync & TTL cache (`24h` meta / `1h` per-account), bounded loopback GSI HTTP listener, Steam `loginusers.vdf` 3-tier account resolver, and `VisionCaptureAdapter`.
- `crates/truesight-app`: CLI/TUI (`--cli`, `--demo`, `--sync`, `--account-id`) and native `eframe`/`egui` Draft Oracle HUD desktop GUI.

## Development Setup

- **Rust Toolchain**: Edition `2024`, MSRV `1.95` (local development pinned via `rust-toolchain.toml`).
- **Linux System Libraries** (required for default `gui` feature):
  ```bash
  sudo apt-get install -y pkg-config libxkbcommon-dev libxcb-render0-dev \
    libxcb-shape0-dev libxcb-xfixes0-dev libwayland-dev \
    libgl1-mesa-dev libegl1-mesa-dev libgtk-3-dev
  ```

## Non-Negotiable Statistical & Code Invariants

1. **Counter Antisymmetry (All 8 Medals)**: $\Delta C_b(A, B) = -\Delta C_b(B, A)$ and $\Delta C_b(A, A) = 0$.
2. **Double-Centered Zero-Mean Synergy Symmetry**: $\Delta S_b(A, B) = \Delta S_b(B, A)$ with $\sum_{A,B} \Delta S_0(A, B) \approx 0$.
3. **Exact Team-Swap Complementarity**: Swapping all 5 Allied and 5 Enemy heroes under neutral comfort gives $P(\text{Allies}) + P(\text{Enemies}) = 1.0$.
4. **Exact `Projected Win%` Parity**: `recommend_heroes` matches `evaluate_draft_5v5` with `0.00%` drift.
5. **Personal Comfort Engine v3**: Exponential recency decay ($\tau = 75\text{d}$, inactive cutoff $G_{\text{eff}} < 0.15 \implies 0.0$) and positive mastery bonus only for active heroes with $\widehat{WR}_{\text{eff}} \ge 0.50$.

## Testing & Verification Workflow

All changes must follow **Red-Green TDD** and pass both Cargo feature profiles with **zero warnings** before opening a PR:

```bash
cargo test --workspace --all-features --locked
cargo test --workspace --no-default-features --locked
```

## Privacy & Commit Standards

- **Never commit personal data**: Do not add personal Steam32/Steam64 IDs, `player_*.json` match dumps, or API tokens to source files, tests, or docs. Use synthetic fixtures in tests.
- **Conventional Commits**: Use structured prefixes (`feat:`, `fix:`, `refactor:`, `ci:`, `docs:`, `test:`).
- **Rustdoc Standards**: Document important `pub` cross-crate APIs, invariants, and external I/O contracts with concise `///` comments, and add selective docs to internal helpers when precedence, fallbacks, or platform edge cases are non-obvious (see [`AGENTS.md`](AGENTS.md)).
