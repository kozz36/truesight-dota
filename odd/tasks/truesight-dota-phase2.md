# Feature: truesight-dota-phase2
Status: completed
Branch: feat/truesight-dota-phase2

## Specs

- **P2-S1**: Maintain a strictly `50-50` faction-neutral draft engine (`mantengamoslo neutro`), preserving `P(Allies) + P(Enemies) = 1.0`.
- **P2-S2**: Detect whether `Allies` is `Radiant` or `Dire` automatically from Dota 2 GSI `"player": { "team_name": "radiant" | "dire" }` so Phase 2 vision capture maps the left 5 top-bar slots (Radiant) and right 5 top-bar slots (Dire) into `Side::Allies` and `Side::Enemies` (`distinguir inmediatamente si allies es dire o radiant es parte de v2 vision capture adapter`).
- **P2-S3**: Implement `VisionCaptureAdapter` (PipeWire / X11 top-bar portrait pHash matcher) + optional STRATZ GraphQL empirical `with` synergy adapter + 3-tier automatic Steam32 Account ID resolution (`1. GUI/CLI config override` -> `2. Dota 2 GSI player.steamid via steam64 - 76561197960265728` -> `3. Steam local loginusers.vdf MostRecent`) with per-account cache isolation (`~/.local/share/truesight-dota/cache/players/{account_id}/`).
- **P2-S4**: Complete Phase 2 open-source privacy/snapshot cleanup before transitioning the GitHub repository from private to public (`Apache-2.0`): compile `OPENDOTA_HERO_SNAPSHOT` with neutral personal stats (`personal_matches = 0`, `personal_wins = 0`, `comfort_decay = 0.0` => `+0.00%` default comfort) and remove `DEFAULT_PLAYER_ACCOUNT_ID` + tracked `data/player_<account_id>_*.json` files.
- **P2-S5**: Add GitHub Actions CI & automated release pipeline (`.github/workflows/ci.yml` + `release.yml`) validating dual-feature test suites (`--all-features` and `--no-default-features`) on `ubuntu-latest` and publishing tagged Linux release artifacts with `SHA256SUMS`.
- **P2-S6**: Add open-source security & contribution governance (`SECURITY.md`, `CONTRIBUTING.md`, `.github/dependabot.yml`) and prepare GitHub security settings before transitioning visibility to `PUBLIC`.

## Tasks

- [x] **T17** (P2-S1, P2-S2) — Route: direct inline — Commit: `7083c69` — Parse `"player.team_name"` (`radiant` | `dire`) in `BoundedGsiListener`, emit faction detection event in `DraftEvent`, unify `Faction` in `truesight-core`, and preserve 50–50 draft symmetry in `truesight-core`.
- [x] **T18** (P2-S2, P2-S3) — Route: direct inline — Commit: `e84b4a2` — Implement `VisionCaptureAdapter` in `truesight-adapters::vision` using the shared embedded 127-hero RGB atlas (`crates/truesight-adapters/assets/hero_atlas.bin`) for 10-slot top-bar portrait recognition + Radiant/Dire mapping via `Faction`.
- [x] **T19** (P2-S3) — Route: direct inline — Commit: `f315c48` — Add optional STRATZ empirical `with` synergy overrides and implement the 3-tier Steam32 resolver (`explicit GUI/CLI/config override` -> `GSI player.steamid` -> `Steam loginusers.vdf MostRecent`) storing caches under `~/.local/share/truesight-dota/cache/players/{account_id}/`.
- [x] **T20** (P2-S4) — Route: direct inline — Commit: `f706bae` — Migrate local player files to per-account cache, neutralize `OPENDOTA_HERO_SNAPSHOT` personal comfort defaults (`0` matches/wins, `0.0` decay), remove `DEFAULT_PLAYER_ACCOUNT_ID` + tracked `data/player_<account_id>_*.json` snapshots, and scrub personal ID references from repository sources/docs.
- [x] **T21** (P2-S5) — Route: direct inline — Commit: `1537e10` — Create `.github/workflows/ci.yml` (dual-feature test matrix on `ubuntu-latest`) and `.github/workflows/release.yml` (tagged Linux binary + `SHA256SUMS`).
- [x] **T22** (P2-S6) — Route: direct inline — Commit: closing commit — Create `SECURITY.md`, `CONTRIBUTING.md`, `.github/dependabot.yml`, update docs (`architecture.md`, `roadmap-and-tasks.md`, `README.md`), verify dual-feature suite & release binary, and merge `feat/truesight-dota-phase2` into `main`.

## Log

- **L1**: "mantengamoslo neutro / entonces distinguir inmediatamente si allies e sdire o radiant es parte de v2 vision capture adapter? / ok entonces procedamos a publicar lo que tenemos actualmente como privado. dejemos las tareas para la fase 2 planificadas solamente sin ejecutar aun"
- **L2**: "es cierto, creo que no hemos definido como la app obtiene el id del usuario que esta ejecutando la app / actualiza la planificacion"
- **L3**: "autorizo y antes de la implementacion veamos primero lo del enlace de cuenta de usuario y como vamos a publicar para luego pasar nuestro repo a publico y poder crear su ci para usar los beneficios open source y demas configuraciones y seguridad en gh. con esto definiremos el alcance para crear o terminar de definir nuevas tareas" / "apruebo"
