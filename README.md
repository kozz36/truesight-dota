# TrueSight Dota (`truesight-dota`)

High-performance, native Rust 5v5 live draft analyzer and probabilistic hero recommender for Dota 2. Built on a strict **Hexagonal Architecture (Ports & Adapters)** with zero runtime bloat, empirical Bayes shrinkage, strictly antisymmetric $127 \times 127$ matchup matrices, 8-medal scaling, and automatic OpenDota cache synchronization.

## Quick Path

1. **Launch the Native Desktop GUI** (or from your CachyOS/KDE App Launcher as **TrueSight Dota**):
   ```bash
   truesight-dota
   ```
2. **Force an immediate database sync from OpenDota** (`heroStats` 24h TTL + linked Steam32 account 1h TTL):
   ```bash
   truesight-dota --sync
   # Optional explicit Steam32/Steam64 account override (or auto | none):
   truesight-dota --sync --account-id <steam32_or_steam64>
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
| **v4 Gem of Truesight Desktop HUD** | Native `eframe`/`egui` GUI embedding zero-dependency binary RGB atlases (`hero_atlas.bin`: Gem icon + 127 base `64x36` portraits; `hero_alt_atlas.bin`: 49 Arcana & Persona `64x36` portraits across 30 heroes), Allies/Oracle/Enemies breakdown cards (`Meta · Cnt · Syn = NET/THREAT`), `Suggested Picks` with `Comfort` meter, **Enemy Scout** strip, 3-tab `COUNTER` / `SYNERGY` / `COMP` 5v5 matrix, and a live `RADIANT`/`DIRE` faction toggle. |
| **8 Individual Dota 2 Medals** | Instant switching across `Herald`, `Guardian`, `Crusader`, `Archon`, `Legend`, `Ancient`, `Divine`, and `Immortal`, dynamically scaling meta winrates, counter sharpness ($\tau_b$), and pairwise synergies. |
| **Mathematical Invariants** | Strict counter antisymmetry ($\Delta C(A,B) = -\Delta C(B,A)$), zero-mean double-centered synergy ($\Delta S(A,B) = \Delta S(B,A)$), $\sqrt{N}$ variance normalization, and exact identity between `Projected Win%` and header `ALLIES WIN PROBABILITY`. |
| **Engine v2 & Comfort v3** | Incorporates **7-day patch momentum** (`pub_pick_trend`/`pub_win_trend`), **Personal Comfort v3** (per-match exponential recency decay $\tau = 75\text{d}$ over `/matches` + active volume mastery $\ln(1 + G_{\text{eff}})$), and **structural team composition balance** (`Melee`/`Ranged` ratio + `Disabler`/`Initiator` coverage). |
| **Auto-Sync & Account Linking** | 3-tier Steam32 resolver (`Explicit GUI/--account-id/config.toml` $\to$ `GSI player.steamid` $\to$ local Steam `loginusers.vdf`), isolating OpenDota player caches under `~/.local/share/truesight-dota/cache/players/{account_id}/` (`24h` Meta TTL, `1h` Player TTL) plus optional `STRATZ_API_TOKEN` GraphQL `with` synergy and a clickable **`[Sync DB]`** / Account modal in the GUI. |
| **Hands-Free Slot Picker & Vision Port** | Click any Ally (`A1..A5`) or Enemy (`E1..E5`) slot button once, type the hero name directly with auto-focus, and press `Enter` to assign. Includes `VisionCaptureAdapter` (`truesight-adapters::vision`) for 10-slot Radiant/Dire top-bar matching (base + Arcana/Persona skins) via X11/XWayland (`xprop` + ImageMagick `import`), phase-aware GSI suppression, and `player.team_name` faction routing. |

## Supported Platforms & Installers

Full per-OS requirements, dependencies, GSI setup, and verification steps live in [`docs/platforms-and-assets.md`](docs/platforms-and-assets.md).

- **Linux (`x86_64`)**: Release archive `truesight-dota-linux-x86_64.tar.gz` (binary, `gamestate_integration_truesight.cfg`, `LICENSE`, `README.md`). Live top-bar vision capture requires an X11/XWayland session with `xprop` and ImageMagick (`import`); manual pick entry and GSI remain active without them.
- **Windows (`x86_64`)**: Release packages `truesight-dota-windows-x86_64.msi` (per-user Start Menu installer) and `truesight-dota-windows-x86_64.zip`, both bundling `gamestate_integration_truesight.cfg` and `install-gsi.ps1`. Uses Win32/GDI top-bar capture (`xcap` — **Borderless Window** / **Windowed** Dota 2 recommended), built-in Windows `curl.exe` & `System32\tasklist.exe`, and `%APPDATA%`/`%LOCALAPPDATA%` storage. Windows binaries are currently **unsigned** (Windows SmartScreen warning expected; verify `SHA256SUMS`), preserve console I/O for `--cli`/`--demo`/`--sync`, and keep live in-game Dota 2 validation tracked in `#12`.
- **Native Wayland & Headless CLI**: Native Wayland capture is tracked in `#13` (`CAP: MANUAL` fallback today); headless CLI builds (`--no-default-features`) require zero GUI dependencies on both Linux and Windows.

## Interactive CLI Commands (`truesight-dota --cli`)

```text
ally <1..5> <hero>      # Assign hero to Ally slot A1..A5 (or 'none' to clear)
enemy <1..5> <hero>     # Assign hero to Enemy slot E1..E5 (or 'none' to clear)
ban <hero>              # Toggle hero ban
role <1..5>             # Set target recommendation role (Pos 1..5)
bracket <medal>         # Switch medal: herald | guardian | crusader | archon | legend | ancient | divine | immortal
faction <radiant|dire>  # Switch Allies team faction (draft math stays 50-50 neutral)
account <id|auto|none>  # Link explicit Steam32/Steam64 ID, switch to auto detection, or unlink
sync                    # Force live refresh from OpenDota API into ~/.local/share/truesight-dota/cache/
reset                   # Clear current draft
quit                    # Exit session
```

## Documentation & Governance

- [`AGENTS.md`](AGENTS.md) — Engineering, hexagonal architecture, statistical invariants, and Red-Green TDD testing policy.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — Local build prerequisites, TDD guidelines, privacy rules, and PR verification matrix.
- [`SECURITY.md`](SECURITY.md) — Security architecture (`#![forbid(unsafe_code)]`, loopback GSI bounds) and vulnerability reporting process.
- [`skills/truesight-dota-domain/SKILL.md`](skills/truesight-dota-domain/SKILL.md) — Product domain skill and invariant reference for AI agents and contributors.
- [`docs/architecture.md`](docs/architecture.md) — Hexagonal architecture, Engine v2 & Comfort v3 mathematical equations, and GSI / Vision / Account specifications.
- [`docs/platforms-and-assets.md`](docs/platforms-and-assets.md) — Supported OS matrix, Windows/Linux installers, `gamestate_integration_truesight.cfg` setup, and third-party asset provenance.
- [`docs/roadmap-and-tasks.md`](docs/roadmap-and-tasks.md) — Formal engineering task plan (`T1`–`T22`), TDD verification gates, and data sync schedule.
- [`.github/workflows/ci.yml`](.github/workflows/ci.yml), [`.github/workflows/release.yml`](.github/workflows/release.yml), and [`.github/dependabot.yml`](.github/dependabot.yml) — Automated dual-feature CI, `v*` binary release packaging, and weekly dependency updates.

## License & Third-Party Assets

- Source code in this repository is licensed under the [Apache License, Version 2.0](LICENSE) (`Apache-2.0`).
- **Third-Party Game Assets**: Embedded hero portrait textures and icons ([`hero_atlas.bin`](crates/truesight-adapters/assets/hero_atlas.bin): 127 base `64x36` portraits + Gem icon; [`hero_alt_atlas.bin`](crates/truesight-adapters/assets/hero_alt_atlas.bin): 49 VPK Arcana & Persona portraits across 30 heroes), the Gem artwork, and Dota 2 hero names/identifiers/GSI schema are the intellectual property of **Valve Corporation** and are **not** covered by the repository's `Apache-2.0` license. TrueSight Dota is an independent open-source project and is not affiliated with or endorsed by Valve Corporation (see [`docs/platforms-and-assets.md`](docs/platforms-and-assets.md)).
- **Third-Party Matchup Data**: Embedded and synced hero matchup/player statistics are sourced from the public **OpenDota** API (and optionally **STRATZ** GraphQL API when `STRATZ_API_TOKEN` is configured by the user).
