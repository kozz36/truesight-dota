# Feature: truesight-dota-foundation
Status: completed
Branch: feat/truesight-dota-foundation

## Specs

- **S1**: Build a native Rust local application (`vamos rust nativo`) named `truesight-dota` at `/data/Projects/truesight-dota` following a scalable Hexagonal Architecture (`sigamos una arquitectura hexagonal escalable y consideremos el uso de mi skill ~/.pi/agent/skills/rust-architect/SKILL.md`).
- **S2**: Implement a real-time 5v5 draft analysis matrix (`un tabla 5v5 donde muestra los picks y en verde sus probabilidades positivas y de rojo las negativas, lo que resulta en un analisis matematico en vivo de las elecciones de heroe`).
- **S3**: Implement hero suggestions ranked from current draft picks by contrasting base meta hero winrates against match picks (`heroes sugeridos a partir de los picks asi tambien este supongo ya tiene el winrate de los heores meta que luego se contrastan contra los picks de la partida para entregar la mejor probabilidad`).
- **S4**: Follow the Phase 1 sequence (`esa secuencia me parece correcta`): mathematical engine + interactive quick-pick selector + local GSI state listener + STRATZ/OpenDota data adapter, with clean port boundaries for Phase 2 screen capture.
- **S5**: Create project architecture documentation in `docs/` and a project `README.md` (`creemos documentacion con la architecura del proyecto en docs/ tras terminar de definir o tambien creemos el readme`).

## Tasks

- [x] **T1** (S1, S2, S3) — Route: direct inline (small cohesive core unit, no Writer trigger) — Commit: `c05e077` — Create Cargo workspace, `truesight-core` domain, Logit/Bayesian 5v5 math engine, hero recommendation simulator, ports, and unit tests.
- [x] **T2** (S1, S4) — Route: direct inline (small cohesive adapter unit, no Writer trigger) — Commit: `7991cbd` — Create `truesight-adapters` with seeded meta dataset + JSON snapshot repository (`StatsRepository`), bounded Dota 2 GSI HTTP listener (`DraftEventSource`), and adapter tests.
- [x] **T3** (S1, S2, S3, S5) — Route: direct inline (small cohesive UI + docs unit, no Writer trigger) — Commit: `fd7a795` — Create `truesight-app` native UI, `README.md`, and `docs/architecture.md`.

## Log

- **L1**: "investiguemos si dota plus de curse forge overwolf expone como obtiene sus datos probabiliticos para sugerencias de partidas o calculos"
- **L2**: "quiero saber si tenemos todo lo necesario para recrear una app parecida a dota plus para mi privada por el momento"
- **L3**: "ok la idea es correcta 1. app nativa local seria lo mas liviano en rust para maximo rendimiento, que te parece? 2. esa secuencia me parece correcta para la presentacion de las probabilidades podemos ahcer algo similar a como lo presenta dota plus, un tabla 5v5 donde muestra los picks y en verde sus probabilidades positivas y de rojo las negativas, lo que resulta en un analisis matematico en vivo de las elecciones de heroe. o heroes sugeridos a partir de los picks asi tambien este supongo ya tiene el winrate de los heores meta que luego se contrastan contra los picks de la partida para entregar la mejor probabilidad"
- **L4**: "sigamos una arquitectura hexagonal escalable y consideremos el uso de mi skill ~/.pi/agent/skills/rust-architect/SKILL.md"
- **L5**: "autorizo la creacion que me sugieres gui o tui? yo estaria bien con una tui pero si quiesiera que mi app escale open source seria mejor ir por una gui yambien revisemos sis en git existe ya el nombre para elegir otro"
- **L6**: "vamos por tu recomendacion nombre puede ser truesight-dota creemos documentacion con la architecura del proyecto en docs/ tras terminar de definir o tambien creemos el readme"
