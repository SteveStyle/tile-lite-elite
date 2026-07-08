use std::collections::HashMap;
use std::sync::Arc;

use api::{
    ApiError, CreateGameRequest, GameActionRequest, GameEventDto, PlayerActionDto, StartGameRequest,
};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sqlx::{Pool, Sqlite};
use tokio::sync::{RwLock, broadcast};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

use crate::game_state::{
    EngineRegistry, GameSession, ParticipantState, move_candidate_from_dto, tile_from_dto,
};
use crate::persistence;

#[derive(Clone)]
pub struct AppState {
    pub db: Pool<Sqlite>,
    pub games: Arc<RwLock<HashMap<String, GameSession>>>,
    pub events: broadcast::Sender<GameEventDto>,
    pub engines: EngineRegistry,
}

impl AppState {
    pub async fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        let db = persistence::connect(database_url).await?;
        let engines = EngineRegistry::default();
        persistence::upsert_engine_profiles(&db, &engines.metadata()).await?;

        let mut games = HashMap::new();
        for id in persistence::list_game_ids(&db).await? {
            if let Some(game) = persistence::load_game(&db, &id).await? {
                games.insert(id, game);
            }
        }

        let (events, _) = broadcast::channel(64);

        Ok(Self {
            db,
            games: Arc::new(RwLock::new(games)),
            events,
            engines,
        })
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/engines", get(list_engines))
        .route("/games", post(create_game).get(list_games))
        .route("/games/{game_id}", get(get_game))
        .route("/games/{game_id}/start", post(start_game))
        .route("/games/{game_id}/actions", post(submit_action))
        .route("/games/{game_id}/events", get(game_events))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn list_engines(State(state): State<AppState>) -> Json<Vec<api::EngineProfileDto>> {
    Json(state.engines.metadata())
}

async fn list_games(State(state): State<AppState>) -> Json<Vec<String>> {
    let games = state.games.read().await;
    Json(games.keys().cloned().collect())
}

async fn create_game(
    State(state): State<AppState>,
    Json(request): Json<CreateGameRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    if request.seats.is_empty() {
        return Err(ApiProblem::bad_request("At least one seat is required"));
    }

    let rules = rules_shared::VariantRules::official();
    let participants = request
        .seats
        .into_iter()
        .enumerate()
        .map(|(seat_number, seat)| ParticipantState {
            seat_number: seat_number as u8,
            kind: seat.kind,
            display_name: seat.display_name,
            player_id: None,
            engine_id: seat.engine_id,
            score: 0,
            rack: rules_shared::Rack::default(),
            resigned: false,
        })
        .collect::<Vec<_>>();

    let seed = request.seed.unwrap_or(0x5EED_1234);
    let game = GameSession::new(Uuid::new_v4().to_string(), participants, seed, rules);
    let dto = game.to_dto();

    persistence::save_game(&state.db, &game)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    state.games.write().await.insert(game.id.clone(), game);
    Ok(Json(dto))
}

async fn get_game(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let games = state.games.read().await;
    let game = games
        .get(&game_id)
        .ok_or_else(|| ApiProblem::not_found("Game not found"))?;
    Ok(Json(game.to_dto()))
}

async fn start_game(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    Json(_request): Json<StartGameRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let dto = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;
        game.start();
        run_engine_turns(game, &state.engines)?;
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        dto
    };

    let _ = state
        .events
        .send(GameEventDto::GameStarted { game: dto.clone() });
    Ok(Json(dto))
}

async fn submit_action(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<GameActionRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let dto = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        match request.action {
            PlayerActionDto::Place { candidate } => game
                .apply_place_move(request.seat_number, move_candidate_from_dto(candidate))
                .map_err(ApiProblem::bad_request)?,
            PlayerActionDto::Pass => game
                .apply_pass(request.seat_number)
                .map_err(ApiProblem::bad_request)?,
            PlayerActionDto::Exchange { tiles } => game
                .apply_exchange(
                    request.seat_number,
                    tiles.into_iter().map(tile_from_dto).collect(),
                )
                .map_err(ApiProblem::bad_request)?,
            PlayerActionDto::Resign => game
                .apply_resign(request.seat_number)
                .map_err(ApiProblem::bad_request)?,
        }

        run_engine_turns(game, &state.engines)?;
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        dto
    };

    let event = if dto.status == api::GameStatus::Finished {
        GameEventDto::GameFinished { game: dto.clone() }
    } else {
        GameEventDto::StateUpdated { game: dto.clone() }
    };
    let _ = state.events.send(event);

    Ok(Json(dto))
}

async fn game_events(
    Path(_game_id): Path<String>,
    State(state): State<AppState>,
    websocket: WebSocketUpgrade,
) -> impl IntoResponse {
    websocket.on_upgrade(move |socket| stream_events(socket, state.events.subscribe()))
}

async fn stream_events(mut socket: WebSocket, mut rx: broadcast::Receiver<GameEventDto>) {
    while let Ok(event) = rx.recv().await {
        let message = match serde_json::to_string(&event) {
            Ok(message) => message,
            Err(_) => continue,
        };

        if socket.send(Message::Text(message.into())).await.is_err() {
            break;
        }
    }
}

fn run_engine_turns(game: &mut GameSession, engines: &EngineRegistry) -> Result<(), ApiProblem> {
    for _ in 0..game.participants.len() {
        let advanced = game
            .maybe_run_engine_turn(engines)
            .map_err(ApiProblem::bad_request)?;
        if !advanced {
            break;
        }
        if game.status == api::GameStatus::Finished {
            break;
        }
    }
    Ok(())
}

pub struct ApiProblem {
    status: StatusCode,
    message: String,
}

impl ApiProblem {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn from_sqlx(error: sqlx::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiProblem {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiError {
                message: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_state::{board_from_dto, move_candidate_to_dto};
    use api::{CreateSeatRequest, GameStateDto, SeatKind};
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use rules_shared::{
        GameState, Letter, MoveGenerator, Rack, RulesEngine, SOWPODS, VariantRules,
    };
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use tower::util::ServiceExt;

    async fn create_test_state(database_url: &str) -> AppState {
        AppState::new(database_url)
            .await
            .expect("test app state should initialize")
    }

    fn test_database_url() -> String {
        let path = std::env::temp_dir().join(format!(
            "scrabble-px-server-test-{}.sqlite3",
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
    async fn create_game_and_list_games_via_http() {
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state);

        let response = send_json(
            app.clone(),
            Method::POST,
            "/games",
            &CreateGameRequest {
                seats: vec![
                    CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        email: None,
                        recovery_secret: None,
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
                        email: None,
                        recovery_secret: None,
                    },
                ],
                seed: Some(1234),
                variant: None,
                language: None,
                board_layout: None,
            },
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let created: GameStateDto = read_json(response).await;
        assert_eq!(created.status, api::GameStatus::Waiting);
        assert_eq!(created.participants.len(), 2);

        let listed_response = send_empty(app.clone(), Method::GET, "/games").await;
        assert_eq!(listed_response.status(), StatusCode::OK);
        let listed: Vec<String> = read_json(listed_response).await;
        assert!(listed.contains(&created.id));

        let fetched_response =
            send_empty(app, Method::GET, &format!("/games/{}", created.id)).await;
        assert_eq!(fetched_response.status(), StatusCode::OK);
        let fetched: GameStateDto = read_json(fetched_response).await;
        assert_eq!(fetched.id, created.id);
    }

    #[tokio::test]
    async fn human_move_endpoint_advances_state_and_triggers_engine_reply() {
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state.clone());

        let created: GameStateDto = read_json(
            send_json(
                app.clone(),
                Method::POST,
                "/games",
                &CreateGameRequest {
                    seats: vec![
                        CreateSeatRequest {
                            kind: SeatKind::Human,
                            display_name: "Alice".to_string(),
                            engine_id: None,
                            email: None,
                            recovery_secret: None,
                        },
                        CreateSeatRequest {
                            kind: SeatKind::Engine,
                            display_name: "Greedy".to_string(),
                            engine_id: Some("greedy-v1".to_string()),
                            email: None,
                            recovery_secret: None,
                        },
                    ],
                    seed: Some(77),
                    variant: None,
                    language: None,
                    board_layout: None,
                },
            )
            .await,
        )
        .await;

        let started_response = send_json(
            app.clone(),
            Method::POST,
            &format!("/games/{}/start", created.id),
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
            game.bag.clear();
            game.participants[0].rack = rack_with_letters(&['A', 'T']);
            game.participants[1].rack = rack_with_letters(&['Q']);
        }

        let rules = VariantRules::official();
        let board = board_from_dto(&started.board).expect("board dto should reconstruct");
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

        let move_response = send_json(
            app,
            Method::POST,
            &format!("/games/{}/actions", created.id),
            &GameActionRequest {
                seat_number: 0,
                action: PlayerActionDto::Place {
                    candidate: move_candidate_to_dto(&candidate),
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
    async fn persisted_games_reload_into_new_app_state() {
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state.clone());

        let created: GameStateDto = read_json(
            send_json(
                app.clone(),
                Method::POST,
                "/games",
                &CreateGameRequest {
                    seats: vec![CreateSeatRequest {
                        kind: SeatKind::Human,
                        display_name: "Alice".to_string(),
                        engine_id: None,
                        email: None,
                        recovery_secret: None,
                    }],
                    seed: Some(999),
                    variant: None,
                    language: None,
                    board_layout: None,
                },
            )
            .await,
        )
        .await;

        let _started: GameStateDto = read_json(
            send_json(
                app,
                Method::POST,
                &format!("/games/{}/start", created.id),
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
}
