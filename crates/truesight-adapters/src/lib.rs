pub mod gsi;
pub mod meta_repo;
pub mod snapshot_data;
pub mod snapshot_pairs;

#[cfg(test)]
mod tests {
    use crate::gsi::{BoundedGsiListener, parse_gsi_json_events};
    use crate::meta_repo::SeededMetaRepository;
    use std::io::Write;
    use std::net::TcpStream;
    use std::thread;
    use std::time::Duration;
    use truesight_core::domain::{Bracket, DraftPick, DraftState, HeroId, Role, Side};
    use truesight_core::math::{ModelWeights, recommend_heroes};
    use truesight_core::ports::{DraftEvent, DraftEventSource, StatsRepository};

    #[test]
    fn seeded_meta_repository_loads_heroes_and_matchup_deltas() {
        let repo = SeededMetaRepository::new();
        let matrix = repo.load_matrix(Bracket::DivineImmortal).unwrap();

        // Anti-Mage (1) vs Medusa (94) should have strong positive counter delta
        let cell = matrix.cell(HeroId(1), HeroId(94));
        assert!(
            cell.counter_delta > 0.04,
            "Anti-Mage should counter Medusa: {}",
            cell.counter_delta
        );

        // Earthshaker (7) vs Phantom Lancer (12)
        let es_pl = matrix.cell(HeroId(7), HeroId(12));
        assert!(es_pl.counter_delta > 0.03);

        // Strict antisymmetry & symmetry across pairs
        let medusa_am = matrix.cell(HeroId(94), HeroId(1));
        assert!(
            (cell.counter_delta + medusa_am.counter_delta).abs() < 1e-6,
            "Counter matrix must be strictly antisymmetric: {} vs {}",
            cell.counter_delta,
            medusa_am.counter_delta
        );
        assert!(
            (cell.synergy_delta - medusa_am.synergy_delta).abs() < 1e-6,
            "Synergy matrix must be strictly symmetric: {} vs {}",
            cell.synergy_delta,
            medusa_am.synergy_delta
        );

        // Recommend Carry when enemy picked Medusa (94) and Storm Spirit (17)
        let mut draft = DraftState {
            target_role: Role::Carry,
            ..DraftState::default()
        };
        draft
            .set_enemy(
                0,
                Some(DraftPick {
                    hero_id: HeroId(94),
                    role: Some(Role::Carry),
                }),
            )
            .unwrap();
        draft
            .set_enemy(
                1,
                Some(DraftPick {
                    hero_id: HeroId(17),
                    role: Some(Role::Mid),
                }),
            )
            .unwrap();

        let recs = recommend_heroes(&draft, &matrix, &ModelWeights::default(), 10);
        // Verify role purity: Spirit Breaker (71), Leshrac (52), Mirana (9), Kunkka (23) must not appear in Pos 1 Carry
        for r in &recs {
            assert!(
                ![HeroId(71), HeroId(52), HeroId(9), HeroId(23)].contains(&r.hero_id),
                "Non-Pos1 hero {:?} leaked into Carry recommendations",
                r.hero_id
            );
        }

        // Verify exact projection identity: picking #1 recommendation into A1 yields exact Projected Win%
        let top_pick = &recs[0];
        let mut picked_draft = draft;
        picked_draft
            .set_ally(
                0,
                Some(DraftPick {
                    hero_id: top_pick.hero_id,
                    role: Some(Role::Carry),
                }),
            )
            .unwrap();
        let eval_after =
            truesight_core::math::evaluate_draft_5v5(&picked_draft, &matrix, &ModelWeights::default());
        assert!(
            (eval_after.ally_win_probability - top_pick.projected_win_probability).abs() < 1e-6,
            "Header win% ({}) must exactly match Projected Win% ({})",
            eval_after.ally_win_probability,
            top_pick.projected_win_probability
        );
        assert!(
            (eval_after.ally_counter_avg[0].unwrap() - top_pick.counter_delta_sum).abs() < 1e-6,
            "Row Avg Counter must exactly match vs Picked Enemies"
        );
        assert_eq!(
            matrix.heroes().count(),
            127,
            "All 127 official OpenDota heroes must be loaded"
        );
        // Verify arbitrary non-curated heroes (e.g. Slardar=28 vs Silencer=75, Enigma=33 vs Necrophos=36)
        // have populated non-zero empirical counter & synergy deltas from the 15,966-pair table.
        let slardar_vs_silencer = matrix.cell(HeroId(28), HeroId(75));
        assert!(
            slardar_vs_silencer.vs_matches > 0
                && slardar_vs_silencer.counter_delta.abs() > 0.0001,
            "Slardar vs Silencer must have empirical matchup data: {:?}",
            slardar_vs_silencer
        );
        let fv_with_enigma = matrix.cell(HeroId(41), HeroId(33));
        assert!(
            fv_with_enigma.with_matches > 0 && fv_with_enigma.synergy_delta.abs() > 0.0001,
            "Faceless Void + Enigma must have empirical synergy data: {:?}",
            fv_with_enigma
        );
    }

    #[test]
    fn gsi_parser_extracts_game_state_hero_and_custom_pick() {
        let sample = r#"{
            "map": {
                "game_state": "DOTA_GAMERULES_STATE_HERO_SELECTION",
                "clock_time": -45
            },
            "hero": {
                "name": "npc_dota_hero_antimage"
            },
            "truesight_pick": {
                "side": "enemy",
                "slot": 1,
                "hero_id": 17
            }
        }"#;

        let events = parse_gsi_json_events(sample);
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            DraftEvent::GameRulesStateChanged {
                state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
                game_time_secs: -45,
            }
        );
        assert_eq!(
            events[1],
            DraftEvent::LocalPlayerHeroDetected {
                hero_slug: "npc_dota_hero_antimage".into(),
            }
        );
        assert_eq!(
            events[2],
            DraftEvent::PickDetected {
                side: Side::Enemies,
                slot: 1,
                hero_id: HeroId(17),
                role: Some(Role::Mid),
            }
        );
    }

    #[test]
    fn bounded_gsi_listener_accepts_http_post_and_emits_event() {
        let mut listener = BoundedGsiListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr();

        let body = r#"{"map":{"game_state":"DOTA_GAMERULES_STATE_HERO_SELECTION","clock_time":-30}}"#;
        let req = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(req.as_bytes()).unwrap();
        stream.flush().unwrap();

        thread::sleep(Duration::from_millis(60));
        let events = listener.poll_events().unwrap();
        assert_eq!(
            events,
            vec![DraftEvent::GameRulesStateChanged {
                state_name: "DOTA_GAMERULES_STATE_HERO_SELECTION".into(),
                game_time_secs: -30,
            }]
        );
    }
}
