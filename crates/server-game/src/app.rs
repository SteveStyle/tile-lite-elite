use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use api::{
    ApiError, CreateGameRequest, GameActionRequest, GameEventDto, GameInvitationDto,
    InvitationStatus, InvitePlayerRequest, LoginPlayerRequest, PlayerActionDto, PlayerDto,
    PlayerSessionDto, PreviewMoveRequest, RegisterPlayerRequest, StartGameRequest,
    ValidateSessionRequest,
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
        // Authentication
        .route("/auth/register", post(register_player))
        .route("/auth/login", post(login_player))
        .route("/auth/validate", post(validate_session))
        // Games
        .route("/games", post(create_game).get(list_games))
        .route("/games/{game_id}", get(get_game))
        .route("/games/{game_id}/start", post(start_game))
        .route("/games/{game_id}/actions", post(submit_action))
        .route("/games/{game_id}/preview", post(preview_move))
        .route("/games/{game_id}/suggest", post(suggest_move))
        .route("/games/{game_id}/events", get(game_events))
        // Game Invitations
        .route("/games/{game_id}/invite", post(invite_player_to_game))
        .route(
            "/players/{player_id}/invitations",
            get(list_player_invitations),
        )
        .route(
            "/invitations/{invitation_id}/accept",
            post(accept_invitation),
        )
        .route(
            "/invitations/{invitation_id}/reject",
            post(reject_invitation),
        )
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn list_engines(State(state): State<AppState>) -> Json<Vec<api::EngineProfileDto>> {
    Json(state.engines.metadata())
}

async fn list_games(State(state): State<AppState>) -> Result<Json<Vec<api::GameSummaryDto>>, ApiProblem> {
    let last_activity = persistence::last_activity_by_game(&state.db)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    let games = state.games.read().await;
    let mut summaries: Vec<api::GameSummaryDto> = games
        .values()
        .map(|game| {
            let last_activity_at = last_activity
                .get(&game.id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            game.to_summary_dto(last_activity_at)
        })
        .collect();
    summaries.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));

    Ok(Json(summaries))
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
        run_engine_turns(game, &state.engines).await?;
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

        run_engine_turns(game, &state.engines).await?;
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

async fn preview_move(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<PreviewMoveRequest>,
) -> Result<Json<api::PreviewMoveResponse>, ApiProblem> {
    let games = state.games.read().await;
    let game = games
        .get(&game_id)
        .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

    if game.status != api::GameStatus::Active {
        return Ok(Json(api::PreviewMoveResponse {
            is_legal: false,
            headline: "Game is not active".to_string(),
            detail: String::new(),
            score: None,
        }));
    }

    let rack = game
        .participants
        .get(request.seat_number as usize)
        .map(|p| p.rack)
        .unwrap_or_default();

    let candidate = move_candidate_from_dto(request.candidate);
    let engine = rules_shared::RulesEngine {
        rules: &game.rules,
        dictionary: &*rules_shared::SOWPODS,
    };

    let response = match engine.validate_game_move(&game.state, Some(&rack), &candidate) {
        Ok(validated) => api::PreviewMoveResponse {
            is_legal: true,
            headline: format!(
                "{} for {} points",
                validated.preview.main_word, validated.score.total
            ),
            detail: if validated.preview.cross_words.is_empty() {
                "No cross words.".to_string()
            } else {
                format!(
                    "Cross words: {}",
                    validated
                        .preview
                        .cross_words
                        .iter()
                        .map(|w| w.word.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            score: Some(validated.score.total),
        },
        Err(error) => api::PreviewMoveResponse {
            is_legal: false,
            headline: "Move is not currently legal".to_string(),
            detail: format_move_error(&error),
            score: None,
        },
    };

    Ok(Json(response))
}

async fn suggest_move(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let dto = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        if game.status != api::GameStatus::Active {
            return Err(ApiProblem::bad_request("Game is not active"));
        }
        let current_seat = game.current_seat as usize;
        let participant = game
            .participants
            .get(current_seat)
            .ok_or_else(|| ApiProblem::bad_request("Current seat missing"))?;
        if participant.kind != api::SeatKind::Human {
            return Err(ApiProblem::bad_request(
                "Current seat is not human-controlled",
            ));
        }

        let rack = participant.rack;
        let engine = rules_shared::RulesEngine {
            rules: &game.rules,
            dictionary: &*rules_shared::SOWPODS,
        };

        use rules_shared::MoveGenerator as _;
        let mut best_candidate = None;
        let mut best_score = i16::MIN;
        for candidate in engine.enumerate_legal_moves(&game.state, &rack) {
            if let Ok(validated) = engine.validate_game_move(&game.state, Some(&rack), &candidate) {
                if validated.score.total > best_score {
                    best_score = validated.score.total;
                    best_candidate = Some(candidate);
                }
            }
        }

        let seat = game.current_seat;
        match best_candidate {
            Some(candidate) => game
                .apply_place_move(seat, candidate)
                .map_err(ApiProblem::bad_request)?,
            None => game.apply_pass(seat).map_err(ApiProblem::bad_request)?,
        }

        run_engine_turns(game, &state.engines).await?;
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

fn format_move_error(error: &rules_shared::MoveError) -> String {
    match error {
        rules_shared::MoveError::InvalidMove => "Invalid move shape.".to_string(),
        rules_shared::MoveError::InvalidWord(word) => format!("Invalid word: {word}"),
        rules_shared::MoveError::InvalidPosition => "Tile placement is off the board.".to_string(),
        rules_shared::MoveError::InvalidDirection => "Invalid move direction.".to_string(),
        rules_shared::MoveError::TilesDoNotFit => {
            "The tiles do not fit the rack or span.".to_string()
        }
        rules_shared::MoveError::TilesDoNotConnect => {
            "The move does not connect to the board correctly.".to_string()
        }
        rules_shared::MoveError::LetterNotAllowedInPosition => {
            "A tile is not allowed at one of the chosen squares.".to_string()
        }
    }
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

// ========== Authentication Handlers ==========

async fn register_player(
    State(state): State<AppState>,
    Json(request): Json<RegisterPlayerRequest>,
) -> Result<Json<PlayerSessionDto>, ApiProblem> {
    // Hash the recovery secret
    let secret_hash = hash_secret(&request.recovery_secret);

    let player_id = Uuid::new_v4().to_string();
    let session_token = Uuid::new_v4().to_string();
    let session_token_hash = hash_secret(&session_token);

    // Create player
    let _ = persistence::create_player(
        &state.db,
        &player_id,
        &request.display_name,
        &request.email,
        &secret_hash,
    )
    .await
    .map_err(|_| ApiProblem::bad_request("Failed to create player"))?;

    // Create session
    let _ = persistence::create_session(
        &state.db,
        &Uuid::new_v4().to_string(),
        &player_id,
        &session_token_hash,
        None,
    )
    .await
    .map_err(|_| ApiProblem::bad_request("Failed to create session"))?;

    Ok(Json(PlayerSessionDto {
        player_id,
        session_token,
        display_name: request.display_name,
        email: request.email,
    }))
}

async fn login_player(
    State(state): State<AppState>,
    Json(request): Json<LoginPlayerRequest>,
) -> Result<Json<PlayerSessionDto>, ApiProblem> {
    // Look up player by display name
    let player = persistence::get_player_by_name(&state.db, &request.display_name)
        .await
        .map_err(|_| ApiProblem::bad_request("Database error"))?
        .ok_or_else(|| ApiProblem::not_found("Player not found"))?;

    // Verify recovery secret
    let secret_hash = hash_secret(&request.recovery_secret);
    if secret_hash != player.recovery_secret_hash {
        return Err(ApiProblem::bad_request("Invalid recovery secret"));
    }

    // Create session
    let session_token = Uuid::new_v4().to_string();
    let session_token_hash = hash_secret(&session_token);
    let _ = persistence::create_session(
        &state.db,
        &Uuid::new_v4().to_string(),
        &player.id,
        &session_token_hash,
        None,
    )
    .await
    .map_err(|_| ApiProblem::bad_request("Failed to create session"))?;

    Ok(Json(PlayerSessionDto {
        player_id: player.id,
        session_token,
        display_name: player.display_name,
        email: player.email,
    }))
}

async fn validate_session(
    State(_state): State<AppState>,
    Json(request): Json<ValidateSessionRequest>,
) -> Result<Json<PlayerDto>, ApiProblem> {
    let _session_token_hash = hash_secret(&request.session_token);

    // Find session by token hash
    // Note: In a real system, you'd want an index on token_hash for performance
    // For now, we'll need to query differently or use a different approach
    // This is a simplification - in production, store sessions in a faster cache

    Err(ApiProblem::bad_request(
        "Session validation not yet implemented",
    ))
}

// ========== Game Invitation Handlers ==========

async fn invite_player_to_game(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<InvitePlayerRequest>,
) -> Result<Json<GameInvitationDto>, ApiProblem> {
    // Get the game
    let games = state.games.read().await;
    let game = games
        .get(&game_id)
        .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

    // Verify the game is still in waiting state
    if game.status != api::GameStatus::Waiting {
        return Err(ApiProblem::bad_request(
            "Game must be in waiting state to invite players",
        ));
    }

    // Find the invited player
    let invited_player = persistence::get_player_by_name(&state.db, &request.invited_display_name)
        .await
        .map_err(|_| ApiProblem::bad_request("Database error"))?
        .ok_or_else(|| ApiProblem::not_found("Invited player not found"))?;

    // For now, assume the first participant is the inviting player
    let inviting_player_id = game
        .participants
        .first()
        .and_then(|p| p.player_id.clone())
        .ok_or_else(|| ApiProblem::bad_request("Cannot determine inviting player"))?;

    // Create the invitation
    let invitation_id = Uuid::new_v4().to_string();
    let _ = persistence::create_invitation(
        &state.db,
        &invitation_id,
        &game_id,
        &invited_player.id,
        &inviting_player_id,
        request.seat_number,
    )
    .await
    .map_err(|_| ApiProblem::bad_request("Failed to create invitation"))?;

    Ok(Json(GameInvitationDto {
        id: invitation_id,
        game_id,
        invited_player_id: invited_player.id,
        inviting_player_id,
        seat_number: request.seat_number,
        status: InvitationStatus::Pending,
        created_at: now_iso(),
        responded_at: None,
        inviting_player_display_name: game.participants[0].display_name.clone(),
    }))
}

async fn list_player_invitations(
    Path(player_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Vec<GameInvitationDto>>, ApiProblem> {
    let invitations = persistence::get_invitations_for_player(&state.db, &player_id)
        .await
        .map_err(|_| ApiProblem::bad_request("Database error"))?;

    let mut result = Vec::new();
    for inv in invitations {
        let inviting_player = persistence::get_player_by_id(&state.db, &inv.inviting_player_id)
            .await
            .map_err(|_| ApiProblem::bad_request("Database error"))?;

        if let Some(inviter) = inviting_player {
            let status = match inv.status.as_str() {
                "accepted" => InvitationStatus::Accepted,
                "rejected" => InvitationStatus::Rejected,
                "cancelled" => InvitationStatus::Cancelled,
                _ => InvitationStatus::Pending,
            };

            result.push(GameInvitationDto {
                id: inv.id,
                game_id: inv.game_id,
                invited_player_id: inv.invited_player_id,
                inviting_player_id: inv.inviting_player_id,
                seat_number: inv.seat_number,
                status,
                created_at: inv.created_at,
                responded_at: inv.responded_at,
                inviting_player_display_name: inviter.display_name,
            });
        }
    }

    Ok(Json(result))
}

async fn accept_invitation(
    Path(invitation_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiProblem> {
    persistence::update_invitation_status(&state.db, &invitation_id, "accepted")
        .await
        .map_err(|_| ApiProblem::bad_request("Failed to update invitation"))?;

    Ok(Json(serde_json::json!({
        "status": "accepted"
    })))
}

async fn reject_invitation(
    Path(invitation_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiProblem> {
    persistence::update_invitation_status(&state.db, &invitation_id, "rejected")
        .await
        .map_err(|_| ApiProblem::bad_request("Failed to update invitation"))?;

    Ok(Json(serde_json::json!({
        "status": "rejected"
    })))
}

// ========== Helper Functions ==========

fn hash_secret(secret: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    secret.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs();
    seconds.to_string()
}

/// How long an engine gets to choose an action before the seat auto-passes.
/// Hobby-project default; not yet configurable per engine or per game.
const ENGINE_TURN_TIMEOUT: Duration = Duration::from_secs(5);

async fn run_engine_turns(game: &mut GameSession, engines: &EngineRegistry) -> Result<(), ApiProblem> {
    for _ in 0..game.participants.len() {
        let advanced = game
            .maybe_run_engine_turn(engines, ENGINE_TURN_TIMEOUT)
            .await
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
        let listed: Vec<api::GameSummaryDto> = read_json(listed_response).await;
        let summary = listed
            .iter()
            .find(|summary| summary.id == created.id)
            .expect("created game should appear in the summary list");
        assert_eq!(summary.status, api::GameStatus::Waiting);
        assert_eq!(summary.participants.len(), 2);
        assert!(
            !summary.last_activity_at.is_empty() && summary.last_activity_at != "unknown",
            "expected a real timestamp, got {:?}",
            summary.last_activity_at
        );

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

    async fn create_two_human_game(app: Router) -> GameStateDto {
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
                            kind: SeatKind::Human,
                            display_name: "Bob".to_string(),
                            engine_id: None,
                            email: None,
                            recovery_secret: None,
                        },
                    ],
                    seed: Some(42),
                    variant: None,
                    language: None,
                    board_layout: None,
                },
            )
            .await,
        )
        .await;

        let started: GameStateDto = read_json(
            send_json(
                app,
                Method::POST,
                &format!("/games/{}/start", created.id),
                &StartGameRequest::default(),
            )
            .await,
        )
        .await;
        assert_eq!(started.status, api::GameStatus::Active);
        started
    }

    #[tokio::test]
    async fn pass_action_advances_turn_and_records_move() {
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state.clone());

        let started = create_two_human_game(app.clone()).await;
        assert_eq!(started.current_seat, 0);

        let response = send_json(
            app,
            Method::POST,
            &format!("/games/{}/actions", started.id),
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
            let game = games.get_mut(&started.id).expect("game should exist");
            game.participants[0].rack = rack_with_letters(&['A', 'T']);
        }

        let response = send_json(
            app,
            Method::POST,
            &format!("/games/{}/actions", started.id),
            &GameActionRequest {
                seat_number: 0,
                action: PlayerActionDto::Exchange {
                    tiles: vec![
                        api::TileDto::Letter { letter: 'A' },
                        api::TileDto::Letter { letter: 'T' },
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

        let response = send_json(
            app,
            Method::POST,
            &format!("/games/{}/actions", started.id),
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
    }

    #[tokio::test]
    async fn acting_out_of_turn_returns_bad_request() {
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state.clone());

        let started = create_two_human_game(app.clone()).await;
        assert_eq!(started.current_seat, 0);

        let response = send_json(
            app,
            Method::POST,
            &format!("/games/{}/actions", started.id),
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

        let response = send_json(
            app.clone(),
            Method::POST,
            &format!("/games/{}/actions", started.id),
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
            send_empty(app, Method::GET, &format!("/games/{}", started.id)).await,
        )
        .await;
        assert_eq!(fetched.current_seat, 0);
        assert_eq!(fetched.moves.len(), 0);
    }
}
