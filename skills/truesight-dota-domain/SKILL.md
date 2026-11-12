---
name: truesight-dota-domain
description: "Trigger: truesight-dota, Dota 2 5v5 draft math, counter synergy matrix, OpenDota cache sync, Comfort v3. Enforce TrueSight Dota domain and TDD invariants."
license: Apache-2.0
metadata:
  author: "kozz36"
  version: "1.0"
---

## Activation Contract

Load this skill whenever modifying or reviewing `truesight-dota` crates (`truesight-core`, `truesight-adapters`, `truesight-app`), updating the 5v5 probabilistic draft engine, tuning 8-medal bracket scaling, calibrating Personal Comfort Engine v3, or changing OpenDota cache synchronization (`LocalCacheManager`).

## Hard Rules

- **Strict Hexagonal Isolation**: `truesight-core` MUST remain 100% pure (zero I/O, zero network, zero async, zero external crates, `#![forbid(unsafe_code)]`). All HTTP, filesystem, and GSI operations live in `truesight-adapters`.
- **Mathematical Invariants**:
  - Counter matrix MUST be strictly antisymmetric across all 8 medals: $\Delta C_b(A, B) = -\Delta C_b(B, A)$ and $\Delta C_b(A, A) = 0$.
  - Synergy matrix MUST be double-centered zero-mean and symmetric: $\Delta S_b(A, B) = \Delta S_b(B, A)$.
  - Team swap invariant MUST hold: $P(\text{Allies vs Enemies}) + P(\text{Enemies vs Allies}) = 1.0$.
  - `Projected Win%` in `recommend_heroes` MUST equal `evaluate_draft_5v5` (`0.00%` drift) when the candidate hero is placed in `draft.target_role.index()`.
- **Personal Comfort v3**: Never use a single `last_played` timestamp over lifetime totals alone. Compute per-match exponentially time-decayed effective games $G_{\text{eff}} = \sum_k e^{-\Delta t_k / 75\text{d}}$ (`g_eff < 0.15 => 0.0`) plus active volume mastery $0.0065 \ln(1 + G_{\text{eff}})$ only when $\widehat{WR}_{\text{eff}} \ge 0.50$.
- **Mandatory Red-Green TDD**: Observe a failing test (`RED`) before any production change (`GREEN`), and run both `--all-features` and `--no-default-features` before completion.

## Decision Gates

| Change Surface | Required Action |
|----------------|-----------------|
| Formula in `truesight-core::math` or `domain` | Add unit test in `crates/truesight-core/src/lib.rs` proving antisymmetry, $\sqrt{N}$ normalization, and `Projected Win%` parity. |
| Bracket scaling or `OPENDOTA_HERO_SNAPSHOT` | Verify all 8 medals (`Herald`..`Immortal`) update Meta/Counter/Synergy and enforce `\|P(Divine) - P(Immortal)\| < 0.03`. |
| `LocalCacheManager` / OpenDota sync | Preserve `META_TTL_SECS = 86_400` (24h) and `PLAYER_TTL_SECS = 3_600` (1h) with offline fallback to `OPENDOTA_HERO_SNAPSHOT`. |
| GUI (`gui.rs`) or CLI (`main.rs`) | Mutate state only via `AppSession`; use ASCII `[v]`/`[^]`/`[Enter]` glyphs to avoid font tofu boxes. |

## Execution Steps

1. Read `references/domain-invariants.md` and `docs/architecture.md` before altering any mathematical weight, role offset, or cache parser.
2. Write the focused test in the target crate and run `cargo test` to observe `RED`.
3. Implement the minimal fix inside the allowed crate boundary while keeping `#![forbid(unsafe_code)]`.
4. Run `cargo test --workspace --all-features` and `cargo test --workspace --no-default-features`.
5. If user-facing behavior changed, rebuild and install the release binary via `install -m 755 target/release/truesight-dota ~/.local/bin/truesight-dota`.

## Output Contract

Return:
- Crate(s) and files modified.
- Observed `RED` -> `GREEN` test results for `--all-features` and `--no-default-features`.
- Verification of the mathematical invariants affected (`Counter` antisymmetry, `Synergy` symmetry, `Comfort v3`, or `Projected Win%` parity).
- Rust toolchain metadata (`edition = "2024"`, `rust-version = "1.85"`, `selected_toolchain = "1.99.1"`).

## References

- `references/domain-invariants.md` — Canonical equations, 8-medal scaling factors, Comfort v3 rules, and cache TTL specifications.
- `../../docs/architecture.md` — Hexagonal architecture and crate boundary specifications.
- `../../docs/roadmap-and-tasks.md` — Work-unit execution matrix (`T1`–`T16`) and verification gates.
