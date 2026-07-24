use super::*;
use crate::game_state::{MoveRecord, board_from_dto, move_candidate_to_dto};
use api::{CreateSeatRequest, GameStateDto, SeatClaim, SeatKind};
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request};
use rules_shared::{
    Direction, ENABLE2K, GERMAN, GameState, Letter, MoveCandidate, MoveGenerator, Position, Rack,
    RulesEngine, SOWPODS, Tile, TilePlacement, VariantRules,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::Row;
use tower::util::ServiceExt;

#[test]
fn no_build_id_is_three_numbers_only() {
    assert_eq!(format_app_version("0.1.0", None), "0.1.0");
}

#[test]
fn empty_build_id_is_treated_as_absent() {
    assert_eq!(format_app_version("0.1.0", Some("")), "0.1.0");
}

#[test]
fn build_id_appends_as_semver_build_metadata() {
    assert_eq!(
        format_app_version("0.1.0", Some("a1c9f02")),
        "0.1.0+a1c9f02"
    );
}

async fn create_test_state(database_url: &str) -> AppState {
    AppState::new(
        database_url,
        "http://127.0.0.1:8080".to_string(),
        crate::email::EmailConfig::new(
            None,
            "Tile Lite Elite <noreply@mail.tileliteelite.com>".to_string(),
        ),
    )
    .await
    .expect("test app state should initialize")
}

fn test_database_url() -> String {
    let path = std::env::temp_dir().join(format!(
        "tile-lite-elite-server-test-{}.sqlite3",
        Uuid::new_v4()
    ));
    std::fs::File::create(&path).expect("test sqlite file should be created");
    format!("sqlite://{}", path.display())
}

async fn send_json<T: Serialize>(
    app: Router,
    method: Method,
    uri: &str,
    payload: &T,
) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(payload).expect("payload should serialize"),
            ))
            .expect("request should build"),
    )
    .await
    .expect("request should succeed")
}

async fn send_json_auth<T: Serialize>(
    app: Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    payload: &T,
) -> axum::http::Response<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    app.oneshot(
        builder
            .body(Body::from(
                serde_json::to_vec(payload).expect("payload should serialize"),
            ))
            .expect("request should build"),
    )
    .await
    .expect("request should succeed")
}

async fn send_empty(app: Router, method: Method, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await
    .expect("request should succeed")
}

async fn send_empty_auth(
    app: Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
) -> axum::http::Response<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    app.oneshot(builder.body(Body::empty()).expect("request should build"))
        .await
        .expect("request should succeed")
}

/// `oneshot()`-driven tests never go through a real TCP listener, so
/// `ConnectInfo` (what `require_loopback` reads) is never populated the
/// way it would be by `into_make_service_with_connect_info` in
/// production — it has to be injected into the request's extensions by
/// hand, exactly like axum's own connect-info middleware would.
fn loopback_peer() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 54321))
}

fn remote_peer() -> SocketAddr {
    SocketAddr::from(([203, 0, 113, 5], 54321))
}

async fn send_admin<T: Serialize>(
    app: Router,
    method: Method,
    uri: &str,
    peer: SocketAddr,
    payload: Option<&T>,
) -> axum::http::Response<Body> {
    let body = match payload {
        Some(payload) => Body::from(serde_json::to_vec(payload).expect("payload should serialize")),
        None => Body::empty(),
    };
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(body)
        .expect("request should build");
    request.extensions_mut().insert(ConnectInfo(peer));
    app.oneshot(request).await.expect("request should succeed")
}

async fn read_json<T: DeserializeOwned>(response: axum::http::Response<Body>) -> T {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    serde_json::from_slice(&bytes).expect("body should deserialize")
}

fn rack_with_letters(letters: &[char]) -> Rack {
    let mut rack = Rack::default();
    for letter in letters {
        rack.add_letter(Letter::from(*letter));
    }
    rack
}

#[tokio::test]
async fn dictionary_endpoint_serves_sowpods_unauthenticated() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_empty(app, Method::GET, "/dictionaries/sowpods").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let text = String::from_utf8(bytes.to_vec()).expect("dictionary should be valid utf8");
    assert!(text.split_whitespace().any(|word| word == "ACE"));
}

#[tokio::test]
async fn dictionary_endpoint_serves_enable2k_unauthenticated() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_empty(app, Method::GET, "/dictionaries/enable2k").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let text = String::from_utf8(bytes.to_vec()).expect("dictionary should be valid utf8");
    assert!(text.split_whitespace().any(|word| word == "ACE"));
}

#[tokio::test]
async fn dictionary_endpoint_serves_german_unauthenticated() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_empty(app, Method::GET, "/dictionaries/german").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let text = String::from_utf8(bytes.to_vec()).expect("dictionary should be valid utf8");
    assert!(text.split_whitespace().any(|word| word == "ÖL"));
}

#[tokio::test]
async fn dictionary_endpoint_serves_spanish_unauthenticated() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_empty(app, Method::GET, "/dictionaries/spanish").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let text = String::from_utf8(bytes.to_vec()).expect("dictionary should be valid utf8");
    assert!(text.split_whitespace().any(|word| word == "CARRO"));
}

#[tokio::test]
async fn dictionary_endpoint_404s_for_an_unknown_name() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_empty(app, Method::GET, "/dictionaries/not-a-real-dictionary").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_game_and_list_games_via_http() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let response = send_json_auth(
        app.clone(),
        Method::POST,
        "/games",
        Some(&alice.session_token),
        &CreateGameRequest {
            seats: vec![
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Alice".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Creator),
                },
                CreateSeatRequest {
                    kind: SeatKind::Engine,
                    display_name: "Greedy".to_string(),
                    engine_id: Some("greedy-v1".to_string()),
                    claim: None,
                },
            ],
            seed: Some(1234),
            variant: None,
            language: None,
            board_layout: None,
            move_time_limit_seconds: None,
        },
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let created: GameStateDto = read_json(response).await;
    assert_eq!(created.status, api::GameStatus::Waiting);
    assert_eq!(created.participants.len(), 2);

    let listed_response = send_empty_auth(
        app.clone(),
        Method::GET,
        "/games",
        Some(&alice.session_token),
    )
    .await;
    assert_eq!(listed_response.status(), StatusCode::OK);
    let listed: Vec<api::GameSummaryDto> = read_json(listed_response).await;
    let summary = listed
        .iter()
        .find(|summary| summary.id == created.id)
        .expect("created game should appear in the summary list");
    assert_eq!(summary.status, api::GameStatus::Waiting);
    assert_eq!(summary.participants.len(), 2);
    assert_eq!(summary.relationship, api::GameRelationship::Participant);
    assert!(
        summary.last_activity_at > 0,
        "expected a real timestamp, got {:?}",
        summary.last_activity_at
    );

    let fetched_response = send_empty_auth(
        app,
        Method::GET,
        &format!("/games/{}", created.id),
        Some(&alice.session_token),
    )
    .await;
    assert_eq!(fetched_response.status(), StatusCode::OK);
    let fetched: GameStateDto = read_json(fetched_response).await;
    assert_eq!(fetched.id, created.id);
}

#[tokio::test]
async fn games_created_without_a_seed_get_different_racks() {
    // Regression test: the default seed used to be a fixed constant, so
    // every game created without an explicit seed (i.e. every real game
    // through the UI, which never sends one) dealt the exact same racks
    // in the exact same order, every single time.
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let new_game = CreateGameRequest {
        seats: vec![
            CreateSeatRequest {
                kind: SeatKind::Human,
                display_name: "Alice".to_string(),
                engine_id: None,
                claim: Some(SeatClaim::Creator),
            },
            CreateSeatRequest {
                kind: SeatKind::Engine,
                display_name: "Greedy".to_string(),
                engine_id: Some("greedy-v1".to_string()),
                claim: None,
            },
        ],
        seed: None,
        variant: None,
        language: None,
        board_layout: None,
        move_time_limit_seconds: None,
    };

    let first: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &new_game,
        )
        .await,
    )
    .await;
    let second: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &new_game,
        )
        .await,
    )
    .await;

    let first_started: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/start", first.id),
            Some(&alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;
    let second_started: GameStateDto = read_json(
        send_json_auth(
            app,
            Method::POST,
            &format!("/games/{}/start", second.id),
            Some(&alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;

    assert_ne!(
        first_started.racks, second_started.racks,
        "two games created without an explicit seed dealt identical racks"
    );
}

#[tokio::test]
async fn human_move_endpoint_advances_state_and_triggers_engine_reply() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(77),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    let started_response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/start", created.id),
        Some(&alice.session_token),
        &StartGameRequest::default(),
    )
    .await;
    assert_eq!(started_response.status(), StatusCode::OK);
    let started: GameStateDto = read_json(started_response).await;
    assert_eq!(started.status, api::GameStatus::Active);

    {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&created.id)
            .expect("created game should exist in memory");
        // Leave plenty of filler tiles in the bag so neither seat's rack
        // can go fully empty after refilling (rack refills top up to
        // the full rack size, not just to the tile count played, so
        // this needs to comfortably outlast both Alice's and a
        // possible follow-up refill for Bob/Greedy). This test is about
        // the engine-reply plumbing, not the separate go-out endgame
        // path (see `human_going_out_with_empty_bag_finishes_game_with_rack_penalty`).
        game.bag = vec![rules_shared::Tile::Letter(Letter::from('X')); 20];
        game.participants[0].rack = rack_with_letters(&['A', 'T']);
        game.participants[1].rack = rack_with_letters(&['Q']);
    }

    let rules = VariantRules::official();
    let board =
        board_from_dto(&started.board, &rules.alphabet).expect("board dto should reconstruct");
    let position = GameState::from_board(board, &rules, &*SOWPODS);
    let player_rack = rack_with_letters(&['A', 'T']);
    let engine = RulesEngine {
        rules: &rules,
        dictionary: &*SOWPODS,
    };
    let candidate = engine
        .enumerate_legal_moves(&position, &player_rack)
        .next()
        .expect("opening rack should have a legal move");

    let move_response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", created.id),
        Some(&alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Place {
                candidate: move_candidate_to_dto(&candidate, &rules.alphabet),
            },
        },
    )
    .await;
    assert_eq!(move_response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(move_response).await;

    assert_eq!(updated.status, api::GameStatus::Active);
    assert_eq!(updated.current_seat, 0);
    assert_eq!(updated.moves.len(), 2);
    assert_eq!(updated.moves[0].seat_number, 0);
    assert_eq!(updated.moves[1].seat_number, 1);
    assert!(matches!(
        updated.moves[1].move_type.as_str(),
        "place" | "pass"
    ));
    assert!(updated.board.iter().any(|cell| cell.letter.is_some()));
}

#[tokio::test]
async fn placed_move_records_the_board_positions_it_used() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let created_id = started.game.id.clone();

    {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&created_id)
            .expect("created game should exist in memory");
        game.bag = vec![rules_shared::Tile::Letter(Letter::from('X')); 20];
        game.participants[0].rack = rack_with_letters(&['A', 'T']);
    }

    let rules = VariantRules::official();
    let board =
        board_from_dto(&started.game.board, &rules.alphabet).expect("board dto should reconstruct");
    let position = GameState::from_board(board, &rules, &*SOWPODS);
    let player_rack = rack_with_letters(&['A', 'T']);
    let engine = RulesEngine {
        rules: &rules,
        dictionary: &*SOWPODS,
    };
    let candidate = engine
        .enumerate_legal_moves(&position, &player_rack)
        .next()
        .expect("opening rack should have a legal move");

    // Derived independently from the same candidate, mirroring the
    // production offset math (`apply_place_move` in game_state.rs) —
    // this is what the response's positions should equal regardless
    // of which legal opening move the engine happened to pick.
    let expected_positions: Vec<api::PositionDto> = candidate
        .tiles
        .iter()
        .map(|placement| match candidate.direction {
            rules_shared::Direction::Horizontal => api::PositionDto {
                x: candidate.start.x + placement.offset,
                y: candidate.start.y,
            },
            rules_shared::Direction::Vertical => api::PositionDto {
                x: candidate.start.x,
                y: candidate.start.y + placement.offset,
            },
        })
        .collect();

    let move_response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", created_id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Place {
                candidate: move_candidate_to_dto(&candidate, &rules.alphabet),
            },
        },
    )
    .await;
    assert_eq!(move_response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(move_response).await;

    assert_eq!(updated.moves[0].move_type, "place");
    assert_eq!(updated.moves[0].positions.len(), expected_positions.len());
    for expected in &expected_positions {
        assert!(
            updated.moves[0].positions.contains(expected),
            "expected placed-tile position {expected:?} to be recorded on the move, got {:?}",
            updated.moves[0].positions
        );
    }
}

#[tokio::test]
async fn human_going_out_with_empty_bag_finishes_game_with_rack_penalty() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let created_id = started.game.id.clone();

    // Empty bag + Alice's rack holding exactly the tiles she's about to
    // play means she goes out this move: the game should end
    // immediately (no engine/Bob follow-up turn) with the standard
    // rack-penalty adjustment — Bob loses the value of his leftover
    // rack, Alice gains it.
    {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&created_id)
            .expect("created game should exist in memory");
        game.bag.clear();
        game.participants[0].rack = rack_with_letters(&['A', 'T']);
        game.participants[1].rack = rack_with_letters(&['Q']);
    }

    let rules = VariantRules::official();
    let board =
        board_from_dto(&started.game.board, &rules.alphabet).expect("board dto should reconstruct");
    let position = GameState::from_board(board, &rules, &*SOWPODS);
    let player_rack = rack_with_letters(&['A', 'T']);
    let engine = RulesEngine {
        rules: &rules,
        dictionary: &*SOWPODS,
    };
    let candidate = engine
        .enumerate_legal_moves(&position, &player_rack)
        .next()
        .expect("opening rack should have a legal move");

    let move_response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", created_id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Place {
                candidate: move_candidate_to_dto(&candidate, &rules.alphabet),
            },
        },
    )
    .await;
    assert_eq!(move_response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(move_response).await;

    assert_eq!(updated.status, api::GameStatus::Finished);
    assert_eq!(updated.winner_seat, Some(0));
    assert_eq!(
        updated.moves.len(),
        1,
        "game should end before any follow-up turn is taken"
    );

    let move_score = updated.moves[0].score_delta;
    let alice = updated
        .participants
        .iter()
        .find(|participant| participant.seat_number == 0)
        .expect("Alice's seat should exist");
    let bob = updated
        .participants
        .iter()
        .find(|participant| participant.seat_number == 1)
        .expect("Bob's seat should exist");
    // Bob's leftover rack is a single 'Q' (value 10 in the official
    // distribution): he loses it, Alice (who went out) gains it.
    assert_eq!(bob.score, -10);
    assert_eq!(alice.score, move_score + 10);
    assert_eq!(updated.final_bonus_seat, Some(0));
    assert_eq!(updated.final_bonus_points, Some(10));
}

#[tokio::test]
async fn persisted_games_reload_into_new_app_state() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Alice".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Creator),
                }],
                seed: Some(999),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    let _started: GameStateDto = read_json(
        send_json_auth(
            app,
            Method::POST,
            &format!("/games/{}/start", created.id),
            Some(&alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;

    let reloaded = create_test_state(&database_url).await;
    let games = reloaded.games.read().await;
    let restored = games
        .get(&created.id)
        .expect("game should reload from sqlite snapshot");
    assert_eq!(restored.id, created.id);
    assert_eq!(restored.status, api::GameStatus::Active);
    assert_eq!(restored.participants.len(), 1);
}

#[tokio::test]
async fn creating_a_game_with_an_unknown_variant_is_rejected() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let response = send_json_auth(
        app,
        Method::POST,
        "/games",
        Some(&alice.session_token),
        &CreateGameRequest {
            seats: vec![CreateSeatRequest {
                kind: SeatKind::Human,
                display_name: "Alice".to_string(),
                engine_id: None,
                claim: Some(SeatClaim::Creator),
            }],
            seed: Some(1),
            variant: Some("not-a-real-variant".to_string()),
            language: None,
            board_layout: None,
            move_time_limit_seconds: None,
        },
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// The whole point of the edition registry: the exact same move, played
/// under two different bundled rulesets, must score differently and
/// must be persisted/reloaded under the ruleset it was actually created
/// with — never silently falling back to official.
#[tokio::test]
async fn wordfeud_game_scores_the_same_move_differently_and_persists_its_own_rules() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;

    async fn create_and_start(
        app: Router,
        token: &str,
        variant: Option<String>,
        seed: u64,
    ) -> GameStateDto {
        let created: GameStateDto = read_json(
            send_json_auth(
                app.clone(),
                Method::POST,
                "/games",
                Some(token),
                &CreateGameRequest {
                    seats: vec![
                        CreateSeatRequest {
                            kind: SeatKind::Human,
                            display_name: "Alice".to_string(),
                            engine_id: None,
                            claim: Some(SeatClaim::Creator),
                        },
                        CreateSeatRequest {
                            kind: SeatKind::Engine,
                            display_name: "Greedy".to_string(),
                            engine_id: Some("greedy-v1".to_string()),
                            claim: None,
                        },
                    ],
                    seed: Some(seed),
                    variant,
                    language: None,
                    board_layout: None,
                    move_time_limit_seconds: None,
                },
            )
            .await,
        )
        .await;
        read_json(
            send_json_auth(
                app,
                Method::POST,
                &format!("/games/{}/start", created.id),
                Some(token),
                &StartGameRequest::default(),
            )
            .await,
        )
        .await
    }

    let official_game = create_and_start(app.clone(), &alice.session_token, None, 501).await;
    let wordfeud_game = create_and_start(
        app.clone(),
        &alice.session_token,
        Some("wordfeud".to_string()),
        502,
    )
    .await;
    assert_eq!(wordfeud_game.variant, "wordfeud");
    assert_eq!(wordfeud_game.board_layout, "wordfeud");
    assert_eq!(wordfeud_game.language, "sowpods");

    // Letter values for B/A/G genuinely differ between the two
    // rulesets (see `VariantRules::official`/`wordfeud`), and the
    // center square is a double-word premium in official but a plain
    // square in Wordfeud's layout — so "BAG" played through the center
    // on an otherwise-empty board scores differently under each,
    // purely from the rules, with everything else held constant.
    let official_rules = VariantRules::official();
    let wordfeud_rules = VariantRules::wordfeud();
    // Premiums live on the board itself, not on `RulesEngine.rules` — so
    // computing an "expected wordfeud score" needs the wordfeud game's
    // own board (with wordfeud's premium layout), not the official
    // game's, even though both are still empty at this point.
    let official_board = board_from_dto(&official_game.board, &official_rules.alphabet)
        .expect("fresh board should parse");
    let wordfeud_board = board_from_dto(&wordfeud_game.board, &wordfeud_rules.alphabet)
        .expect("fresh board should parse");
    let official_position = GameState::from_board(official_board, &official_rules, &*SOWPODS);
    let wordfeud_position = GameState::from_board(wordfeud_board, &wordfeud_rules, &*SOWPODS);
    let rack = rack_with_letters(&['B', 'A', 'G']);
    // Enumeration only depends on board geometry/dictionary/rack (not
    // premiums or letter values), so the same candidate is valid and
    // identical under either ruleset — reusing it is what makes this
    // an apples-to-apples comparison of the rules, not of two
    // different words.
    let official_engine = RulesEngine {
        rules: &official_rules,
        dictionary: &*SOWPODS,
    };
    let candidate = official_engine
        .enumerate_legal_moves(&official_position, &rack)
        .next()
        .expect("B/A/G should have a legal opening move");
    let expected_official_score = official_engine
        .validate_game_move(&official_position, Some(&rack), &candidate)
        .expect("candidate should be legal under official rules")
        .score
        .total;
    let wordfeud_engine = RulesEngine {
        rules: &wordfeud_rules,
        dictionary: &*SOWPODS,
    };
    let expected_wordfeud_score = wordfeud_engine
        .validate_game_move(&wordfeud_position, Some(&rack), &candidate)
        .expect("the same candidate should also be legal under wordfeud rules")
        .score
        .total;
    assert_ne!(
        expected_official_score, expected_wordfeud_score,
        "test setup should pick a move whose score actually differs between rulesets"
    );

    // Force both seats' racks to the exact known letters (bypassing the
    // random deal) so the same candidate is legal to actually submit,
    // same technique `human_move_endpoint_advances_state_and_triggers_engine_reply`
    // uses.
    for game_id in [&official_game.id, &wordfeud_game.id] {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(game_id)
            .expect("created game should exist in memory");
        game.bag = vec![rules_shared::Tile::Letter(Letter::from('X')); 20];
        game.participants[0].rack = rack;
        game.participants[1].rack = rack_with_letters(&['Q']);
    }

    let candidate_dto = move_candidate_to_dto(&candidate, &official_rules.alphabet);
    let official_response: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/actions", official_game.id),
            Some(&alice.session_token),
            &GameActionRequest {
                seat_number: 0,
                action: PlayerActionDto::Place {
                    candidate: candidate_dto.clone(),
                },
            },
        )
        .await,
    )
    .await;
    let wordfeud_response: GameStateDto = read_json(
        send_json_auth(
            app,
            Method::POST,
            &format!("/games/{}/actions", wordfeud_game.id),
            Some(&alice.session_token),
            &GameActionRequest {
                seat_number: 0,
                action: PlayerActionDto::Place {
                    candidate: candidate_dto,
                },
            },
        )
        .await,
    )
    .await;

    assert_eq!(
        official_response.participants[0].score,
        expected_official_score as i32
    );
    assert_eq!(
        wordfeud_response.participants[0].score,
        expected_wordfeud_score as i32
    );

    // Persistence round-trip: a fresh AppState reading the same DB must
    // reconstruct the wordfeud game with wordfeud's rules, not silently
    // default back to official.
    let reloaded = create_test_state(&database_url).await;
    let games = reloaded.games.read().await;
    let restored = games
        .get(&wordfeud_game.id)
        .expect("wordfeud game should reload from its sqlite snapshot");
    assert_eq!(restored.variant, "wordfeud");
    assert_eq!(restored.rules.bingo_bonus, wordfeud_rules.bingo_bonus);
    assert_eq!(restored.rules.letter_values, wordfeud_rules.letter_values);
}

/// The North American edition's whole point is a second real
/// dictionary (ENABLE2K, not SOWPODS) behind the exact same choke
/// point (`dictionary_by_name`) every other call site now goes
/// through — this exercises all three of those call sites end to end:
/// human move validation (`apply_place_move`), the engine's reply
/// (`GreedyEngine::choose_action`), and reloading from persistence.
#[tokio::test]
async fn north_american_game_plays_a_move_and_persists_its_own_dictionary() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(88),
                variant: Some("north_american".to_string()),
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert_eq!(created.variant, "north_american");
    assert_eq!(created.language, "enable2k");

    let started: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/start", created.id),
            Some(&alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;

    {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&created.id)
            .expect("created game should exist in memory");
        game.bag = vec![rules_shared::Tile::Letter(Letter::from('X')); 20];
        game.participants[0].rack = rack_with_letters(&['A', 'T']);
        game.participants[1].rack = rack_with_letters(&['Q']);
    }

    let rules = VariantRules::north_american();
    let board =
        board_from_dto(&started.board, &rules.alphabet).expect("board dto should reconstruct");
    let position = GameState::from_board(board, &rules, &*ENABLE2K);
    let player_rack = rack_with_letters(&['A', 'T']);
    let engine = RulesEngine {
        rules: &rules,
        dictionary: &*ENABLE2K,
    };
    let candidate = engine
        .enumerate_legal_moves(&position, &player_rack)
        .next()
        .expect("opening rack should have a legal move");

    let move_response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", created.id),
        Some(&alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Place {
                candidate: move_candidate_to_dto(&candidate, &rules.alphabet),
            },
        },
    )
    .await;
    assert_eq!(move_response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(move_response).await;
    // Confirms `apply_place_move` validated against ENABLE2K (not
    // SOWPODS) without panicking, and the greedy engine's reply
    // (`GreedyEngine::choose_action`) resolved its own dictionary the
    // same way.
    assert_eq!(updated.moves.len(), 2);
    assert_eq!(updated.moves[0].seat_number, 0);
    assert_eq!(updated.moves[1].seat_number, 1);

    let reloaded = create_test_state(&database_url).await;
    let games = reloaded.games.read().await;
    let restored = games
        .get(&created.id)
        .expect("north_american game should reload from its sqlite snapshot");
    assert_eq!(restored.variant, "north_american");
    assert_eq!(restored.language, "enable2k");
}

/// German is the real proof of the widened wire/persistence formats
/// (`RackDto.counts`, `PersistedVariantRules`) — unlike north_american
/// (still plain ASCII), a German rack genuinely needs a rack slot past
/// index 25 (Ö is index 28 of its 29-letter alphabet), so this only
/// passes if the whole chain (rack transport, move validation,
/// persistence round-trip) is actually alphabet-width-correct rather
/// than coincidentally working for a 26-or-fewer-letter edition.
#[tokio::test]
async fn german_game_plays_a_move_with_an_umlaut_tile_and_persists_it() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(99),
                variant: Some("german".to_string()),
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert_eq!(created.variant, "german");
    assert_eq!(created.language, "german");

    let started: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/start", created.id),
            Some(&alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;

    let rules = VariantRules::german();
    let umlaut_rack = {
        let mut rack = Rack::default();
        for ch in ['Ö', 'L'] {
            rack.add_letter(
                rules
                    .alphabet
                    .to_letter(&ch.to_string())
                    .expect("Ö/L should be in the German alphabet"),
            );
        }
        rack
    };
    {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&created.id)
            .expect("created game should exist in memory");
        game.bag = vec![rules_shared::Tile::Letter(Letter::from('X')); 20];
        game.participants[0].rack = umlaut_rack;
        game.participants[1].rack = umlaut_rack;
    }

    let board =
        board_from_dto(&started.board, &rules.alphabet).expect("board dto should reconstruct");
    let position = GameState::from_board(board, &rules, &*GERMAN);
    let engine = RulesEngine {
        rules: &rules,
        dictionary: &*GERMAN,
    };
    let candidate = engine
        .enumerate_legal_moves(&position, &umlaut_rack)
        .next()
        .expect("ÖL should have a legal opening move");

    let move_response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", created.id),
        Some(&alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Place {
                candidate: move_candidate_to_dto(&candidate, &rules.alphabet),
            },
        },
    )
    .await;
    assert_eq!(move_response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(move_response).await;
    // Confirms the wire round-trip: the placed Ö actually made it onto
    // the board through `RackDto`/`TileDto` (both now wider than 26)
    // without corruption.
    assert!(
        updated
            .board
            .iter()
            .any(|cell| cell.letter.as_deref() == Some("Ö"))
    );

    let reloaded = create_test_state(&database_url).await;
    let games = reloaded.games.read().await;
    let restored = games
        .get(&created.id)
        .expect("german game should reload from its sqlite snapshot");
    assert_eq!(restored.variant, "german");
    assert_eq!(restored.language, "german");
    assert_eq!(restored.rules.alphabet, rules.alphabet);
    assert_eq!(restored.rules.letter_values, rules.letter_values);
}

async fn create_and_start_spanish_game(
    state: &AppState,
    app: Router,
) -> (PlayerSessionDto, GameStateDto) {
    let alice = register_player(app.clone(), "Alice").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(100),
                variant: Some("spanish".to_string()),
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert_eq!(created.variant, "spanish");
    assert_eq!(created.language, "spanish");

    let started: GameStateDto = read_json(
        send_json_auth(
            app,
            Method::POST,
            &format!("/games/{}/start", created.id),
            Some(&alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;
    {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&created.id)
            .expect("created game should exist in memory");
        game.bag = vec![Tile::Letter(Letter::from('X')); 20];
    }
    (alice, started)
}

/// Spanish is the real proof of the whole digraph-tile design: the CH/
/// LL/RR tiles are genuinely distinct rack/bag objects (their own
/// scarcity, their own point value), but the dictionary needs no
/// annotation because both tilings of a word are accepted. This test
/// covers the digraph-tile spelling — one RR tile, one square.
#[tokio::test]
async fn spanish_game_plays_carro_with_the_rr_digraph_tile() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());
    let (alice, started) = create_and_start_spanish_game(&state, app.clone()).await;

    let rules = VariantRules::spanish();
    let spanish_letter = |s: &str| rules.alphabet.to_letter(s).expect("real Spanish tile");
    let carro_via_digraph = MoveCandidate {
        start: Position::new(7, 7),
        direction: Direction::Horizontal,
        tiles: vec![
            TilePlacement {
                offset: 0,
                tile: Tile::Letter(spanish_letter("C")),
            },
            TilePlacement {
                offset: 1,
                tile: Tile::Letter(spanish_letter("A")),
            },
            TilePlacement {
                offset: 2,
                tile: Tile::Letter(spanish_letter("RR")),
            },
            TilePlacement {
                offset: 3,
                tile: Tile::Letter(spanish_letter("O")),
            },
        ],
    };

    {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&started.id)
            .expect("created game should exist in memory");
        let mut rack = Rack::default();
        for letter in ["C", "A", "RR", "O"] {
            rack.add_letter(spanish_letter(letter));
        }
        game.participants[0].rack = rack;
    }

    let move_response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", started.id),
        Some(&alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Place {
                candidate: move_candidate_to_dto(&carro_via_digraph, &rules.alphabet),
            },
        },
    )
    .await;
    assert_eq!(move_response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(move_response).await;
    // "CARRO" via the digraph tile occupies exactly 4 squares — a
    // literal "R" cell should not appear at all, only the fused "RR".
    let placed: Vec<Option<String>> = updated.board[7 * 15 + 7..7 * 15 + 11]
        .iter()
        .map(|cell| cell.letter.clone())
        .collect();
    assert_eq!(
        placed,
        vec![
            Some("C".to_string()),
            Some("A".to_string()),
            Some("RR".to_string()),
            Some("O".to_string()),
        ]
    );
    // RR is worth 8, not 2× R's value (1) — confirms it scored as the
    // real digraph tile, not as two ordinary letters. C=3, A=1, O=1;
    // the center square (x=7) is a DoubleWord premium and nothing
    // else in this 4-square span (x=7..10) is a premium, so the raw
    // sum (3+1+8+1=13) just doubles.
    assert_eq!(updated.participants[0].score, (3 + 1 + 8 + 1) * 2);
}

/// The other half of the same proof: two *ordinary* R tiles (no RR
/// tile at all) spelling the same word is also accepted, occupying 5
/// squares instead of 4.
#[tokio::test]
async fn spanish_game_plays_carro_with_two_ordinary_r_tiles() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());
    let (alice, started) = create_and_start_spanish_game(&state, app.clone()).await;

    let rules = VariantRules::spanish();
    let spanish_letter = |s: &str| rules.alphabet.to_letter(s).expect("real Spanish tile");
    let carro_via_ordinary_tiles = MoveCandidate {
        start: Position::new(7, 7),
        direction: Direction::Horizontal,
        tiles: vec![
            TilePlacement {
                offset: 0,
                tile: Tile::Letter(spanish_letter("C")),
            },
            TilePlacement {
                offset: 1,
                tile: Tile::Letter(spanish_letter("A")),
            },
            TilePlacement {
                offset: 2,
                tile: Tile::Letter(spanish_letter("R")),
            },
            TilePlacement {
                offset: 3,
                tile: Tile::Letter(spanish_letter("R")),
            },
            TilePlacement {
                offset: 4,
                tile: Tile::Letter(spanish_letter("O")),
            },
        ],
    };

    {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&started.id)
            .expect("created game should exist in memory");
        let mut rack = Rack::default();
        for letter in ["C", "A", "R", "R", "O"] {
            rack.add_letter(spanish_letter(letter));
        }
        game.participants[0].rack = rack;
    }

    let move_response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", started.id),
        Some(&alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Place {
                candidate: move_candidate_to_dto(&carro_via_ordinary_tiles, &rules.alphabet),
            },
        },
    )
    .await;
    assert_eq!(move_response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(move_response).await;
    let placed: Vec<Option<String>> = updated.board[7 * 15 + 7..7 * 15 + 12]
        .iter()
        .map(|cell| cell.letter.clone())
        .collect();
    assert_eq!(
        placed,
        vec![
            Some("C".to_string()),
            Some("A".to_string()),
            Some("R".to_string()),
            Some("R".to_string()),
            Some("O".to_string()),
        ]
    );
    // Two ordinary R tiles score 1 point each (2 total), not RR's 8 —
    // confirms this really did use two separate letter tiles. This
    // 5-square span (x=7..11) also reaches a DoubleLetter premium at
    // x=11 (the O), on top of the center DoubleWord: (3+1+1+1+1×2)*2.
    assert_eq!(updated.participants[0].score, (3 + 1 + 1 + 1 + 2) * 2);
}

async fn register_player(app: Router, display_name: &str) -> PlayerSessionDto {
    read_json(
        send_json(
            app,
            Method::POST,
            "/auth/register",
            &RegisterPlayerRequest {
                display_name: display_name.to_string(),
                email: format!("{}@example.com", display_name.to_lowercase()),
                password: "correct horse battery staple".to_string(),
                stay_logged_in: false,
            },
        )
        .await,
    )
    .await
}

/// The result of building a real 2-human game end to end through the
/// invitation flow: Alice creates it (claiming seat 0 and leaving seat 1
/// open to strangers), Bob discovers and accepts the open seat, then
/// either of them starts it. Both sessions are returned since, under the
/// per-seat ownership model, every action from here on needs the
/// matching seat owner's token.
struct TwoHumanGame {
    game: GameStateDto,
    alice: PlayerSessionDto,
    bob: PlayerSessionDto,
}

/// Everything `create_two_human_game` does, minus the final `/start` —
/// both seats claimed, game still `Waiting`. Its own fixture since a
/// couple of tests (seat reordering) specifically need that
/// pre-start window.
async fn create_two_human_game_waiting(app: Router) -> TwoHumanGame {
    let alice = register_player(app.clone(), "Alice").await;
    let bob = register_player(app.clone(), "Bob").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Open seat".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Open),
                    },
                ],
                seed: Some(42),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    // Bob discovers the open seat via his personalized games list, then
    // accepts it — exercising the exact path a real client would use,
    // not a shortcut into persistence.
    let bob_games: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(app.clone(), Method::GET, "/games", Some(&bob.session_token)).await,
    )
    .await;
    let invitation = bob_games
        .iter()
        .find(|summary| summary.id == created.id)
        .expect("the open seat should appear in Bob's games list")
        .invitation_id
        .clone()
        .expect("an invited-open entry should carry an invitation id");

    let accept_response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/invitations/{invitation}/accept"),
        Some(&bob.session_token),
    )
    .await;
    assert_eq!(accept_response.status(), StatusCode::OK);

    TwoHumanGame {
        game: created,
        alice,
        bob,
    }
}

async fn create_two_human_game(app: Router) -> TwoHumanGame {
    let waiting = create_two_human_game_waiting(app.clone()).await;
    let started: GameStateDto = read_json(
        send_json_auth(
            app,
            Method::POST,
            &format!("/games/{}/start", waiting.game.id),
            Some(&waiting.alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;
    assert_eq!(started.status, api::GameStatus::Active);
    TwoHumanGame {
        game: started,
        alice: waiting.alice,
        bob: waiting.bob,
    }
}

struct ThreeHumanGame {
    game: GameStateDto,
    alice: PlayerSessionDto,
    bob: PlayerSessionDto,
    carol: PlayerSessionDto,
}

/// Same shape as `create_two_human_game`, one more seat — used by the
/// multi-player continuation tests, which specifically need at least 3
/// active seats to observe "one resigns, the other two keep playing"
/// (with only 2 seats, any single exit always ends the game).
async fn create_three_human_game(app: Router) -> ThreeHumanGame {
    let alice = register_player(app.clone(), "Alice3").await;
    let bob = register_player(app.clone(), "Bob3").await;
    let carol = register_player(app.clone(), "Carol3").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice3".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Bob3".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Named {
                            display_name: "Bob3".to_string(),
                        }),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Carol3".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Named {
                            display_name: "Carol3".to_string(),
                        }),
                    },
                ],
                seed: Some(42),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    for player in [&bob, &carol] {
        let games: Vec<api::GameSummaryDto> = read_json(
            send_empty_auth(
                app.clone(),
                Method::GET,
                "/games",
                Some(&player.session_token),
            )
            .await,
        )
        .await;
        let invitation_id = games
            .iter()
            .find(|summary| summary.id == created.id)
            .expect("the named invitation should appear in this player's games list")
            .invitation_id
            .clone()
            .expect("a named-invited entry should carry an invitation id");
        let accept_response = send_empty_auth(
            app.clone(),
            Method::POST,
            &format!("/invitations/{invitation_id}/accept"),
            Some(&player.session_token),
        )
        .await;
        assert_eq!(accept_response.status(), StatusCode::OK);
    }

    let started: GameStateDto = read_json(
        send_json_auth(
            app,
            Method::POST,
            &format!("/games/{}/start", created.id),
            Some(&alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;
    assert_eq!(started.status, api::GameStatus::Active);
    ThreeHumanGame {
        game: started,
        alice,
        bob,
        carol,
    }
}

#[tokio::test]
async fn a_resignation_in_a_three_player_game_only_removes_that_seat() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_three_human_game(app.clone()).await;
    assert_eq!(started.game.current_seat, 0, "Alice3 (seat 0) goes first");

    // Alice passes so it becomes Bob's turn.
    let response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Pass,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let after_alice: GameStateDto = read_json(response).await;
    assert_eq!(after_alice.current_seat, 1);

    // Bob resigns instead of taking his turn.
    let response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.bob.session_token),
        &GameActionRequest {
            seat_number: 1,
            action: PlayerActionDto::Resign,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let after_bob: GameStateDto = read_json(response).await;
    assert_eq!(
        after_bob.status,
        api::GameStatus::Active,
        "2 active seats remain (Alice, Carol) — the game must not end"
    );
    assert_eq!(
        after_bob.current_seat, 2,
        "turn should skip straight to Carol (seat 2), not wrap back to resigned Bob"
    );

    // Carol takes a real turn — confirms she genuinely can still act.
    let response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.carol.session_token),
        &GameActionRequest {
            seat_number: 2,
            action: PlayerActionDto::Pass,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let after_carol: GameStateDto = read_json(response).await;
    assert_eq!(after_carol.status, api::GameStatus::Active);
    assert_eq!(
        after_carol.current_seat, 0,
        "turn wraps back to Alice, skipping resigned Bob again"
    );
}

#[tokio::test]
async fn six_player_game_ranks_finishers_by_score_then_resigners_by_recency_and_excludes_force_resigned_and_timed_out()
 {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    // A: resigns first. B: resigns second (should outrank A). C:
    // force-resigned (excluded). D: times out (excluded). E: finishes
    // with 100. F: finishes with 200 (should outrank E). Built via the
    // real API, then only the exits themselves are poked directly
    // (score/resigned/move history) — same convention already used by
    // `voluntary_resignation_moves_rating_via_forfeit_ranking_even_when_ahead_on_score`.
    let alice = register_player(app.clone(), "A6").await;
    let bob = register_player(app.clone(), "B6").await;
    let carol = register_player(app.clone(), "C6").await;
    let dave = register_player(app.clone(), "D6").await;
    let eve = register_player(app.clone(), "E6").await;
    let frank = register_player(app.clone(), "F6").await;

    let named = |name: &str| CreateSeatRequest {
        kind: SeatKind::Human,
        display_name: name.to_string(),
        engine_id: None,
        claim: Some(SeatClaim::Named {
            display_name: name.to_string(),
        }),
    };
    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "A6".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    named("B6"),
                    named("C6"),
                    named("D6"),
                    named("E6"),
                    named("F6"),
                ],
                seed: Some(99),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    for player in [&bob, &carol, &dave, &eve, &frank] {
        let games: Vec<api::GameSummaryDto> = read_json(
            send_empty_auth(
                app.clone(),
                Method::GET,
                "/games",
                Some(&player.session_token),
            )
            .await,
        )
        .await;
        let invitation_id = games
            .iter()
            .find(|summary| summary.id == created.id)
            .expect("named invitation should appear in this player's games list")
            .invitation_id
            .clone()
            .expect("invitation id");
        let response = send_empty_auth(
            app.clone(),
            Method::POST,
            &format!("/invitations/{invitation_id}/accept"),
            Some(&player.session_token),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/start", created.id),
        Some(&alice.session_token),
        &StartGameRequest::default(),
    )
    .await;

    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&created.id).expect("game should exist");
        let exit = |game: &mut GameSession,
                    seat: u8,
                    move_number: i64,
                    move_type: &str,
                    description: &str| {
            game.participants[seat as usize].resigned = true;
            game.moves.push(MoveRecord {
                move_number,
                seat_number: seat,
                move_type: move_type.to_string(),
                main_word: None,
                score_delta: 0,
                positions: Vec::new(),
                description: description.to_string(),
            });
        };
        exit(game, 0, 1, "resign", "A6 resigned");
        exit(game, 1, 2, "resign", "B6 resigned");
        exit(
            game,
            2,
            3,
            "force_resign",
            "C6 resigned (by the game creator)",
        );
        exit(
            game,
            3,
            4,
            "timeout",
            "D6 was retired for exceeding the move time limit",
        );
        game.participants[4].score = 100;
        game.participants[5].score = 200;
        game.status = api::GameStatus::Finished;
        game.winner_seat = Some(5);
        persistence::save_game(&state.db, game)
            .await
            .expect("save should succeed");
    }

    for (name, player_id) in [("C6", &carol.player_id), ("D6", &dave.player_id)] {
        let stats = stats::get_subject_stats(&state.db, "player", player_id)
            .await
            .expect("stats query should succeed");
        assert_eq!(
            stats.games_rated, 0,
            "{name} was force-resigned/timed out and must not be rated"
        );
    }

    let rating_of = |player_id: &str| {
        let pool = state.db.clone();
        let player_id = player_id.to_string();
        async move {
            stats::get_subject_stats(&pool, "player", &player_id)
                .await
                .expect("stats query should succeed")
        }
    };
    let rating_a = rating_of(&alice.player_id).await;
    let rating_b = rating_of(&bob.player_id).await;
    let rating_e = rating_of(&eve.player_id).await;
    let rating_f = rating_of(&frank.player_id).await;
    assert_eq!(rating_a.games_rated, 1);
    assert_eq!(rating_b.games_rated, 1);
    assert_eq!(rating_e.games_rated, 1);
    assert_eq!(rating_f.games_rated, 1);
    assert!(
        rating_f.rating > rating_e.rating,
        "F (score 200) should rank above E (score 100): F={} E={}",
        rating_f.rating,
        rating_e.rating
    );
    assert!(
        rating_e.rating > rating_b.rating,
        "E (a finisher) should rank above B (a resigner) regardless of score: E={} B={}",
        rating_e.rating,
        rating_b.rating
    );
    assert!(
        rating_b.rating > rating_a.rating,
        "B (resigned later) should rank above A (resigned earlier): B={} A={}",
        rating_b.rating,
        rating_a.rating
    );
}

#[tokio::test]
async fn compute_winner_seat_ignores_a_resigned_seats_frozen_score() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_three_human_game(app.clone()).await;
    assert_eq!(started.game.current_seat, 0);

    // Alice resigns first, holding what would be (if it counted) the
    // highest score in the game — must never be allowed to "win" via
    // `compute_winner_seat`'s max, since she was no longer playing
    // when the game actually ended.
    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&started.game.id).expect("game should exist");
        game.apply_resign(0)
            .expect("Alice should be able to resign on her own turn");
        game.participants[0].score = 500;
    }

    // Bob and Carol pass repeatedly to hit the scoreless-turn limit
    // and finish the game normally between just the two of them.
    for seat in [1u8, 2, 1, 2, 1, 2] {
        let token = if seat == 1 {
            &started.bob.session_token
        } else {
            &started.carol.session_token
        };
        let response = send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/actions", started.game.id),
            Some(token),
            &GameActionRequest {
                seat_number: seat,
                action: PlayerActionDto::Pass,
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    let fetched: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::GET,
            &format!("/games/{}", started.game.id),
            Some(&started.alice.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(fetched.status, api::GameStatus::Finished);
    assert_ne!(
        fetched.winner_seat,
        Some(0),
        "Alice resigned and must never be declared the winner despite her frozen score"
    );
}

#[tokio::test]
async fn pass_action_advances_turn_and_records_move() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    assert_eq!(started.game.current_seat, 0);

    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Pass,
        },
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(response).await;
    assert_eq!(updated.status, api::GameStatus::Active);
    assert_eq!(updated.current_seat, 1);
    assert_eq!(updated.moves.len(), 1);
    assert_eq!(updated.moves[0].seat_number, 0);
    assert_eq!(updated.moves[0].move_type, "pass");
}

#[tokio::test]
async fn exchange_action_refills_rack_and_advances_turn() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;

    // Rig seat 0's rack with known letters so we can request their exchange
    // deterministically (mirrors the pattern used by the Place-move test above).
    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&started.game.id).expect("game should exist");
        game.participants[0].rack = rack_with_letters(&['A', 'T']);
    }

    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Exchange {
                tiles: vec![
                    api::TileDto::Letter {
                        letter: "A".to_string(),
                    },
                    api::TileDto::Letter {
                        letter: "T".to_string(),
                    },
                ],
            },
        },
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(response).await;
    assert_eq!(updated.current_seat, 1);
    assert_eq!(updated.moves.len(), 1);
    assert_eq!(updated.moves[0].move_type, "exchange");
}

#[tokio::test]
async fn resign_action_finishes_game_and_sets_winner() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;

    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Resign,
        },
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(response).await;
    assert_eq!(updated.status, api::GameStatus::Finished);
    assert_eq!(updated.winner_seat, Some(1));
    assert_eq!(updated.moves[0].move_type, "resign");
    // A resignation has no rack transfer — only going out does.
    assert_eq!(updated.final_bonus_seat, None);
    assert_eq!(updated.final_bonus_points, None);
}

#[tokio::test]
async fn removing_a_finished_game_hides_it_from_only_that_player() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Resign,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let remove_response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/remove", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(remove_response.status(), StatusCode::OK);

    let alice_games: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(
            app.clone(),
            Method::GET,
            "/games",
            Some(&started.alice.session_token),
        )
        .await,
    )
    .await;
    assert!(
        !alice_games
            .iter()
            .any(|summary| summary.id == started.game.id),
        "Alice removed this game, it should no longer appear in her list"
    );

    let bob_games: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(
            app.clone(),
            Method::GET,
            "/games",
            Some(&started.bob.session_token),
        )
        .await,
    )
    .await;
    assert!(
        bob_games
            .iter()
            .any(|summary| summary.id == started.game.id),
        "Bob never removed this game, it should still appear in his list"
    );
}

#[tokio::test]
async fn removing_a_game_that_is_not_finished_is_rejected() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let remove_response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/remove", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(remove_response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn removing_a_game_you_are_not_seated_in_is_rejected() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Resign,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let stranger = register_player(app.clone(), "Stranger").await;
    let remove_response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/remove", started.game.id),
        Some(&stranger.session_token),
    )
    .await;
    assert_eq!(remove_response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn acting_out_of_turn_returns_bad_request() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    assert_eq!(started.game.current_seat, 0);

    // Bob owns seat 1, so this clears the ownership check and reaches
    // the turn-order check underneath it.
    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.bob.session_token),
        &GameActionRequest {
            seat_number: 1,
            action: PlayerActionDto::Pass,
        },
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: api::ApiError = read_json(response).await;
    assert!(body.message.contains("turn"));
}

#[tokio::test]
async fn action_on_unknown_game_returns_not_found() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_json(
        app.clone(),
        Method::POST,
        "/games/does-not-exist/actions",
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Pass,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let get_response = send_empty(app, Method::GET, "/games/does-not-exist").await;
    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn illegal_placement_is_rejected_and_does_not_advance_turn() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;

    // An empty tile placement is not a legal move shape.
    let empty_candidate = api::MoveCandidateDto {
        start: api::PositionDto { x: 7, y: 7 },
        direction: api::DirectionDto::Horizontal,
        tiles: vec![],
    };

    let response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Place {
                candidate: empty_candidate,
            },
        },
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Turn should not have advanced after a rejected move.
    let fetched: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::GET,
            &format!("/games/{}", started.game.id),
            Some(&started.alice.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(fetched.current_seat, 0);
    assert_eq!(fetched.moves.len(), 0);
}

#[tokio::test]
async fn claimed_seat_rejects_actions_from_a_different_player() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice: PlayerSessionDto = read_json(
        send_json(
            app.clone(),
            Method::POST,
            "/auth/register",
            &RegisterPlayerRequest {
                display_name: "Alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "correct horse battery staple".to_string(),
                stay_logged_in: false,
            },
        )
        .await,
    )
    .await;

    let mallory: PlayerSessionDto = read_json(
        send_json(
            app.clone(),
            Method::POST,
            "/auth/register",
            &RegisterPlayerRequest {
                display_name: "Mallory".to_string(),
                email: "mallory@example.com".to_string(),
                password: "another password entirely".to_string(),
                stay_logged_in: false,
            },
        )
        .await,
    )
    .await;

    // Alice creates a game while logged in, so seat 0 is bound to her.
    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(7),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert_eq!(
        created.participants[0].player_id.as_deref(),
        Some(alice.player_id.as_str())
    );

    send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/start", created.id),
        Some(&alice.session_token),
        &StartGameRequest::default(),
    )
    .await;

    // Mallory can't act as Alice's seat, even with a valid session of
    // her own.
    let rejected = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/actions", created.id),
        Some(&mallory.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Pass,
        },
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

    // Alice can.
    let accepted = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", created.id),
        Some(&alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Pass,
        },
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
}

/// Registers Alice and Mallory, and has Alice create a game (while
/// authenticated) with a human seat 0 for herself and the given kind
/// for seat 1. Returns both sessions and the created game.
async fn create_claimed_game_and_second_player(
    app: Router,
    seat_one_kind: SeatKind,
) -> (PlayerSessionDto, PlayerSessionDto, GameStateDto) {
    let alice: PlayerSessionDto = read_json(
        send_json(
            app.clone(),
            Method::POST,
            "/auth/register",
            &RegisterPlayerRequest {
                display_name: "Alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "correct horse battery staple".to_string(),
                stay_logged_in: false,
            },
        )
        .await,
    )
    .await;

    let mallory: PlayerSessionDto = read_json(
        send_json(
            app.clone(),
            Method::POST,
            "/auth/register",
            &RegisterPlayerRequest {
                display_name: "Mallory".to_string(),
                email: "mallory@example.com".to_string(),
                password: "another password entirely".to_string(),
                stay_logged_in: false,
            },
        )
        .await,
    )
    .await;

    let seat_one = match seat_one_kind {
        // Never exercised as Human by any current caller — left open
        // (unclaimed) purely so the request stays valid to build.
        SeatKind::Human => CreateSeatRequest {
            kind: SeatKind::Human,
            display_name: "Player 2".to_string(),
            engine_id: None,
            claim: Some(SeatClaim::Open),
        },
        SeatKind::Engine => CreateSeatRequest {
            kind: SeatKind::Engine,
            display_name: "Greedy".to_string(),
            engine_id: Some("greedy-v1".to_string()),
            claim: None,
        },
    };

    let created: GameStateDto = read_json(
        send_json_auth(
            app,
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    seat_one,
                ],
                seed: Some(7),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert_eq!(
        created.participants[0].player_id.as_deref(),
        Some(alice.player_id.as_str())
    );

    (alice, mallory, created)
}

#[tokio::test]
async fn claimed_game_only_starts_for_a_seat_owner() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let (alice, mallory, created) =
        create_claimed_game_and_second_player(app.clone(), SeatKind::Engine).await;

    let rejected = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/start", created.id),
        Some(&mallory.session_token),
        &StartGameRequest::default(),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

    let accepted = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/start", created.id),
        Some(&alice.session_token),
        &StartGameRequest::default(),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
}

#[tokio::test]
async fn creating_a_game_without_auth_is_rejected() {
    // Every seat now needs a real claiming party, so there's no more
    // "anonymous, open to anyone" game to create.
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_json(
        app,
        Method::POST,
        "/games",
        &CreateGameRequest {
            seats: vec![CreateSeatRequest {
                kind: SeatKind::Engine,
                display_name: "Greedy".to_string(),
                engine_id: Some("greedy-v1".to_string()),
                claim: None,
            }],
            seed: None,
            variant: None,
            language: None,
            board_layout: None,
            move_time_limit_seconds: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn listing_games_without_auth_is_rejected() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_empty(app, Method::GET, "/games").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn claimed_seat_preview_rejects_a_different_player() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let (alice, mallory, created) =
        create_claimed_game_and_second_player(app.clone(), SeatKind::Engine).await;

    let candidate = api::MoveCandidateDto {
        start: api::PositionDto { x: 7, y: 7 },
        direction: api::DirectionDto::Horizontal,
        tiles: vec![api::TilePlacementDto {
            offset: 0,
            tile: api::TileDto::Letter {
                letter: "A".to_string(),
            },
        }],
    };

    let rejected = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/preview", created.id),
        Some(&mallory.session_token),
        &PreviewMoveRequest {
            seat_number: 0,
            candidate: candidate.clone(),
        },
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

    let accepted = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/preview", created.id),
        Some(&alice.session_token),
        &PreviewMoveRequest {
            seat_number: 0,
            candidate,
        },
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
}

#[tokio::test]
async fn claimed_seat_suggest_move_rejects_a_different_player() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let (alice, mallory, created) =
        create_claimed_game_and_second_player(app.clone(), SeatKind::Engine).await;

    send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/start", created.id),
        Some(&alice.session_token),
        &StartGameRequest::default(),
    )
    .await;

    let rejected = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/suggest", created.id),
        Some(&mallory.session_token),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

    let accepted = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/suggest", created.id),
        Some(&alice.session_token),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
}

#[test]
fn event_belongs_to_game_filters_by_id() {
    // Regression test for the bug this fixed: a socket subscribed to
    // one game's events must not receive another game's, even though
    // every game shares the same underlying broadcast channel.
    let event_for_a = GameEventDto::StateUpdated {
        game: empty_live_game_for_test("game-a"),
    };
    let event_for_b = GameEventDto::GameFinished {
        game: empty_live_game_for_test("game-b"),
    };

    assert!(event_belongs_to_game(&event_for_a, "game-a"));
    assert!(!event_belongs_to_game(&event_for_a, "game-b"));
    assert!(event_belongs_to_game(&event_for_b, "game-b"));
    assert!(!event_belongs_to_game(&event_for_b, "game-a"));
}

// `game_events`'s auth (query-token lookup, `resolve_viewer_access`,
// rejecting before `.on_upgrade()`) isn't covered by an HTTP-level test
// here — axum's `WebSocketUpgrade` extractor needs a real hyper
// connection's upgrade machinery (an `OnUpgrade` future stashed in
// request extensions during actual socket I/O) that a `oneshot`-driven
// fake `Request` can't provide, regardless of headers; every attempt
// came back `426 Upgrade Required` from the extractor itself, before
// reaching this handler's own logic at all. The authorization logic
// itself (`resolve_viewer_access`, `redact_game_state`/`redact_event`)
// is fully covered by unit tests in `game_state.rs`; the HTTP wiring
// (query-token parsing, the pre-upgrade rejection, per-connection
// redaction) was verified live in the browser instead.

#[tokio::test]
async fn seated_participant_can_chat_and_it_persists() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);
    let started = create_two_human_game(app.clone()).await;

    let response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/chat", started.game.id),
        Some(&started.alice.session_token),
        &PostChatMessageRequest {
            body: "  good luck!  ".to_string(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let dto: GameStateDto = read_json(response).await;
    assert_eq!(dto.messages.len(), 1);
    assert_eq!(dto.messages[0].display_name, "Alice");
    // Trimmed, per `post_chat_message`'s own contract.
    assert_eq!(dto.messages[0].body, "good luck!");

    // Shows up for the other seated participant too, via a plain fetch.
    let fetched: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::GET,
            &format!("/games/{}", started.game.id),
            Some(&started.bob.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(fetched.messages.len(), 1);
    assert_eq!(fetched.messages[0].body, "good luck!");
}

#[tokio::test]
async fn a_non_participant_cannot_chat() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);
    let started = create_two_human_game(app.clone()).await;
    let mallory = register_player(app.clone(), "Mallory").await;

    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/chat", started.game.id),
        Some(&mallory.session_token),
        &PostChatMessageRequest {
            body: "let me in".to_string(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_game_rejects_unauthenticated_and_unrelated_callers() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);
    let started = create_two_human_game(app.clone()).await;
    let mallory = register_player(app.clone(), "Mallory").await;

    let unauthenticated = send_empty(
        app.clone(),
        Method::GET,
        &format!("/games/{}", started.game.id),
    )
    .await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let unrelated = send_empty_auth(
        app,
        Method::GET,
        &format!("/games/{}", started.game.id),
        Some(&mallory.session_token),
    )
    .await;
    assert_eq!(unrelated.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_game_redacts_racks_and_chat_by_viewer_tier() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);
    let started = create_two_human_game(app.clone()).await;

    send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/chat", started.game.id),
        Some(&started.alice.session_token),
        &PostChatMessageRequest {
            body: "hi".to_string(),
        },
    )
    .await;

    // Alice (seat 0) sees her own rack and the chat.
    let as_alice: GameStateDto = read_json(
        send_empty_auth(
            app.clone(),
            Method::GET,
            &format!("/games/{}", started.game.id),
            Some(&started.alice.session_token),
        )
        .await,
    )
    .await;
    assert!(!as_alice.racks[0].counts.is_empty());
    assert!(
        as_alice.racks[1].counts.is_empty(),
        "opponent's rack must stay redacted"
    );
    assert_eq!(as_alice.messages.len(), 1);
}

/// A minimal single-player-vs-engine game, started immediately (no
/// invitation dance needed) — for tests that just need *some* game to
/// exist in `state.games`/the database and don't care about real
/// gameplay. Takes an explicit `creator_name` since a display name can
/// only be registered once per test's shared state.
async fn create_and_start_engine_game(app: Router, creator_name: &str) -> GameStateDto {
    let creator = register_player(app.clone(), creator_name).await;
    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&creator.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: creator_name.to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(1),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    read_json(
        send_json_auth(
            app,
            Method::POST,
            &format!("/games/{}/start", created.id),
            Some(&creator.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await
}

#[tokio::test]
async fn expire_old_finished_games_deletes_stale_games_but_not_recent_ones() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let old = create_and_start_engine_game(app.clone(), "ExpireOld").await;
    let recent = create_and_start_engine_game(app.clone(), "ExpireRecent").await;

    // `ended_at` is only ever set in SQL, by `save_game`, based on
    // `status == Finished` at save time — bypass the normal flow and
    // write it directly so the test controls the exact age.
    let now = now_unix_seconds();
    let eight_days_ago = now - 8 * 24 * 60 * 60;
    let one_day_ago = now - 24 * 60 * 60;
    sqlx::query("update games set status = 'finished', ended_at = ?1 where id = ?2")
        .bind(eight_days_ago)
        .bind(&old.id)
        .execute(&state.db)
        .await
        .expect("update should succeed");
    sqlx::query("update games set status = 'finished', ended_at = ?1 where id = ?2")
        .bind(one_day_ago)
        .bind(&recent.id)
        .execute(&state.db)
        .await
        .expect("update should succeed");
    {
        let mut games = state.games.write().await;
        for id in [&old.id, &recent.id] {
            let game = games.get_mut(id).expect("game should exist");
            game.status = api::GameStatus::Finished;
        }
    }

    expire_old_finished_games(&state).await;

    let games = state.games.read().await;
    assert!(
        !games.contains_key(&old.id),
        "a game finished 8 days ago should have been deleted"
    );
    assert!(
        games.contains_key(&recent.id),
        "a game finished 1 day ago should still be here"
    );
    let remaining_ids = persistence::list_game_ids(&state.db)
        .await
        .expect("list should succeed");
    assert!(
        !remaining_ids.contains(&old.id),
        "the stale game's row should be gone from the database too"
    );
}

fn empty_live_game_for_test(id: &str) -> GameStateDto {
    GameStateDto {
        id: id.to_string(),
        status: api::GameStatus::Waiting,
        creator_player_id: None,
        variant: "official".to_string(),
        language: "sowpods".to_string(),
        board_layout: "official".to_string(),
        turn_number: 0,
        current_seat: 0,
        winner_seat: None,
        final_bonus_seat: None,
        final_bonus_points: None,
        bag_count: 100,
        move_time_limit_seconds: 0,
        turn_started_at: 0,
        participants: Vec::new(),
        board: Vec::new(),
        racks: Vec::new(),
        moves: Vec::new(),
        messages: Vec::new(),
    }
}

#[tokio::test]
async fn registering_a_taken_display_name_is_rejected() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let first = send_json(
        app.clone(),
        Method::POST,
        "/auth/register",
        &RegisterPlayerRequest {
            display_name: "John".to_string(),
            email: "john1@example.com".to_string(),
            password: "first-password".to_string(),
            stay_logged_in: false,
        },
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);

    // A second "John" with a different email/password must be rejected
    // outright, rather than silently colliding with the first at login.
    let second = send_json(
        app,
        Method::POST,
        "/auth/register",
        &RegisterPlayerRequest {
            display_name: "John".to_string(),
            email: "john2@example.com".to_string(),
            password: "second-password".to_string(),
            stay_logged_in: false,
        },
    )
    .await;
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn registering_without_stay_logged_in_gets_a_session_that_expires() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let db = state.db.clone();
    let app = build_router(state);

    let response = send_json(
        app,
        Method::POST,
        "/auth/register",
        &RegisterPlayerRequest {
            display_name: "Alice".to_string(),
            email: "alice@example.com".to_string(),
            password: "correct horse battery staple".to_string(),
            stay_logged_in: false,
        },
    )
    .await;
    let session: PlayerSessionDto = read_json(response).await;

    let record = persistence::get_session_by_token_hash(&db, &hash_token(&session.session_token))
        .await
        .expect("query should succeed")
        .expect("session should exist");
    assert!(!record.stay_logged_in);
    let expires_at = record.expires_at.expect("should have an expiry");
    let now = super::now_unix_seconds();
    // Should be ~10 days out (the uniform absolute cap) — just confirm it's
    // in the right ballpark rather than pinning an exact second.
    assert!(expires_at > now + 9 * 24 * 60 * 60);
    assert!(expires_at < now + 11 * 24 * 60 * 60);
}

#[tokio::test]
async fn stay_logged_in_no_longer_changes_server_side_expiry() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let db = state.db.clone();
    let app = build_router(state);

    register_player(app.clone(), "Alice").await;
    let response = send_json(
        app,
        Method::POST,
        "/auth/login",
        &LoginPlayerRequest {
            display_name: "Alice".to_string(),
            password: "correct horse battery staple".to_string(),
            stay_logged_in: true,
        },
    )
    .await;
    let session: PlayerSessionDto = read_json(response).await;

    let record = persistence::get_session_by_token_hash(&db, &hash_token(&session.session_token))
        .await
        .expect("query should succeed")
        .expect("session should exist");
    // `stay_logged_in` is still recorded (it drives client-side token
    // persistence), but it no longer affects server-side expiry: every
    // session now gets the same ~10-day absolute cap, not the old "never
    // expires".
    assert!(record.stay_logged_in);
    let expires_at = record
        .expires_at
        .expect("a stay_logged_in session now has an absolute expiry too");
    let now = super::now_unix_seconds();
    assert!(expires_at > now + 9 * 24 * 60 * 60);
    assert!(expires_at < now + 11 * 24 * 60 * 60);
}

/// A session past its absolute `expires_at` is rejected. A session with *no*
/// absolute expiry (the shape old `stay_logged_in` rows have, from before
/// expiry became uniform) is still honoured while its `last_seen_at` is
/// fresh — but the idle window catches it once dormant, which is what
/// finally bounds those previously-immortal rows.
#[tokio::test]
async fn expired_absolute_token_is_rejected_and_a_no_expiry_one_idle_expires() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());
    let db = state.db.clone();

    let expired_token = "expired-token";
    persistence::create_session(
        &state.db,
        &Uuid::new_v4().to_string(),
        &register_player(app.clone(), "Alice").await.player_id,
        &hash_token(expired_token),
        false,
        Some(0), // already past its absolute expiry
    )
    .await
    .expect("session should be created");

    let no_expiry_token = "no-expiry-token";
    persistence::create_session(
        &state.db,
        &Uuid::new_v4().to_string(),
        &register_player(app.clone(), "Bob").await.player_id,
        &hash_token(no_expiry_token),
        true,
        None, // no absolute expiry, like a pre-change stay_logged_in row
    )
    .await
    .expect("session should be created");

    // Past absolute expiry -> rejected.
    let expired = send_empty_auth(app.clone(), Method::GET, "/games", Some(expired_token)).await;
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);

    // No absolute expiry but freshly created (last_seen = now) -> still valid.
    let fresh = send_empty_auth(app.clone(), Method::GET, "/games", Some(no_expiry_token)).await;
    assert_eq!(fresh.status(), StatusCode::OK);

    // ...but once it goes dormant past the idle window, it's rejected too.
    let now = now_unix_seconds();
    let stale = now - persistence::SESSION_IDLE_WINDOW_SECS - 60;
    sqlx::query("update sessions set last_seen_at = ?1 where token_hash = ?2")
        .bind(stale)
        .bind(hash_token(no_expiry_token))
        .execute(&db)
        .await
        .expect("backdate last_seen");
    let idle = send_empty_auth(app, Method::GET, "/games", Some(no_expiry_token)).await;
    assert_eq!(idle.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delete_expired_sessions_removes_only_past_expiry_ones() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());
    let db = state.db.clone();

    let alice = register_player(app.clone(), "Alice").await;
    let bob = register_player(app.clone(), "Bob").await;
    let carol = register_player(app.clone(), "Carol").await;

    // alice: already past absolute expiry; bob: expiry far in the future;
    // carol: stay_logged_in, no absolute expiry. All three get a fresh
    // `last_seen_at` from create_session, so the idle clause doesn't touch
    // them and only alice's absolute expiry deletes her — the idle clause is
    // exercised in `an_idle_session_past_the_window_is_rejected_and_swept`.
    persistence::create_session(
        &db,
        &Uuid::new_v4().to_string(),
        &alice.player_id,
        &hash_token("alice-expired"),
        false,
        Some(0),
    )
    .await
    .expect("session should be created");
    persistence::create_session(
        &db,
        &Uuid::new_v4().to_string(),
        &bob.player_id,
        &hash_token("bob-future"),
        false,
        Some(9999999999),
    )
    .await
    .expect("session should be created");
    persistence::create_session(
        &db,
        &Uuid::new_v4().to_string(),
        &carol.player_id,
        &hash_token("carol-forever"),
        true,
        None,
    )
    .await
    .expect("session should be created");

    persistence::delete_expired_sessions(&db)
        .await
        .expect("cleanup should succeed");

    assert!(
        persistence::get_session_by_token_hash(&db, &hash_token("alice-expired"))
            .await
            .expect("query should succeed")
            .is_none()
    );
    assert!(
        persistence::get_session_by_token_hash(&db, &hash_token("bob-future"))
            .await
            .expect("query should succeed")
            .is_some()
    );
    assert!(
        persistence::get_session_by_token_hash(&db, &hash_token("carol-forever"))
            .await
            .expect("query should succeed")
            .is_some()
    );
}

/// Idle expiry: a session dormant past the idle window is rejected on the
/// next authenticated call and removed by the sweep — even though its
/// absolute `expires_at` is still in the future. This is what finally bounds
/// the previously-immortal `stay_logged_in` rows.
#[tokio::test]
async fn an_idle_session_past_the_window_is_rejected_and_swept() {
    let state = create_test_state(&test_database_url()).await;
    let app = build_router(state.clone());
    let db = state.db.clone();
    let alice = register_player(app.clone(), "Alice").await;

    let now = now_unix_seconds();
    let stale = now - persistence::SESSION_IDLE_WINDOW_SECS - 60;
    sqlx::query("update sessions set last_seen_at = ?1 where token_hash = ?2")
        .bind(stale)
        .bind(hash_token(&alice.session_token))
        .execute(&db)
        .await
        .expect("backdate last_seen");

    let response = send_json(
        app.clone(),
        Method::POST,
        "/auth/validate",
        &api::ValidateSessionRequest {
            session_token: alice.session_token.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    persistence::delete_expired_sessions(&db)
        .await
        .expect("sweep should succeed");
    assert!(
        persistence::get_session_by_token_hash(&db, &hash_token(&alice.session_token))
            .await
            .expect("query should succeed")
            .is_none()
    );
}

/// Absolute cap: a session past its hard `expires_at` is rejected and swept
/// even when `last_seen_at` is fresh (i.e. it was being actively used) — the
/// two limits are independent.
#[tokio::test]
async fn a_session_past_absolute_expiry_is_rejected_even_when_recently_active() {
    let state = create_test_state(&test_database_url()).await;
    let app = build_router(state.clone());
    let db = state.db.clone();
    let alice = register_player(app.clone(), "Alice").await;

    sqlx::query("update sessions set expires_at = '1' where token_hash = ?1")
        .bind(hash_token(&alice.session_token))
        .execute(&db)
        .await
        .expect("backdate expires_at");

    let response = send_json(
        app.clone(),
        Method::POST,
        "/auth/validate",
        &api::ValidateSessionRequest {
            session_token: alice.session_token.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    persistence::delete_expired_sessions(&db)
        .await
        .expect("sweep should succeed");
    assert!(
        persistence::get_session_by_token_hash(&db, &hash_token(&alice.session_token))
            .await
            .expect("query should succeed")
            .is_none()
    );
}

/// Explicit logout deletes the row immediately and the token stops
/// validating right away — no waiting for the sweep.
#[tokio::test]
async fn logout_deletes_the_session_and_invalidates_its_token() {
    let state = create_test_state(&test_database_url()).await;
    let app = build_router(state.clone());
    let db = state.db.clone();
    let alice = register_player(app.clone(), "Alice").await;

    let response = send_empty_auth(
        app.clone(),
        Method::POST,
        "/auth/logout",
        Some(&alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    assert!(
        persistence::get_session_by_token_hash(&db, &hash_token(&alice.session_token))
            .await
            .expect("query should succeed")
            .is_none()
    );

    let validate = send_json(
        app.clone(),
        Method::POST,
        "/auth/validate",
        &api::ValidateSessionRequest {
            session_token: alice.session_token.clone(),
        },
    )
    .await;
    assert_eq!(validate.status(), StatusCode::UNAUTHORIZED);
}

/// An authenticated request refreshes `last_seen_at` when it's staler than
/// the bump throttle, so the idle window slides with activity.
#[tokio::test]
async fn an_authenticated_request_bumps_stale_last_seen() {
    let state = create_test_state(&test_database_url()).await;
    let app = build_router(state.clone());
    let db = state.db.clone();
    let alice = register_player(app.clone(), "Alice").await;

    // Past the bump throttle but well within the idle window: still valid,
    // but due for a refresh.
    let now = now_unix_seconds();
    let staleish = now - persistence::LAST_SEEN_BUMP_THROTTLE_SECS - 60;
    sqlx::query("update sessions set last_seen_at = ?1 where token_hash = ?2")
        .bind(staleish)
        .bind(hash_token(&alice.session_token))
        .execute(&db)
        .await
        .expect("backdate last_seen");

    let response = send_json(
        app.clone(),
        Method::POST,
        "/auth/validate",
        &api::ValidateSessionRequest {
            session_token: alice.session_token.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let session = persistence::get_session_by_token_hash(&db, &hash_token(&alice.session_token))
        .await
        .expect("query should succeed")
        .expect("session still exists");
    let bumped = session.last_seen_at;
    assert!(
        bumped >= now - 5,
        "last_seen should have been refreshed to ~now (backdated to {staleish}, now {bumped})"
    );
}

/// A game snapshot written before timestamps became `i64` stored
/// `turn_started_at` (and chat `created_at`) as strings inside
/// `snapshot_json`. The `0004` migration only converts the timestamp
/// *columns*, not the JSON blob, so the serde string-or-number shim is what
/// lets such a game still load — i.e. this is a real in-place upgrade, not a
/// wipe.
#[tokio::test]
async fn a_snapshot_with_string_timestamps_still_loads() {
    let state = create_test_state(&test_database_url()).await;
    let app = build_router(state.clone());
    let db = state.db.clone();
    let game = create_two_human_game(app.clone()).await;
    let game_id = game.game.id.clone();

    // Rewrite the snapshot's numeric turn_started_at as a string, mimicking a
    // snapshot written by the pre-integer code.
    let snapshot: String = sqlx::query_scalar("select snapshot_json from games where id = ?1")
        .bind(&game_id)
        .fetch_one(&db)
        .await
        .expect("snapshot exists");
    let mut value: serde_json::Value = serde_json::from_str(&snapshot).expect("valid json");
    let ts = value["turn_started_at"]
        .as_i64()
        .expect("numeric turn_started_at");
    value["turn_started_at"] = serde_json::Value::String(ts.to_string());
    let rewritten = serde_json::to_string(&value).expect("serialize");
    sqlx::query("update games set snapshot_json = ?1 where id = ?2")
        .bind(&rewritten)
        .bind(&game_id)
        .execute(&db)
        .await
        .expect("update snapshot");

    let reloaded = persistence::load_game(&db, &game_id)
        .await
        .expect("load ok")
        .expect("game still loads from a string-timestamp snapshot");
    assert_eq!(reloaded.turn_started_at, ts);
}

#[tokio::test]
async fn search_players_matches_by_prefix_case_insensitively_and_requires_auth() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;
    register_player(app.clone(), "Alicia").await;
    register_player(app.clone(), "Bob").await;

    let unauthenticated =
        send_empty_auth(app.clone(), Method::GET, "/players/search?q=ali", None).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let names: Vec<String> = read_json(
        send_empty_auth(
            app,
            Method::GET,
            "/players/search?q=ALI",
            Some(&alice.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(names, vec!["Alice".to_string(), "Alicia".to_string()]);
}

#[tokio::test]
async fn engine_vs_engine_game_runs_to_completion() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let owner = register_player(app.clone(), "Referee").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&owner.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy One".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy Two".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(777),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert_eq!(created.status, api::GameStatus::Waiting);

    // A single /start call should drive both engine seats all the way
    // to game-over: no human ever exists to trigger a follow-up round,
    // so `run_engine_turns` has to run the whole game in one go. Neither
    // seat is claimed (both are engines), so any signed-in caller may
    // start it.
    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/start", created.id),
        Some(&owner.session_token),
        &StartGameRequest::default(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let finished: GameStateDto = read_json(response).await;

    assert_eq!(finished.status, api::GameStatus::Finished);
    assert!(
        !finished.moves.is_empty(),
        "expected the engines to have played at least one move before the game ended"
    );
    assert!(
        finished
            .moves
            .iter()
            .any(|record| record.move_type == "place"),
        "expected at least one engine to place tiles rather than only pass, got moves: {:?}",
        finished.moves
    );
    assert!(
        finished
            .participants
            .iter()
            .any(|participant| participant.score != 0),
        "expected at least one participant to have a non-zero score by game end"
    );
}

#[tokio::test]
async fn bot_showdown_nets_to_exactly_zero_rating_change_but_still_counts_the_game() {
    // Both seats resolve to the identical rating subject ("greedy-v1",
    // the only registered engine) — per the design, this must not be
    // skipped as unratable; instead the self-pair's two opposite
    // deltas must sum to exactly zero, leaving the bot's rating
    // unchanged while still recording that it played one game.
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let owner = register_player(app.clone(), "Referee3").await;
    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&owner.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy One".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy Two".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(778),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/start", created.id),
        Some(&owner.session_token),
        &StartGameRequest::default(),
    )
    .await;

    let stats = stats::get_subject_stats(&state.db, "engine", "greedy-v1")
        .await
        .expect("stats query should succeed");
    assert_eq!(
        stats.rating, 1500.0,
        "a self-play game must net to zero rating change"
    );
    assert_eq!(
        stats.games_rated, 1,
        "one game played, not two, even though it occupied two seats"
    );

    let history = stats::get_rating_history(&state.db, "engine", "greedy-v1")
        .await
        .expect("history query should succeed");
    assert_eq!(
        history.len(),
        1,
        "one rating_history row for the game, not one per seat"
    );
}

#[tokio::test]
async fn passing_out_a_game_ties_and_leaves_first_time_ratings_at_1500() {
    // Two brand-new (1500-rated) players drawing means expected score
    // and actual score both land on exactly 0.5 for each side — zero
    // delta either way. This also exercises the only path that can
    // produce a genuine 'tie' outcome (the scoreless-turn-limit path,
    // via `finish_game(None)`).
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;

    // Both racks emptied so the scoreless-limit ending's usual
    // rack-value deduction (`finish_game`'s `participant.score -=
    // rack_value`) subtracts zero from both — otherwise whichever
    // rack happened to be dealt a lower-value hand would "win" on
    // points, which isn't the tie this test is actually after.
    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&started.game.id).expect("game should exist");
        for participant in &mut game.participants {
            participant.rack = Rack::default();
        }
    }

    for seat in [0u8, 1, 0, 1, 0, 1] {
        let response = send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/actions", started.game.id),
            Some(if seat == 0 {
                &started.alice.session_token
            } else {
                &started.bob.session_token
            }),
            &GameActionRequest {
                seat_number: seat,
                action: PlayerActionDto::Pass,
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    let fetched: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::GET,
            &format!("/games/{}", started.game.id),
            Some(&started.alice.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(fetched.status, api::GameStatus::Finished);
    assert_eq!(
        fetched.winner_seat, None,
        "an even pass-out is a genuine tie"
    );

    for participant in &fetched.participants {
        assert_eq!(participant.rating_before, Some(1500.0));
        assert_eq!(participant.rating_after, Some(1500.0));
    }
}

#[tokio::test]
async fn voluntary_resignation_moves_rating_via_forfeit_ranking_even_when_ahead_on_score() {
    // Seat 0 is well ahead on points, then resigns. Ranking by raw
    // score would let the resigner "win" the rating exchange by
    // quitting while ahead — the forfeit override must rank them last
    // regardless, so the game still records seat 0 losing rating and
    // seat 1 gaining it.
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&started.game.id).expect("game should exist");
        game.participants[0].score = 300;
    }

    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/actions", started.game.id),
        Some(&started.alice.session_token),
        &GameActionRequest {
            seat_number: 0,
            action: PlayerActionDto::Resign,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let finished: GameStateDto = read_json(response).await;
    assert_eq!(finished.status, api::GameStatus::Finished);
    assert_eq!(finished.winner_seat, Some(1));

    let resigner_rating = finished.participants[0]
        .rating_after
        .expect("resigner should be rated");
    let winner_rating = finished.participants[1]
        .rating_after
        .expect("winner should be rated");
    assert!(
        resigner_rating < 1500.0,
        "resigning while 300 points ahead must still lose rating: got {resigner_rating}"
    );
    assert!(
        winner_rating > 1500.0,
        "the non-resigning seat must gain rating: got {winner_rating}"
    );
    // `current_rating` (shown next to a player's name in the roster)
    // must reflect the just-applied change, not the stale pre-game
    // value.
    assert_eq!(
        finished.participants[0].current_rating,
        Some(resigner_rating)
    );
    assert_eq!(finished.participants[1].current_rating, Some(winner_rating));
}

#[tokio::test]
async fn current_rating_defaults_to_1500_for_a_never_rated_player() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    for participant in &started.game.participants {
        assert_eq!(
            participant.current_rating,
            Some(1500.0),
            "a brand-new player's roster entry should show the default rating, not None"
        );
    }
}

#[tokio::test]
async fn force_resignation_does_not_move_rating() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let response = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/seats/1/force-resign", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let finished: GameStateDto = read_json(response).await;
    assert_eq!(finished.status, api::GameStatus::Finished);
    for participant in &finished.participants {
        assert_eq!(
            participant.rating_before, None,
            "a creator-forced resignation must not move rating"
        );
        assert_eq!(participant.rating_after, None);
    }

    let winner_stats = stats::get_subject_stats(&state.db, "player", &started.alice.player_id)
        .await
        .expect("stats query should succeed");
    assert_eq!(winner_stats.games_rated, 0);
}

#[tokio::test]
async fn timeout_does_not_move_rating() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&started.game.id).expect("game should exist");
        game.turn_started_at = 0;
    }

    let fetched: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::GET,
            &format!("/games/{}", started.game.id),
            Some(&started.alice.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(fetched.status, api::GameStatus::Finished);
    assert_eq!(
        fetched
            .moves
            .last()
            .expect("a move should be recorded")
            .move_type,
        "timeout"
    );
    for participant in &fetched.participants {
        assert_eq!(
            participant.rating_before, None,
            "a timeout must not move rating"
        );
        assert_eq!(participant.rating_after, None);
    }
}

#[tokio::test]
async fn admin_force_end_does_not_move_rating() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let response = send_admin::<()>(
        app,
        Method::POST,
        &format!("/admin/games/{}/force-end", started.game.id),
        loopback_peer(),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let finished: GameStateDto = read_json(response).await;
    assert_eq!(finished.status, api::GameStatus::Finished);
    for participant in &finished.participants {
        assert_eq!(
            participant.rating_before, None,
            "an admin force-end must not move rating"
        );
        assert_eq!(participant.rating_after, None);
    }
}

#[tokio::test]
async fn resaving_an_already_finished_game_does_not_re_rate_it() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    for seat in [0u8, 1, 0, 1, 0, 1] {
        send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/actions", started.game.id),
            Some(if seat == 0 {
                &started.alice.session_token
            } else {
                &started.bob.session_token
            }),
            &GameActionRequest {
                seat_number: seat,
                action: PlayerActionDto::Pass,
            },
        )
        .await;
    }

    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&started.game.id).expect("game should exist");
        assert_eq!(game.status, api::GameStatus::Finished);
        persistence::save_game(&state.db, game)
            .await
            .expect("re-saving an already-finished game should succeed");
    }

    let stats = stats::get_subject_stats(&state.db, "player", &started.alice.player_id)
        .await
        .expect("stats query should succeed");
    assert_eq!(
        stats.games_rated, 1,
        "re-saving an already-rated game must not rate it a second time"
    );
}

#[tokio::test]
async fn engine_vs_engine_game_appears_in_its_creators_list() {
    // The creator holds no seat in an Engine vs Engine game (both seats
    // are engines), so `list_games` can't find them via `participants`
    // or an invitation the way it does for every other game kind — this
    // is what `creator_player_id` exists to cover.
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let owner = register_player(app.clone(), "Referee2").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&owner.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy One".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy Two".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(778),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert!(
        created
            .participants
            .iter()
            .all(|participant| participant.player_id.is_none()),
        "the creator should not hold a seat in an all-engine game"
    );

    let listed: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(
            app.clone(),
            Method::GET,
            "/games",
            Some(&owner.session_token),
        )
        .await,
    )
    .await;
    let summary = listed
        .iter()
        .find(|summary| summary.id == created.id)
        .expect("engine-vs-engine game should still appear for its creator");
    assert_eq!(summary.relationship, api::GameRelationship::Creator);
}

#[tokio::test]
async fn removing_a_finished_engine_vs_engine_game_hides_it_from_its_unseated_creator() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let owner = register_player(app.clone(), "Referee3").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&owner.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy One".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy Two".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(779),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    let started: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/start", created.id),
            Some(&owner.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;
    assert_eq!(started.status, api::GameStatus::Finished);

    let remove_response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/remove", started.id),
        Some(&owner.session_token),
    )
    .await;
    assert_eq!(remove_response.status(), StatusCode::OK);

    let listed: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(
            app.clone(),
            Method::GET,
            "/games",
            Some(&owner.session_token),
        )
        .await,
    )
    .await;
    assert!(
        !listed.iter().any(|summary| summary.id == started.id),
        "the creator removed this game, it should no longer appear in their list"
    );
}

#[tokio::test]
async fn reorder_seats_swaps_turn_order_before_the_game_starts() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;
    assert_eq!(waiting.game.participants[0].display_name, "Alice");
    assert_eq!(waiting.game.participants[1].display_name, "Open seat");

    let response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/reorder-seats", waiting.game.id),
        Some(&waiting.alice.session_token),
        &api::SwapSeatsRequest {
            seat_a: 0,
            seat_b: 1,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let reordered: GameStateDto = read_json(response).await;
    // "Bob", not "Open seat" — this is fetched fresh after Bob already
    // accepted (unlike `waiting.game` above, a snapshot from before
    // that), so it reflects `accept_invitation` filling in the real
    // display name of whoever claimed the open seat.
    assert_eq!(reordered.participants[0].display_name, "Bob");
    assert_eq!(reordered.participants[0].seat_number, 0);
    assert_eq!(reordered.participants[1].display_name, "Alice");
    assert_eq!(reordered.participants[1].seat_number, 1);

    // The new order actually determines turn order once started.
    let started: GameStateDto = read_json(
        send_json_auth(
            app,
            Method::POST,
            &format!("/games/{}/start", waiting.game.id),
            Some(&waiting.alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;
    assert_eq!(started.current_seat, 0);
    assert_eq!(started.participants[0].display_name, "Bob");
}

#[tokio::test]
async fn reorder_seats_is_rejected_for_a_seated_non_creator() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;
    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/reorder-seats", waiting.game.id),
        Some(&waiting.bob.session_token),
        &api::SwapSeatsRequest {
            seat_a: 0,
            seat_b: 1,
        },
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "Bob holds a seat, but only the creator (Alice) may reorder"
    );
}

#[tokio::test]
async fn starting_a_game_is_rejected_for_a_seated_non_creator() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;
    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/start", waiting.game.id),
        Some(&waiting.bob.session_token),
        &StartGameRequest::default(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn add_seat_appends_a_not_sent_seat_and_is_creator_only() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;
    register_player(app.clone(), "Carol").await;

    let non_creator = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/seats", waiting.game.id),
        Some(&waiting.bob.session_token),
        &api::CreateSeatRequest {
            kind: SeatKind::Human,
            display_name: "Carol".to_string(),
            engine_id: None,
            claim: Some(SeatClaim::Named {
                display_name: "Carol".to_string(),
            }),
        },
    )
    .await;
    assert_eq!(non_creator.status(), StatusCode::UNAUTHORIZED);

    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/seats", waiting.game.id),
        Some(&waiting.alice.session_token),
        &api::CreateSeatRequest {
            kind: SeatKind::Human,
            display_name: "Carol".to_string(),
            engine_id: None,
            claim: Some(SeatClaim::Named {
                display_name: "Carol".to_string(),
            }),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(response).await;
    assert_eq!(updated.participants.len(), 3);
    let new_seat = &updated.participants[2];
    assert_eq!(new_seat.seat_number, 2);
    assert_eq!(new_seat.display_name, "Carol");
    assert_eq!(
        new_seat.invitation_status,
        Some(api::SeatInvitationStatus::NotSent),
        "a freshly added seat has no invitation yet"
    );
}

#[tokio::test]
async fn sending_the_invitation_for_an_added_seat_makes_it_pending_and_emails_them() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;
    register_player(app.clone(), "Carol").await;
    let added: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/seats", waiting.game.id),
            Some(&waiting.alice.session_token),
            &api::CreateSeatRequest {
                kind: SeatKind::Human,
                display_name: "Carol".to_string(),
                engine_id: None,
                claim: Some(SeatClaim::Named {
                    display_name: "Carol".to_string(),
                }),
            },
        )
        .await,
    )
    .await;

    let log = start_capturing_log_on_this_thread();

    let response = send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/invite", waiting.game.id),
        Some(&waiting.alice.session_token),
        &api::InvitePlayerRequest {
            invited_display_name: Some("Carol".to_string()),
            invited_email: None,
            seat_number: added.participants[2].seat_number,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let log_text = log.text();
    assert!(
        log_text.contains("carol@example.com"),
        "expected an email addressed to carol@example.com, got log:\n{log_text}"
    );

    let fetched: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::GET,
            &format!("/games/{}", waiting.game.id),
            Some(&waiting.alice.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        fetched.participants[2].invitation_status,
        Some(api::SeatInvitationStatus::Pending)
    );
}

#[tokio::test]
async fn remove_seat_deletes_an_unclaimed_seat_and_renumbers_the_rest() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;
    register_player(app.clone(), "Carol").await;
    // Alice(0, creator) / Bob(1, claimed) / Carol(2, not-sent) — remove
    // seat 1 (Bob) and confirm Carol shifts down to seat 1.
    send_json_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/seats", waiting.game.id),
        Some(&waiting.alice.session_token),
        &api::CreateSeatRequest {
            kind: SeatKind::Human,
            display_name: "Carol".to_string(),
            engine_id: None,
            claim: Some(SeatClaim::Named {
                display_name: "Carol".to_string(),
            }),
        },
    )
    .await;

    let response = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/seats/1/remove", waiting.game.id),
        Some(&waiting.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(response).await;
    assert_eq!(updated.participants.len(), 2);
    assert_eq!(updated.participants[1].display_name, "Carol");
    assert_eq!(updated.participants[1].seat_number, 1);
}

#[tokio::test]
async fn remove_seat_can_kick_an_already_claimed_seat_and_is_creator_only() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;

    let non_creator = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/seats/1/remove", waiting.game.id),
        Some(&waiting.bob.session_token),
    )
    .await;
    assert_eq!(non_creator.status(), StatusCode::UNAUTHORIZED);

    let response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/seats/1/remove", waiting.game.id),
        Some(&waiting.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(response).await;
    assert_eq!(
        updated.participants.len(),
        1,
        "Bob's claimed seat is gone entirely"
    );

    let creators_own_seat = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/seats/0/remove", waiting.game.id),
        Some(&waiting.alice.session_token),
    )
    .await;
    assert_eq!(
        creators_own_seat.status(),
        StatusCode::BAD_REQUEST,
        "the creator's own seat can't be removed"
    );
}

#[tokio::test]
async fn withdraw_clears_the_claim_and_flips_the_invitation_to_rejected() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;

    let not_the_seat_holder = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/seats/1/withdraw", waiting.game.id),
        Some(&waiting.alice.session_token),
    )
    .await;
    assert_eq!(not_the_seat_holder.status(), StatusCode::UNAUTHORIZED);

    let response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/seats/1/withdraw", waiting.game.id),
        Some(&waiting.bob.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(response).await;
    assert_eq!(updated.participants[1].player_id, None);
    assert_eq!(
        updated.participants[1].invitation_status,
        Some(api::SeatInvitationStatus::Rejected)
    );

    let creators_own_seat = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/seats/0/withdraw", waiting.game.id),
        Some(&waiting.alice.session_token),
    )
    .await;
    assert_eq!(creators_own_seat.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn force_resign_ends_the_game_regardless_of_whose_turn_it_is() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    assert_eq!(started.game.current_seat, 0, "it's Alice's (seat 0) turn");

    let non_creator = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/seats/1/force-resign", started.game.id),
        Some(&started.bob.session_token),
    )
    .await;
    assert_eq!(non_creator.status(), StatusCode::UNAUTHORIZED);

    // Alice force-resigns Bob (seat 1) even though it's currently seat
    // 0's turn — the whole point of force-resign over self-resign.
    let response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/seats/1/force-resign", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: GameStateDto = read_json(response).await;
    assert_eq!(updated.status, api::GameStatus::Finished);
    assert_eq!(updated.winner_seat, Some(0));

    let already_finished = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/seats/1/force-resign", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(
        already_finished.status(),
        StatusCode::BAD_REQUEST,
        "the game is already finished"
    );
}

#[tokio::test]
async fn reorder_seats_is_rejected_once_the_game_has_started() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/reorder-seats", started.game.id),
        Some(&started.alice.session_token),
        &api::SwapSeatsRequest {
            seat_a: 0,
            seat_b: 1,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reorder_seats_is_rejected_for_a_caller_with_no_claimed_seat() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;
    let stranger = register_player(app.clone(), "Stranger").await;
    let response = send_json_auth(
        app,
        Method::POST,
        &format!("/games/{}/reorder-seats", waiting.game.id),
        Some(&stranger.session_token),
        &api::SwapSeatsRequest {
            seat_a: 0,
            seat_b: 1,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_endpoints_reject_non_loopback_callers() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_admin::<()>(app, Method::GET, "/admin/users", remote_peer(), None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_can_list_and_delete_users() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice: PlayerSessionDto = read_json(
        send_json(
            app.clone(),
            Method::POST,
            "/auth/register",
            &RegisterPlayerRequest {
                display_name: "Alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "correct horse battery staple".to_string(),
                stay_logged_in: false,
            },
        )
        .await,
    )
    .await;

    let listed: Vec<PlayerDto> = read_json(
        send_admin::<()>(
            app.clone(),
            Method::GET,
            "/admin/users",
            loopback_peer(),
            None,
        )
        .await,
    )
    .await;
    assert!(listed.iter().any(|player| player.id == alice.player_id));

    let delete_response = send_admin::<()>(
        app.clone(),
        Method::DELETE,
        &format!("/admin/users/{}", alice.player_id),
        loopback_peer(),
        None,
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    let listed_after: Vec<PlayerDto> =
        read_json(send_admin::<()>(app, Method::GET, "/admin/users", loopback_peer(), None).await)
            .await;
    assert!(
        !listed_after
            .iter()
            .any(|player| player.id == alice.player_id)
    );
}

#[tokio::test]
async fn admin_deleting_a_user_unclaims_their_seat_but_keeps_the_game() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice: PlayerSessionDto = read_json(
        send_json(
            app.clone(),
            Method::POST,
            "/auth/register",
            &RegisterPlayerRequest {
                display_name: "Alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "correct horse battery staple".to_string(),
                stay_logged_in: false,
            },
        )
        .await,
    )
    .await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        claim: None,
                    },
                ],
                seed: Some(7),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert_eq!(
        created.participants[0].player_id.as_deref(),
        Some(alice.player_id.as_str())
    );

    let delete_response = send_admin::<()>(
        app.clone(),
        Method::DELETE,
        &format!("/admin/users/{}", alice.player_id),
        loopback_peer(),
        None,
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // Alice's session was deleted along with her account, and nobody
    // else is tied to this game (the seat is now unclaimed, and admin
    // deletion doesn't touch `creator_player_id` — see its own doc
    // comment), so there's no longer a legitimate caller who could
    // fetch it through the normal player-facing endpoint. Assert
    // directly on the in-memory state instead, which is also a more
    // direct check of what this test actually cares about.
    let games = state.games.read().await;
    let fetched = games
        .get(&created.id)
        .expect("the game itself should survive");
    assert_eq!(fetched.id, created.id);
    assert_eq!(
        fetched.participants[0].player_id, None,
        "the seat should be unclaimed, not still pointing at a deleted player"
    );
}

#[tokio::test]
async fn admin_can_reset_a_password() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    read_json::<PlayerSessionDto>(
        send_json(
            app.clone(),
            Method::POST,
            "/auth/register",
            &RegisterPlayerRequest {
                display_name: "Alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "original-password".to_string(),
                stay_logged_in: false,
            },
        )
        .await,
    )
    .await;

    let reset_response = send_admin(
        app.clone(),
        Method::POST,
        "/admin/users/whoever/reset-password",
        loopback_peer(),
        Some(&AdminResetPasswordRequest {
            new_password: "brand-new-password".to_string(),
        }),
    )
    .await;
    // Wrong id on purpose first, to check a bad id 404s rather than
    // silently succeeding.
    assert_eq!(reset_response.status(), StatusCode::NOT_FOUND);

    let listed: Vec<PlayerDto> = read_json(
        send_admin::<()>(
            app.clone(),
            Method::GET,
            "/admin/users",
            loopback_peer(),
            None,
        )
        .await,
    )
    .await;
    let alice_id = listed
        .iter()
        .find(|player| player.display_name == "Alice")
        .expect("Alice should be listed")
        .id
        .clone();

    let reset_response = send_admin(
        app.clone(),
        Method::POST,
        &format!("/admin/users/{alice_id}/reset-password"),
        loopback_peer(),
        Some(&AdminResetPasswordRequest {
            new_password: "brand-new-password".to_string(),
        }),
    )
    .await;
    assert_eq!(reset_response.status(), StatusCode::NO_CONTENT);

    let old_password_login = send_json(
        app.clone(),
        Method::POST,
        "/auth/login",
        &LoginPlayerRequest {
            display_name: "Alice".to_string(),
            password: "original-password".to_string(),
            stay_logged_in: false,
        },
    )
    .await;
    assert_eq!(old_password_login.status(), StatusCode::BAD_REQUEST);

    let new_password_login = send_json(
        app,
        Method::POST,
        "/auth/login",
        &LoginPlayerRequest {
            display_name: "Alice".to_string(),
            password: "brand-new-password".to_string(),
            stay_logged_in: false,
        },
    )
    .await;
    assert_eq!(new_password_login.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_can_list_and_delete_games() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let created = create_two_human_game(app.clone()).await;

    let listed: Vec<AdminGameSummaryDto> = read_json(
        send_admin::<()>(
            app.clone(),
            Method::GET,
            "/admin/games",
            loopback_peer(),
            None,
        )
        .await,
    )
    .await;
    let listed_game = listed
        .iter()
        .find(|game| game.id == created.game.id)
        .expect("created game should be listed");
    assert!(listed_game.created_at > 0);

    let delete_response = send_admin::<()>(
        app.clone(),
        Method::DELETE,
        &format!("/admin/games/{}", created.game.id),
        loopback_peer(),
        None,
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    let fetch_after = send_empty(
        app.clone(),
        Method::GET,
        &format!("/games/{}", created.game.id),
    )
    .await;
    assert_eq!(fetch_after.status(), StatusCode::NOT_FOUND);

    let listed_after: Vec<AdminGameSummaryDto> =
        read_json(send_admin::<()>(app, Method::GET, "/admin/games", loopback_peer(), None).await)
            .await;
    assert!(!listed_after.iter().any(|game| game.id == created.game.id));
}

#[tokio::test]
async fn admin_can_force_end_a_stuck_game() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let started = create_two_human_game(app.clone()).await;
    assert_eq!(started.game.status, api::GameStatus::Active);

    let response = send_admin::<()>(
        app.clone(),
        Method::POST,
        &format!("/admin/games/{}/force-end", started.game.id),
        loopback_peer(),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let ended: GameStateDto = read_json(response).await;
    assert_eq!(ended.status, api::GameStatus::Finished);

    let fetched: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::GET,
            &format!("/games/{}", started.game.id),
            Some(&started.alice.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(fetched.status, api::GameStatus::Finished);
}

#[tokio::test]
async fn creating_a_game_with_an_unknown_named_invitee_is_rejected() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let response = send_json_auth(
        app,
        Method::POST,
        "/games",
        Some(&alice.session_token),
        &CreateGameRequest {
            seats: vec![
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Alice".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Creator),
                },
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Nobody".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Named {
                        display_name: "Nobody".to_string(),
                    }),
                },
            ],
            seed: Some(1),
            variant: None,
            language: None,
            board_layout: None,
            move_time_limit_seconds: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Captures everything logged via `tracing` on the calling thread while
/// its `CapturedLog` is registered. Used to verify `crate::email::send_*`
/// actually fired — with no `RESEND_API_KEY` configured in tests, a send
/// logs the full message (see `EmailConfig`'s doc comment) instead of
/// calling out over the network, which this reads back instead of
/// standing up a mock server.
///
/// Installs exactly one global `tracing` subscriber for the whole test
/// binary (via `Once`), rather than each test installing its own with
/// `tracing::subscriber::set_default`. `tracing`'s callsite "interest" is
/// cached process-wide, not per-thread: with `cargo test`'s default
/// parallelism, one test's `set_default`/drop churn can flip another
/// concurrently-running test's callsite back to "nobody's interested"
/// mid-request, silently swallowing the exact log line being asserted
/// on — this was observed as intermittent, full-suite-only failures of
/// `creating_a_game_with_a_named_invitee_emails_them` with an empty or
/// partial captured log. A single subscriber installed once means
/// interest is computed once and never invalidated; routing a given
/// thread's output to that thread's own test is done with an ordinary
/// (non-tracing) `thread_local!`, which the standard test harness's
/// one-OS-thread-per-test model keeps cleanly isolated.
#[derive(Clone, Default)]
struct CapturedLog(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl CapturedLog {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).expect("log output should be utf8")
    }
}

thread_local! {
    static LOG_SINK: std::cell::RefCell<Option<CapturedLog>> = const { std::cell::RefCell::new(None) };
}

struct ThreadLocalWriter;

impl std::io::Write for ThreadLocalWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        LOG_SINK.with(|sink| {
            if let Some(log) = sink.borrow().as_ref() {
                log.0.lock().unwrap().extend_from_slice(buf);
            }
        });
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ThreadLocalWriter {
    type Writer = ThreadLocalWriter;
    fn make_writer(&'a self) -> Self::Writer {
        ThreadLocalWriter
    }
}

/// Guard returned by `start_capturing_log_on_this_thread`: reads back
/// with `.text()`, and clears this thread's capture slot on drop
/// (including on an assertion panic) so a later test whose OS thread
/// gets reused by the harness never inherits a stale sink.
struct LogCapture(CapturedLog);

impl LogCapture {
    fn text(&self) -> String {
        self.0.text()
    }
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        LOG_SINK.with(|sink| *sink.borrow_mut() = None);
    }
}

/// Starts capturing this thread's `tracing` output, readable via the
/// returned guard's `.text()` once the request under test has completed.
fn start_capturing_log_on_this_thread() -> LogCapture {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(ThreadLocalWriter)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("global default should only be installed once per test binary");
    });
    let log = CapturedLog::default();
    LOG_SINK.with(|sink| *sink.borrow_mut() = Some(log.clone()));
    LogCapture(log)
}

/// This is a regression test for a real bug found in production
/// 2026-07-18: `invite_player_to_game` had email wiring, but
/// `create_game` — the *only* path the UI's initial draft builder
/// actually calls, and so the only place a named invitation is created
/// in practice — didn't, so nobody who got invited into a brand-new
/// game ever received an email, only someone invited into an
/// already-existing one (a code path the UI doesn't expose at all).
#[tokio::test]
async fn creating_a_game_with_a_named_invitee_emails_them() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;
    register_player(app.clone(), "Bob").await;

    let log = start_capturing_log_on_this_thread();

    let response = send_json_auth(
        app,
        Method::POST,
        "/games",
        Some(&alice.session_token),
        &CreateGameRequest {
            seats: vec![
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Alice".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Creator),
                },
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Bob".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Named {
                        display_name: "Bob".to_string(),
                    }),
                },
            ],
            seed: Some(1),
            variant: None,
            language: None,
            board_layout: None,
            move_time_limit_seconds: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let log_text = log.text();
    assert!(
        log_text.contains("bob@example.com"),
        "expected an email addressed to bob@example.com, got log:\n{log_text}"
    );
    assert!(
        log_text.contains("invited you"),
        "expected the invitation email's subject/body, got log:\n{log_text}"
    );
}

#[tokio::test]
async fn creating_a_game_with_an_email_invitee_emails_a_join_link() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let log = start_capturing_log_on_this_thread();

    let response = send_json_auth(
        app,
        Method::POST,
        "/games",
        Some(&alice.session_token),
        &CreateGameRequest {
            seats: vec![
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Alice".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Creator),
                },
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "carol@example.com".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Email {
                        email: "carol@example.com".to_string(),
                    }),
                },
            ],
            seed: Some(1),
            variant: None,
            language: None,
            board_layout: None,
            move_time_limit_seconds: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let log_text = log.text();
    assert!(
        log_text.contains("carol@example.com"),
        "expected an email addressed to carol@example.com, got log:\n{log_text}"
    );
    assert!(
        log_text.contains("/invite?id="),
        "expected a join link in the email body, got log:\n{log_text}"
    );
}

#[tokio::test]
async fn move_time_reminder_fires_once_when_a_long_limit_game_runs_low_on_time() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    // Default move_time_limit_seconds is 72h; back-date the turn's start
    // so only ~1000s (well under the 24h/third-of-72h reminder
    // threshold) remain on Alice's (seat 0) turn.
    let started = create_two_human_game(app.clone()).await;
    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&started.game.id).expect("game should exist");
        let remaining: i64 = 1_000;
        game.turn_started_at = now_unix_seconds() + remaining - game.move_time_limit_seconds as i64;
    }

    let log = start_capturing_log_on_this_thread();

    let response = send_empty_auth(
        app.clone(),
        Method::GET,
        "/games",
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let log_text = log.text();
    assert!(
        log_text.contains("alice@example.com"),
        "expected a reminder emailed to alice@example.com, got log:\n{log_text}"
    );
    assert!(
        log_text.contains("move is due soon"),
        "expected the move-reminder email's subject, got log:\n{log_text}"
    );

    // A second sweep on the same turn must not send a second reminder.
    let response = send_empty_auth(
        app,
        Method::GET,
        "/games",
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let occurrences = log.text().matches("move is due soon").count();
    assert_eq!(
        occurrences,
        1,
        "reminder should fire at most once per turn, got log:\n{}",
        log.text()
    );
}

#[tokio::test]
async fn move_time_reminder_does_not_fire_for_a_same_day_limit_game() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;
    let bob = register_player(app.clone(), "Bob").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Open seat".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Open),
                    },
                ],
                seed: Some(42),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: Some(3_600),
            },
        )
        .await,
    )
    .await;

    let bob_games: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(app.clone(), Method::GET, "/games", Some(&bob.session_token)).await,
    )
    .await;
    let invitation = bob_games
        .iter()
        .find(|summary| summary.id == created.id)
        .expect("the open seat should appear in Bob's games list")
        .invitation_id
        .clone()
        .expect("an invited-open entry should carry an invitation id");
    let accept_response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/invitations/{invitation}/accept"),
        Some(&bob.session_token),
    )
    .await;
    assert_eq!(accept_response.status(), StatusCode::OK);

    let started: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/start", created.id),
            Some(&alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;
    assert_eq!(started.status, api::GameStatus::Active);

    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&started.id).expect("game should exist");
        game.turn_started_at = now_unix_seconds() - game.move_time_limit_seconds as i64 + 30;
    }

    let log = start_capturing_log_on_this_thread();
    let response = send_empty_auth(app, Method::GET, "/games", Some(&alice.session_token)).await;
    assert_eq!(response.status(), StatusCode::OK);

    let log_text = log.text();
    assert!(
        !log_text.contains("move is due soon"),
        "a same-day move-time-limit game should never send a reminder, got log:\n{log_text}"
    );
}

/// Regression guard for the reason `SeatClaim::Email` doesn't just reuse
/// `SeatClaim::Open`: without the `invited_email is null` clause in
/// `get_open_invitations`, this exact scenario would let Mallory —
/// nobody Alice ever addressed — see and snipe the seat before
/// carol@example.com even opens her inbox.
#[tokio::test]
async fn email_invitation_does_not_appear_as_a_generic_open_invitation() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;
    let mallory = register_player(app.clone(), "Mallory").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "carol@example.com".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Email {
                            email: "carol@example.com".to_string(),
                        }),
                    },
                ],
                seed: Some(1),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    let mallory_games: Vec<api::GameSummaryDto> =
        read_json(send_empty_auth(app, Method::GET, "/games", Some(&mallory.session_token)).await)
            .await;
    assert!(
        mallory_games.iter().all(|summary| summary.id != created.id),
        "an email invitation must not be visible as a generic open invitation"
    );
}

#[tokio::test]
async fn invitation_preview_returns_the_inviter_and_status_without_auth() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let db = state.db.clone();
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;
    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "carol@example.com".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Email {
                            email: "carol@example.com".to_string(),
                        }),
                    },
                ],
                seed: Some(1),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    let invitations = persistence::get_invitations_for_game(&db, &created.id)
        .await
        .expect("invitations should load");
    let invitation_id = invitations.first().expect("one invitation").id.clone();

    // Deliberately no auth header — this is reached before the visitor
    // has necessarily registered or logged in.
    let response = send_empty(
        app,
        Method::GET,
        &format!("/invitations/{invitation_id}/preview"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let preview: api::InvitationPreviewDto = read_json(response).await;
    assert_eq!(preview.inviting_player_display_name, "Alice");
    assert_eq!(preview.status, api::InvitationStatus::Pending);
}

#[tokio::test]
async fn invitation_preview_404s_for_an_unknown_id() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_empty(app, Method::GET, "/invitations/does-not-exist/preview").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn accepting_an_email_invitation_claims_the_seat_like_an_open_one() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let db = state.db.clone();
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;
    let carol = register_player(app.clone(), "Carol").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "carol@example.com".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Email {
                            email: "carol@example.com".to_string(),
                        }),
                    },
                ],
                seed: Some(1),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    let invitations = persistence::get_invitations_for_game(&db, &created.id)
        .await
        .expect("invitations should load");
    let invitation_id = invitations.first().expect("one invitation").id.clone();

    let response = send_empty_auth(
        app,
        Method::POST,
        &format!("/invitations/{invitation_id}/accept"),
        Some(&carol.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let joined: GameStateDto = read_json(response).await;
    assert_eq!(joined.participants[1].display_name, "Carol");
    assert_eq!(
        joined.participants[1].player_id.as_deref(),
        Some(carol.player_id.as_str())
    );
}

#[tokio::test]
async fn only_one_creator_seat_is_allowed() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let response = send_json_auth(
        app,
        Method::POST,
        "/games",
        Some(&alice.session_token),
        &CreateGameRequest {
            seats: vec![
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Alice".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Creator),
                },
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Also Alice?".to_string(),
                    engine_id: None,
                    claim: Some(SeatClaim::Creator),
                },
            ],
            seed: Some(1),
            variant: None,
            language: None,
            board_layout: None,
            move_time_limit_seconds: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn named_invitation_shows_up_and_claims_the_seat_on_accept() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;
    let bob = register_player(app.clone(), "Bob").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Bob".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Named {
                            display_name: "Bob".to_string(),
                        }),
                    },
                ],
                seed: Some(1),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert_eq!(created.participants[1].player_id, None);

    let bob_games: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(app.clone(), Method::GET, "/games", Some(&bob.session_token)).await,
    )
    .await;
    let summary = bob_games
        .iter()
        .find(|summary| summary.id == created.id)
        .expect("Bob should see the game he was named-invited to");
    assert_eq!(summary.relationship, api::GameRelationship::InvitedByName);
    let invitation_id = summary.invitation_id.clone().expect("invitation id");

    // A stranger who wasn't invited can't accept Bob's named invitation.
    let mallory = register_player(app.clone(), "Mallory").await;
    let stolen = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/invitations/{invitation_id}/accept"),
        Some(&mallory.session_token),
    )
    .await;
    assert_eq!(stolen.status(), StatusCode::UNAUTHORIZED);

    let accepted: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::POST,
            &format!("/invitations/{invitation_id}/accept"),
            Some(&bob.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        accepted.participants[1].player_id.as_deref(),
        Some(bob.player_id.as_str())
    );
}

#[tokio::test]
async fn open_invitation_is_claimed_by_only_the_first_acceptor() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;
    let bob = register_player(app.clone(), "Bob").await;
    let mallory = register_player(app.clone(), "Mallory").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Open seat".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Open),
                    },
                ],
                seed: Some(1),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    let mallory_games: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(
            app.clone(),
            Method::GET,
            "/games",
            Some(&mallory.session_token),
        )
        .await,
    )
    .await;
    let invitation_id = mallory_games
        .iter()
        .find(|summary| summary.id == created.id)
        .expect("the open seat should be visible to any signed-in player")
        .invitation_id
        .clone()
        .expect("invitation id");

    let bob_accept = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/invitations/{invitation_id}/accept"),
        Some(&bob.session_token),
    )
    .await;
    assert_eq!(bob_accept.status(), StatusCode::OK);

    // Mallory loses the race — the seat is already Bob's.
    let mallory_accept = send_empty_auth(
        app,
        Method::POST,
        &format!("/invitations/{invitation_id}/accept"),
        Some(&mallory.session_token),
    )
    .await;
    assert_eq!(mallory_accept.status(), StatusCode::BAD_REQUEST);
}

/// Regression test: `accept_invitation` used to only set `player_id`,
/// leaving an open seat's placeholder "Open seat" display name in place
/// forever even after someone genuinely claimed it.
#[tokio::test]
async fn accepting_an_open_invitation_replaces_the_placeholder_display_name() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;
    let bob = register_player(app.clone(), "Bob").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Open seat".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Open),
                    },
                ],
                seed: Some(1),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;
    assert_eq!(created.participants[1].display_name, "Open seat");

    let bob_games: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(app.clone(), Method::GET, "/games", Some(&bob.session_token)).await,
    )
    .await;
    let invitation_id = bob_games
        .iter()
        .find(|summary| summary.id == created.id)
        .expect("invitation id")
        .invitation_id
        .clone()
        .expect("invitation id");

    let accepted: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::POST,
            &format!("/invitations/{invitation_id}/accept"),
            Some(&bob.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(accepted.participants[1].display_name, "Bob");
    assert_eq!(
        accepted.participants[1].player_id.as_deref(),
        Some(bob.player_id.as_str())
    );
}

#[tokio::test]
async fn rejecting_a_named_invitation_removes_it_without_claiming_the_seat() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;
    let bob = register_player(app.clone(), "Bob").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Bob".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Named {
                            display_name: "Bob".to_string(),
                        }),
                    },
                ],
                seed: Some(1),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: None,
            },
        )
        .await,
    )
    .await;

    let bob_games: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(app.clone(), Method::GET, "/games", Some(&bob.session_token)).await,
    )
    .await;
    let invitation_id = bob_games
        .iter()
        .find(|summary| summary.id == created.id)
        .expect("Bob should see the invitation")
        .invitation_id
        .clone()
        .expect("invitation id");

    let reject_response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/invitations/{invitation_id}/reject"),
        Some(&bob.session_token),
    )
    .await;
    assert_eq!(reject_response.status(), StatusCode::OK);

    let fetched: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::GET,
            &format!("/games/{}", created.id),
            Some(&alice.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(fetched.participants[1].player_id, None);
}

#[tokio::test]
async fn overdue_turn_is_auto_retired_on_next_access() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;
    let bob = register_player(app.clone(), "Bob").await;

    let created: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            "/games",
            Some(&alice.session_token),
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Creator),
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Bob".to_string(),
                        engine_id: None,
                        claim: Some(SeatClaim::Named {
                            display_name: "Bob".to_string(),
                        }),
                    },
                ],
                seed: Some(1),
                variant: None,
                language: None,
                board_layout: None,
                move_time_limit_seconds: Some(60),
            },
        )
        .await,
    )
    .await;

    let bob_games: Vec<api::GameSummaryDto> = read_json(
        send_empty_auth(app.clone(), Method::GET, "/games", Some(&bob.session_token)).await,
    )
    .await;
    let invitation_id = bob_games
        .iter()
        .find(|summary| summary.id == created.id)
        .expect("invitation id")
        .invitation_id
        .clone()
        .expect("invitation id");
    send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/invitations/{invitation_id}/accept"),
        Some(&bob.session_token),
    )
    .await;

    let started: GameStateDto = read_json(
        send_json_auth(
            app.clone(),
            Method::POST,
            &format!("/games/{}/start", created.id),
            Some(&alice.session_token),
            &StartGameRequest::default(),
        )
        .await,
    )
    .await;
    assert_eq!(started.move_time_limit_seconds, 60);
    assert_eq!(started.current_seat, 0);

    // Rewind the in-memory turn clock rather than sleeping 60 real
    // seconds in a test.
    {
        let mut games = state.games.write().await;
        let game = games.get_mut(&created.id).expect("game should exist");
        game.turn_started_at = 0;
    }

    let fetched: GameStateDto = read_json(
        send_empty_auth(
            app,
            Method::GET,
            &format!("/games/{}", created.id),
            Some(&alice.session_token),
        )
        .await,
    )
    .await;
    assert_eq!(fetched.status, api::GameStatus::Finished);
    assert_eq!(fetched.winner_seat, Some(1));
    let last_move = fetched
        .moves
        .last()
        .expect("a timeout move should be recorded");
    assert_eq!(last_move.move_type, "timeout");
    assert_eq!(last_move.seat_number, 0);
}

#[tokio::test]
async fn change_password_requires_auth() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_json(
        app,
        Method::POST,
        "/auth/change-password",
        &api::ChangePasswordRequest {
            current_password: "correcthorsebatterystaple".to_string(),
            new_password: "new-password-entirely".to_string(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn change_password_rejects_a_wrong_current_password() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let response = send_json_auth(
        app,
        Method::POST,
        "/auth/change-password",
        Some(&alice.session_token),
        &api::ChangePasswordRequest {
            current_password: "totally-the-wrong-password".to_string(),
            new_password: "new-password-entirely".to_string(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn change_password_succeeds_and_signs_out_existing_sessions() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let response = send_json_auth(
        app.clone(),
        Method::POST,
        "/auth/change-password",
        Some(&alice.session_token),
        &api::ChangePasswordRequest {
            current_password: "correct horse battery staple".to_string(),
            new_password: "a brand new password".to_string(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // The session used to make the change is itself invalidated —
    // changing your password means starting fresh, not silently
    // keeping whatever session made the request.
    let old_session_response = send_empty_auth(
        app.clone(),
        Method::GET,
        "/games",
        Some(&alice.session_token),
    )
    .await;
    assert_eq!(old_session_response.status(), StatusCode::UNAUTHORIZED);

    // The old password no longer works...
    let old_login = send_json(
        app.clone(),
        Method::POST,
        "/auth/login",
        &LoginPlayerRequest {
            display_name: "Alice".to_string(),
            password: "correct horse battery staple".to_string(),
            stay_logged_in: false,
        },
    )
    .await;
    assert_eq!(old_login.status(), StatusCode::BAD_REQUEST);

    // ...but the new one does.
    let new_login = send_json(
        app,
        Method::POST,
        "/auth/login",
        &LoginPlayerRequest {
            display_name: "Alice".to_string(),
            password: "a brand new password".to_string(),
            stay_logged_in: false,
        },
    )
    .await;
    assert_eq!(new_login.status(), StatusCode::OK);
}

#[tokio::test]
async fn update_player_details_requires_auth() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_json(
        app,
        Method::POST,
        "/auth/update-details",
        &api::UpdatePlayerDetailsRequest {
            display_name: Some("New Name".to_string()),
            email: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn update_player_details_changes_display_name_and_email_without_touching_the_session() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let response = send_json_auth(
        app.clone(),
        Method::POST,
        "/auth/update-details",
        Some(&alice.session_token),
        &api::UpdatePlayerDetailsRequest {
            display_name: Some("Alicia".to_string()),
            email: Some("alicia@example.com".to_string()),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: api::PlayerDto = read_json(response).await;
    assert_eq!(updated.display_name, "Alicia");
    assert_eq!(updated.email, "alicia@example.com");

    // Unlike a password change, the session used to make the request
    // stays valid — no re-login required.
    let still_valid = send_empty_auth(app, Method::GET, "/games", Some(&alice.session_token)).await;
    assert_eq!(still_valid.status(), StatusCode::OK);
}

#[tokio::test]
async fn update_player_details_rejects_a_taken_display_name() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    register_player(app.clone(), "Bob").await;
    let alice = register_player(app.clone(), "Alice").await;

    let response = send_json_auth(
        app,
        Method::POST,
        "/auth/update-details",
        Some(&alice.session_token),
        &api::UpdatePlayerDetailsRequest {
            display_name: Some("Bob".to_string()),
            email: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn update_player_details_allows_keeping_your_own_display_name() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    // Setting display_name to the value it already is shouldn't trip
    // the "taken" check against yourself.
    let response = send_json_auth(
        app,
        Method::POST,
        "/auth/update-details",
        Some(&alice.session_token),
        &api::UpdatePlayerDetailsRequest {
            display_name: Some("Alice".to_string()),
            email: Some("alice-new@example.com".to_string()),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn update_player_details_rejects_a_blank_display_name_and_a_request_with_nothing_to_update() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let alice = register_player(app.clone(), "Alice").await;

    let blank = send_json_auth(
        app.clone(),
        Method::POST,
        "/auth/update-details",
        Some(&alice.session_token),
        &api::UpdatePlayerDetailsRequest {
            display_name: Some("   ".to_string()),
            email: None,
        },
    )
    .await;
    assert_eq!(blank.status(), StatusCode::BAD_REQUEST);

    let empty = send_json_auth(
        app,
        Method::POST,
        "/auth/update-details",
        Some(&alice.session_token),
        &api::UpdatePlayerDetailsRequest {
            display_name: None,
            email: None,
        },
    )
    .await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn forgot_password_returns_no_content_whether_or_not_the_email_is_registered() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    register_player(app.clone(), "Alice").await;

    let known = send_json(
        app.clone(),
        Method::POST,
        "/auth/forgot-password",
        &RequestPasswordResetRequest {
            email: "alice@example.com".to_string(),
        },
    )
    .await;
    assert_eq!(known.status(), StatusCode::NO_CONTENT);

    // Same response for an email nobody registered — an attacker
    // probing this endpoint can't use the response to tell accounts
    // apart from non-accounts.
    let unknown = send_json(
        app,
        Method::POST,
        "/auth/forgot-password",
        &RequestPasswordResetRequest {
            email: "nobody@example.com".to_string(),
        },
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn forgot_password_issues_exactly_one_live_token_and_retires_earlier_ones() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    register_player(app.clone(), "Alice").await;
    let alice = persistence::get_player_by_email(&state.db, "alice@example.com")
        .await
        .expect("query should succeed")
        .expect("alice should exist");

    send_json(
        app.clone(),
        Method::POST,
        "/auth/forgot-password",
        &RequestPasswordResetRequest {
            email: "alice@example.com".to_string(),
        },
    )
    .await;
    let first_token_id = {
        let rows = sqlx::query("select id from password_reset_tokens where player_id = ?1")
            .bind(&alice.id)
            .fetch_all(&state.db)
            .await
            .expect("query should succeed");
        assert_eq!(rows.len(), 1, "exactly one token after the first request");
        rows[0].get::<String, _>(0)
    };

    // Requesting again retires the first token rather than leaving both
    // valid — only the newest emailed link should ever work.
    send_json(
        app,
        Method::POST,
        "/auth/forgot-password",
        &RequestPasswordResetRequest {
            email: "alice@example.com".to_string(),
        },
    )
    .await;
    let rows = sqlx::query("select id from password_reset_tokens where player_id = ?1")
        .bind(&alice.id)
        .fetch_all(&state.db)
        .await
        .expect("query should succeed");
    assert_eq!(
        rows.len(),
        1,
        "still exactly one token after the second request"
    );
    assert_ne!(
        rows[0].get::<String, _>(0),
        first_token_id,
        "the second request's token should replace, not join, the first"
    );
}

#[tokio::test]
async fn reset_password_rejects_an_unknown_token() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state);

    let response = send_json(
        app,
        Method::POST,
        "/auth/reset-password",
        &ResetPasswordRequest {
            token: "not-a-real-token".to_string(),
            new_password: "whatever new password".to_string(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reset_password_rejects_an_expired_token() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;
    let token = Uuid::new_v4().to_string();
    persistence::create_password_reset_token(
        &state.db,
        &Uuid::new_v4().to_string(),
        &alice.player_id,
        &hash_token(&token),
        0, // already expired (epoch second 0)
    )
    .await
    .expect("token should be created");

    let response = send_json(
        app,
        Method::POST,
        "/auth/reset-password",
        &ResetPasswordRequest {
            token,
            new_password: "whatever new password".to_string(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reset_password_rejects_a_token_already_used_once() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;
    let token = Uuid::new_v4().to_string();
    let far_future = now_unix_seconds() + 3600;
    persistence::create_password_reset_token(
        &state.db,
        &Uuid::new_v4().to_string(),
        &alice.player_id,
        &hash_token(&token),
        far_future,
    )
    .await
    .expect("token should be created");

    let first = send_json(
        app.clone(),
        Method::POST,
        "/auth/reset-password",
        &ResetPasswordRequest {
            token: token.clone(),
            new_password: "first new password".to_string(),
        },
    )
    .await;
    assert_eq!(first.status(), StatusCode::NO_CONTENT);

    // Re-presenting the same (now-consumed) token — e.g. a second click
    // on the same emailed link, or an attacker replaying an
    // intercepted-but-already-used one — must not work a second time.
    let second = send_json(
        app,
        Method::POST,
        "/auth/reset-password",
        &ResetPasswordRequest {
            token,
            new_password: "second new password".to_string(),
        },
    )
    .await;
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reset_password_succeeds_updates_password_and_signs_out_existing_sessions() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let alice = register_player(app.clone(), "Alice").await;
    let token = Uuid::new_v4().to_string();
    let far_future = now_unix_seconds() + 3600;
    persistence::create_password_reset_token(
        &state.db,
        &Uuid::new_v4().to_string(),
        &alice.player_id,
        &hash_token(&token),
        far_future,
    )
    .await
    .expect("token should be created");

    let response = send_json(
        app.clone(),
        Method::POST,
        "/auth/reset-password",
        &ResetPasswordRequest {
            token,
            new_password: "reset via emailed token".to_string(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // The session that existed before the reset (from registering) no
    // longer works — a reset should mean starting fresh, same as a
    // self-service change-password.
    let old_session = send_empty_auth(
        app.clone(),
        Method::GET,
        "/games",
        Some(&alice.session_token),
    )
    .await;
    assert_eq!(old_session.status(), StatusCode::UNAUTHORIZED);

    let old_login = send_json(
        app.clone(),
        Method::POST,
        "/auth/login",
        &LoginPlayerRequest {
            display_name: "Alice".to_string(),
            password: "correct horse battery staple".to_string(),
            stay_logged_in: false,
        },
    )
    .await;
    assert_eq!(old_login.status(), StatusCode::BAD_REQUEST);

    let new_login = send_json(
        app,
        Method::POST,
        "/auth/login",
        &LoginPlayerRequest {
            display_name: "Alice".to_string(),
            password: "reset via emailed token".to_string(),
            stay_logged_in: false,
        },
    )
    .await;
    assert_eq!(new_login.status(), StatusCode::OK);
}

#[tokio::test]
async fn creator_can_abort_an_active_game() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;

    // A non-creator can't abort.
    let non_creator = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/abort", started.game.id),
        Some(&started.bob.session_token),
    )
    .await;
    assert_eq!(non_creator.status(), StatusCode::UNAUTHORIZED);

    // The creator aborts the whole game.
    let response = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/abort", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let aborted: GameStateDto = read_json(response).await;
    assert_eq!(aborted.status, api::GameStatus::Aborted);
    assert_eq!(aborted.winner_seat, None, "an abort has no winner");
    assert!(
        aborted.participants.iter().all(|p| p.resigned),
        "every seat is force-resigned by an abort"
    );
    assert_eq!(
        aborted.moves.last().expect("a terminal move").move_type,
        "abort"
    );

    // Aborting again is rejected — the game has already ended.
    let again = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/abort", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(again.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn creator_can_abort_a_waiting_game() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let waiting = create_two_human_game_waiting(app.clone()).await;
    assert_eq!(waiting.game.status, api::GameStatus::Waiting);

    let response = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/abort", waiting.game.id),
        Some(&waiting.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let aborted: GameStateDto = read_json(response).await;
    assert_eq!(aborted.status, api::GameStatus::Aborted);
}

#[tokio::test]
async fn aborting_a_game_does_not_move_rating() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let response = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/abort", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let aborted: GameStateDto = read_json(response).await;
    assert_eq!(aborted.status, api::GameStatus::Aborted);
    for participant in &aborted.participants {
        assert_eq!(
            participant.rating_before, None,
            "an aborted game must not move rating"
        );
        assert_eq!(participant.rating_after, None);
    }

    let alice_stats = stats::get_subject_stats(&state.db, "player", &started.alice.player_id)
        .await
        .expect("stats query should succeed");
    assert_eq!(alice_stats.games_rated, 0);
}

#[tokio::test]
async fn an_aborted_game_reloads_from_its_snapshot() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    let started = create_two_human_game(app.clone()).await;
    let response = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/abort", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // Reload straight from the DB (bypassing the in-memory cache) to confirm
    // the `aborted` status round-trips through the snapshot blob.
    let reloaded = persistence::load_game(&state.db, &started.game.id)
        .await
        .expect("load ok")
        .expect("aborted game still loads");
    assert_eq!(reloaded.status, api::GameStatus::Aborted);
    assert!(reloaded.participants.iter().all(|p| p.resigned));
}

#[tokio::test]
async fn a_finished_game_cannot_be_aborted() {
    let database_url = test_database_url();
    let state = create_test_state(&database_url).await;
    let app = build_router(state.clone());

    // Finish the game: the creator force-resigns the only other seat, leaving
    // one active seat, which ends the game as Finished.
    let started = create_two_human_game(app.clone()).await;
    let finish = send_empty_auth(
        app.clone(),
        Method::POST,
        &format!("/games/{}/seats/1/force-resign", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(finish.status(), StatusCode::OK);
    let finished: GameStateDto = read_json(finish).await;
    assert_eq!(finished.status, api::GameStatus::Finished);

    // Abort must not overwrite a real result.
    let response = send_empty_auth(
        app,
        Method::POST,
        &format!("/games/{}/abort", started.game.id),
        Some(&started.alice.session_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
