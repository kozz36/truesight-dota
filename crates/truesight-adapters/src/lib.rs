pub mod gsi;
pub mod meta_repo;

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
            cell.counter_delta > 0.08,
            "Anti-Mage should hard-counter Medusa: {}",
            cell.counter_delta
        );

        // Earthshaker (7) vs Phantom Lancer (12)
        let es_pl = matrix.cell(HeroId(7), HeroId(12));
        assert!(es_pl.counter_delta > 0.10);

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

        let recs = recommend_heroes(&draft, &matrix, &ModelWeights::default(), 3);
        assert_eq!(recs[0].hero_id, HeroId(1), "Anti-Mage must rank #1 vs Medusa + Storm");
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
