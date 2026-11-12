# Feature: truesight-dota-foundation
Status: completed
Branch: feat/truesight-dota-foundation

## Specs

- **S1**: Build a native Rust local application (`vamos rust nativo`) named `truesight-dota` at `/data/Projects/truesight-dota` following a scalable Hexagonal Architecture (`sigamos una arquitectura hexagonal escalable y consideremos el uso de mi skill ~/.pi/agent/skills/rust-architect/SKILL.md`).
- **S2**: Implement a real-time 5v5 draft analysis matrix (`un tabla 5v5 donde muestra los picks y en verde sus probabilidades positivas y de rojo las negativas, lo que resulta en un analisis matematico en vivo de las elecciones de heroe`).
- **S3**: Implement hero suggestions ranked from current draft picks by contrasting base meta hero winrates against match picks (`heroes sugeridos a partir de los picks asi tambien este supongo ya tiene el winrate de los heores meta que luego se contrastan contra los picks de la partida para entregar la mejor probabilidad`).
- **S4**: Follow the Phase 1 sequence (`esa secuencia me parece correcta`): mathematical engine + interactive quick-pick selector + local GSI state listener + STRATZ/OpenDota data adapter, with clean port boundaries for Phase 2 screen capture.
- **S5**: Create project architecture documentation in `docs/` and a project `README.md` (`creemos documentacion con la architecura del proyecto en docs/ tras terminar de definir o tambien creemos el readme`).
- **S6**: Synchronize the complete 127-hero catalog and real player comfort history (`127255741`) into a local snapshot adapter (`SeededMetaRepository`) and add a native `eframe`/`egui` desktop GUI alongside the CLI/TUI (`ok procede`).
- **S7**: Upgrade and pin the Rust toolchain to `1.99.1`, package a release desktop executable (`~/.local/bin/truesight-dota` + `~/.local/share/applications/truesight-dota.desktop`), and install the Dota 2 GSI integration config (`por que no usamos la ultima version 1.99.1? cual seria el sifuiente paso, deberiamos tener un ejecutable de escritorio` / `1. autorizo actualizacion 2. procedamos`).
- **S8**: Fix manual testing findings (`solo veo 0% en todos los calculos`, `los suggest deberia ser otra seccion`, `la eleccion de cada herore por slot no permite escoger todos lo heores de manera independiente. se tiene que pickear desde el panel de suggetsed pick`, `revisa la imagen para notar otras inconsistencias y corregirlo`): embed all 15,966 (`127x127`) empirical OpenDota matchup & synergy pairs, add independent 127-hero dropdown selectors per slot (`A1..A5`, `E1..E5`), move `Suggested Heroes` into its own dedicated bottom section, fix neutral `0.00%` coloring, and evaluate `Draft Gain` against the vacated target slot.

## Tasks

- [x] **T1** (S1, S2, S3) — Route: direct inline (small cohesive core unit, no Writer trigger) — Commit: `c05e077` — Create Cargo workspace, `truesight-core` domain, Logit/Bayesian 5v5 math engine, hero recommendation simulator, ports, and unit tests.
- [x] **T2** (S1, S4) — Route: direct inline (small cohesive adapter unit, no Writer trigger) — Commit: `7991cbd` — Create `truesight-adapters` with seeded meta dataset + JSON snapshot repository (`StatsRepository`), bounded Dota 2 GSI HTTP listener (`DraftEventSource`), and adapter tests.
- [x] **T3** (S1, S2, S3, S5) — Route: direct inline (small cohesive UI + docs unit, no Writer trigger) — Commit: `932c2f5` — Create `truesight-app` native UI, `README.md`, and `docs/architecture.md`.
- [x] **T4** (S4, S6) — Route: direct inline (cohesive adapter unit, no Writer trigger) — Commit: `878603f` — Synchronize the full 127-hero OpenDota dataset and player `127255741` comfort history into `truesight-adapters` (`SeededMetaRepository`) with bracket-aware winrates and unit tests.
- [x] **T5** (S1, S2, S3, S5, S6) — Route: direct inline (cohesive GUI + docs unit, no Writer trigger) — Commit: `cfd19ec` — Add native desktop `eframe`/`egui` GUI mode (`--gui`) in `truesight-app` sharing `AppSession` with CLI/TUI, and update documentation.
- [x] **T6** (S7) — Route: direct inline (cohesive release packaging + GSI hook unit, no Writer trigger) — Commit: `2799255` — Pin `rust-toolchain.toml` to `1.99.1`, make GUI the default binary entrypoint (`truesight-dota`), build `--release`, install `~/.local/bin/truesight-dota`, `~/.local/share/applications/truesight-dota.desktop`, and Dota 2 `gamestate_integration_truesight.cfg`.
- [x] **T7** (S8) — Route: direct inline (cohesive data + GUI layout + math baseline fix, no Writer trigger) — Commit: `c4e7853` — Embed all 15,966 `127x127` OpenDota matchup & synergy pairs, add independent per-slot 127-hero dropdown pickers, move Suggested Heroes to a dedicated bottom section, and fix neutral color thresholds + slot replacement baseline.
- [x] **T8** (S2, S3, S8) — Route: direct inline (statistical calibration fix, no Writer trigger) — Commit: `564f476` — Fix 96.5% win-probability inflation by computing cohort-consistent baseline winrates inside `/heroes/{id}/matchups`, applying $\sqrt{N}$ variance normalization across the 25 correlated 5v5 pairs, and averaging per-slot/suggestion deltas.

## Log

- **L1**: "investiguemos si dota plus de curse forge overwolf expone como obtiene sus datos probabiliticos para sugerencias de partidas o calculos"
- **L2**: "quiero saber si tenemos todo lo necesario para recrear una app parecida a dota plus para mi privada por el momento"
- **L3**: "ok la idea es correcta 1. app nativa local seria lo mas liviano en rust para maximo rendimiento, que te parece? 2. esa secuencia me parece correcta para la presentacion de las probabilidades podemos ahcer algo similar a como lo presenta dota plus, un tabla 5v5 donde muestra los picks y en verde sus probabilidades positivas y de rojo las negativas, lo que resulta en un analisis matematico en vivo de las elecciones de heroe. o heroes sugeridos a partir de los picks asi tambien este supongo ya tiene el winrate de los heores meta que luego se contrastan contra los picks de la partida para entregar la mejor probabilidad"
- **L4**: "sigamos una arquitectura hexagonal escalable y consideremos el uso de mi skill ~/.pi/agent/skills/rust-architect/SKILL.md"
- **L5**: "autorizo la creacion que me sugieres gui o tui? yo estaria bien con una tui pero si quiesiera que mi app escale open source seria mejor ir por una gui yambien revisemos sis en git existe ya el nombre para elegir otro"
- **L6**: "vamos por tu recomendacion nombre puede ser truesight-dota creemos documentacion con la architecura del proyecto en docs/ tras terminar de definir o tambien creemos el readme"
- **L7**: "ok procede"
- **L8**: "por que no usamos la ultima version 1.99.1? cual seria el sifuiente paso, deberiamos tener un ejecutable de escritorio"
- **L9**: "1. autorizo actualizacion 2. procedamos"
- **L10**: "acabo de probar manualmente y tengo varias dudas y veo errores - solo veo 0% en todos los calculos - los suggest deberia ser otra seccion - la eleccion de cada herore por slot no permite escoger todos lo heores de manera independiente. se tiene que pickear desde el panel de suggetsed pick = revisa la imagen para notar otras inconsistencias y corregirlo"
- **L11**: "me parece exagerado la probabilidad de ganar revisa si es correcto"
