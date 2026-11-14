# AGENTS.md — Engineering, Architecture & Testing Policy (`truesight-dota`)

This file defines the mandatory implementation, architectural, statistical, and testing rules for AI agents and human contributors working on `truesight-dota`.

## Quick Path

1. **Load Project Skills First**: Read [`skills/truesight-dota-domain/SKILL.md`](skills/truesight-dota-domain/SKILL.md) and [`.agents/skills/rust-architect/SKILL.md`](.agents/skills/rust-architect/SKILL.md) before modifying any domain model, probability formula, OpenDota adapter, or UI workflow.
2. **Execute Red-Green-Refactor TDD**: Write a failing deterministic test (`RED`) in the target crate before writing production code (`GREEN`).
3. **Run Full Verification Suite** (both feature profiles must pass with zero warnings):
   ```bash
   cargo test --workspace --all-features
   cargo test --workspace --no-default-features
   ```

---

## 1. Hexagonal Architecture & Rust Toolchain Policy

| Boundary / Rule | Requirement |
|-----------------|-------------|
| **Toolchain & Edition** | `edition = "2024"`, `rust-version = "1.85"` (MSRV), pinned to `rustc 1.99.1` via `rust-toolchain.toml`. |
| **Memory & Safety** | `#![forbid(unsafe_code)]` and `unused_must_use = "deny"` across all workspace crates. Zero `.unwrap()` / `.expect()` on runtime external input. |
| **`crates/truesight-core`** | **100% pure Hexagon Core**: zero I/O, zero filesystem, zero network, zero async, zero external dependencies. Owns `domain`, `math`, and `ports`. |
| **`crates/truesight-adapters`** | **Driven & Driving Infrastructure**: owns `SeededMetaRepository`, `LocalCacheManager` (`24h`/`1h` TTL), and `BoundedGsiListener` (`MAX_GSI_PAYLOAD_BYTES = 64 KiB`, `sync_channel(16)`). |
| **`crates/truesight-app`** | **Presentation & Composition Root**: owns `AppSession`, native `eframe`/`egui` desktop GUI (default `gui` feature), and TrueColor CLI/TUI (`--cli`, `--demo`, `--sync`). |
| **Module Cohesion & Test Separation** | No production source file (excluding generated seed data `snapshot_pairs.rs`) may exceed ~800–900 lines or mix unrelated layers. All unit and regression test suites MUST reside in dedicated `tests.rs` files wired via `#[cfg(test)] mod tests;`, never inline inside `lib.rs`, `main.rs`, or UI panel files. |

---

## 2. Statistical & Mathematical Invariants (Non-Negotiable)

Every change to `truesight-core::math` or `truesight-adapters` MUST preserve these 5 mathematical laws:

1. **Strict Counter Antisymmetry (All 8 Medals)**:
   $$\Delta C_b(A, B) = -\Delta C_b(B, A) \quad \text{and} \quad \Delta C_b(A, A) = 0$$
2. **Double-Centered Zero-Mean Synergy Symmetry**:
   $$\Delta S_b(A, B) = \Delta S_b(B, A) \quad \text{with} \quad \sum_{A, B} \Delta S_0(A, B) \approx 0$$
3. **Exact Team-Swap Complementarity**:
   Swapping all 5 Allied picks with all 5 Enemy picks (under neutral personal comfort) MUST satisfy $P(\text{Allies}) + P(\text{Enemies}) = 1.0$.
4. **Exact `Projected Win%` Parity**:
   In `recommend_heroes`, `Projected Win%` for candidate hero $H$ in `draft.target_role` MUST equal `evaluate_draft_5v5` (`0.00%` drift) when $H$ is placed into `draft.allies[draft.target_role.index()]`.
5. **Personal Comfort Engine v3**:
   Personal comfort MUST use per-match exponential recency decay ($\tau = 75\text{d}$, floor `0.0`, inactive cutoff $G_{\text{eff}} < 0.15 \implies 0.0$) over `/api/players/{id}/matches` combined with active volume mastery $0.0065 \ln(1 + G_{\text{eff}})$ (only when $\widehat{WR}_{\text{eff}} \ge 0.50$).

---

## 3. Testing & Release Gate Checklist

- [ ] **Red-Green TDD**: Every bugfix or feature starts with a unit test reproducing the defect or specifying the new behavior.
- [ ] **Dual Feature Matrix**: Both `cargo test --workspace --all-features` (GUI + CLI) and `cargo test --workspace --no-default-features` (headless zero-dependency CLI) pass.
- [ ] **Desktop Binary Installation**: When updating `truesight-app` or embedded snapshots, install the release binary atomically via `install -m 755 target/release/truesight-dota ~/.local/bin/truesight-dota` (never raw `cp`, which fails with `Text file busy` if the GUI is running).
- [ ] **Conventional Commits**: Commit reviewable work units (`feat:`, `fix:`, `docs:`, `test:`) without AI attribution footers.

---

## 4. Project Skills

| Skill | Trigger & Purpose | Path |
|-------|-------------------|------|
| `truesight-dota-domain` | Trigger: `truesight-dota`, Dota 2 5v5 draft math, counter synergy matrix, OpenDota cache sync, Comfort v3. Enforce TrueSight Dota domain and TDD invariants. | [`skills/truesight-dota-domain/SKILL.md`](skills/truesight-dota-domain/SKILL.md) |
| `rust-architect` | Systems-grade Rust engineering skill (`edition = "2024"`, MSRV `1.85`, `#![forbid(unsafe_code)]`, hexagonal boundaries, CLI/TUI & desktop architecture). | [`.agents/skills/rust-architect/SKILL.md`](.agents/skills/rust-architect/SKILL.md) |
