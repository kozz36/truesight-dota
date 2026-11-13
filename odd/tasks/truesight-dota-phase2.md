# Feature: truesight-dota-phase2
Status: planned
Branch: feat/truesight-dota-phase2

## Specs

- **P2-S1**: Maintain a strictly `50-50` faction-neutral draft engine (`mantengamoslo neutro`), preserving `P(Allies) + P(Enemies) = 1.0`.
- **P2-S2**: Detect whether `Allies` is `Radiant` or `Dire` automatically from Dota 2 GSI `"player": { "team_name": "radiant" | "dire" }` so Phase 2 vision capture maps the left 5 top-bar slots (Radiant) and right 5 top-bar slots (Dire) into `Side::Allies` and `Side::Enemies` (`distinguir inmediatamente si allies es dire o radiant es parte de v2 vision capture adapter`).
- **P2-S3**: Implement `VisionCaptureAdapter` (PipeWire / X11 top-bar portrait pHash matcher) + optional STRATZ GraphQL empirical `with` synergy adapter + configurable Steam32 player ID.
- **P2-S4**: Complete Phase 2 open-source privacy/snapshot cleanup before transitioning the GitHub repository from private to public (`Apache-2.0`).

## Tasks

- [ ] **T17** (P2-S1, P2-S2) — Route: planned — Parse `"player.team_name"` (`radiant` | `dire`) in `BoundedGsiListener`, emit faction detection event in `DraftEvent`, and preserve 50–50 draft symmetry in `truesight-core`.
- [ ] **T18** (P2-S2, P2-S3) — Route: planned — Implement `VisionCaptureAdapter` in `truesight-adapters` (PipeWire/X11 1 Hz top-bar 10-slot portrait pHash recognition during `DOTA_GAMERULES_STATE_HERO_SELECTION` + Radiant/Dire mapping via GSI).
- [ ] **T19** (P2-S3) — Route: planned — Add optional `STRATZ_API_TOKEN` GraphQL empirical `with` synergy client and user-configurable Steam32 Account ID in `LocalCacheManager` & GUI/CLI.
- [ ] **T20** (P2-S4) — Route: planned — Externalize/parameterize personal Steam32 snapshot fixtures prior to switching the private GitHub repository to public (`Apache-2.0`).

## Log

- **L1**: "mantengamoslo neutro / entonces distinguir inmediatamente si allies e sdire o radiant es parte de v2 vision capture adapter? / ok entonces procedamos a publicar lo que tenemos actualmente como privado. dejemos las tareas para la fase 2 planificadas solamente sin ejecutar aun"
