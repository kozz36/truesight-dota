# Feature: truesight-dota-phase2
Status: planned
Branch: feat/truesight-dota-phase2

## Specs

- **P2-S1**: Maintain a strictly `50-50` faction-neutral draft engine (`mantengamoslo neutro`), preserving `P(Allies) + P(Enemies) = 1.0`.
- **P2-S2**: Detect whether `Allies` is `Radiant` or `Dire` automatically from Dota 2 GSI `"player": { "team_name": "radiant" | "dire" }` so Phase 2 vision capture maps the left 5 top-bar slots (Radiant) and right 5 top-bar slots (Dire) into `Side::Allies` and `Side::Enemies` (`distinguir inmediatamente si allies es dire o radiant es parte de v2 vision capture adapter`).
- **P2-S3**: Implement `VisionCaptureAdapter` (PipeWire / X11 top-bar portrait pHash matcher) + optional STRATZ GraphQL empirical `with` synergy adapter + 3-tier automatic Steam32 Account ID resolution (`1. GUI/CLI config override` -> `2. Dota 2 GSI player.steamid via steam64 - 76561197960265728` -> `3. Steam local loginusers.vdf MostRecent`) with per-account cache isolation (`~/.local/share/truesight-dota/cache/{account_id}/`).
- **P2-S4**: Complete Phase 2 open-source privacy/snapshot cleanup before transitioning the GitHub repository from private to public (`Apache-2.0`): compile `OPENDOTA_HERO_SNAPSHOT` with neutral personal stats (`personal_matches = 0`, `personal_wins = 0`, `comfort_decay = 0.0` => `+0.00%` default comfort) and remove `DEFAULT_PLAYER_ACCOUNT_ID` + tracked `data/player_127255741_*.json` files.

## Tasks

- [ ] **T17** (P2-S1, P2-S2) — Route: planned — Parse `"player.team_name"` (`radiant` | `dire`) in `BoundedGsiListener`, emit faction detection event in `DraftEvent`, and preserve 50–50 draft symmetry in `truesight-core`.
- [ ] **T18** (P2-S2, P2-S3) — Route: planned — Implement `VisionCaptureAdapter` in `truesight-adapters` (PipeWire/X11 1 Hz top-bar 10-slot portrait pHash recognition during `DOTA_GAMERULES_STATE_HERO_SELECTION` + Radiant/Dire mapping via GSI).
- [ ] **T19** (P2-S3) — Route: planned — Add optional `STRATZ_API_TOKEN` GraphQL empirical `with` synergy client and implement the 3-tier Steam32 resolver (`explicit GUI/CLI/config override` -> `GSI player.steamid` -> `Steam loginusers.vdf MostRecent`) storing caches under `~/.local/share/truesight-dota/cache/{account_id}/`.
- [ ] **T20** (P2-S4) — Route: planned — Neutralize `OPENDOTA_HERO_SNAPSHOT` personal comfort defaults (`0` matches/wins, `0.0` decay) and remove `DEFAULT_PLAYER_ACCOUNT_ID` + tracked `data/player_127255741_*.json` snapshots prior to switching the private GitHub repository to public (`Apache-2.0`).

## Log

- **L1**: "mantengamoslo neutro / entonces distinguir inmediatamente si allies e sdire o radiant es parte de v2 vision capture adapter? / ok entonces procedamos a publicar lo que tenemos actualmente como privado. dejemos las tareas para la fase 2 planificadas solamente sin ejecutar aun"
- **L2**: "es cierto, creo que no hemos definido como la app obtiene el id del usuario que esta ejecutando la app / actualiza la planificacion"
