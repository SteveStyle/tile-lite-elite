use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use std::net::SocketAddr;

use api::{
    AdminGameSummaryDto, AdminResetPasswordRequest, ApiError, CreateGameRequest,
    GameActionRequest, GameEventDto, GameInvitationDto, InvitationStatus, InvitePlayerRequest,
    LoginPlayerRequest, PlayerActionDto, PlayerDto, PlayerSessionDto, PreviewMoveRequest,
    RegisterPlayerRequest, StartGameRequest, ValidateSessionRequest,
};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, Path, Query, Request, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use sqlx::{Pool, Sqlite};
use tokio::sync::{RwLock, broadcast};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

use crate::game_state::{
    EngineRegistry, GameSession, ParticipantState, format_move_error, move_candidate_from_dto,
    tile_from_dto,
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
    // Admin routes are for operating the server, not for players — no
    // token or account, just "you're on the same machine as the server."
    // The guard below enforces that regardless of what SCRABBLE_PX_BIND is
    // set to (docs/operations.md documents binding to 0.0.0.0 for LAN
    // play, which would otherwise expose these to the whole LAN too).
    let admin_routes = Router::new()
        .route("/admin/users", get(admin_list_users))
        .route("/admin/users/{player_id}", delete(admin_delete_user))
        .route(
            "/admin/users/{player_id}/reset-password",
            post(admin_reset_password),
        )
        .route("/admin/games", get(admin_list_games))
        .route("/admin/games/{game_id}", delete(admin_delete_game))
        .route("/admin/games/{game_id}/force-end", post(admin_force_end_game))
        .layer(middleware::from_fn(require_loopback));

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
        .merge(admin_routes)
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn require_loopback(ConnectInfo(addr): ConnectInfo<SocketAddr>, request: Request, next: Next) -> Response {
    if !addr.ip().is_loopback() {
        return ApiProblem::forbidden("Admin endpoints are only reachable from the server itself")
            .into_response();
    }
    next.run(request).await
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
    headers: HeaderMap,
    Json(request): Json<CreateGameRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    if request.seats.is_empty() {
        return Err(ApiProblem::bad_request("At least one seat is required"));
    }

    // If the creator is logged in, they own the first human seat, so nobody
    // else can act as "them" later. Anonymous creation (no session) still
    // works and produces an unclaimed game, same as before this change —
    // claiming additional seats for other players is a separate invitation
    // flow this doesn't attempt to solve yet.
    let creator_player_id = authenticated_player_id(&state, &headers).await;
    let mut creator_seat_claimed = false;

    let rules = rules_shared::VariantRules::official();
    let participants = request
        .seats
        .into_iter()
        .enumerate()
        .map(|(seat_number, seat)| {
            let player_id = if !creator_seat_claimed && seat.kind == api::SeatKind::Human {
                creator_seat_claimed = true;
                creator_player_id.clone()
            } else {
                None
            };
            ParticipantState {
                seat_number: seat_number as u8,
                kind: seat.kind,
                display_name: seat.display_name,
                player_id,
                engine_id: seat.engine_id,
                score: 0,
                rack: rules_shared::Rack::default(),
                resigned: false,
            }
        })
        .collect::<Vec<_>>();

    // A caller-supplied seed exists purely for deterministic tests; real
    // play must get a fresh shuffle each time. The old fallback was a fixed
    // constant, so every game created through the UI (which never sends a
    // seed) dealt the exact same racks in the exact same order, every game.
    let seed = request.seed.unwrap_or_else(rand::random);
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
    headers: HeaderMap,
    Json(_request): Json<StartGameRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    let dto = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        // A fully anonymous game (no claimed seats) stays open to anyone,
        // same as before this change. Once any seat is claimed, only a
        // player who owns one of those seats can start the match — same
        // "unclaimed stays open, claimed is owner-only" rule submit_action
        // already applies per-seat, just evaluated game-wide here since
        // starting isn't a per-seat action.
        let claimed_owners: Vec<&str> = game
            .participants
            .iter()
            .filter_map(|participant| participant.player_id.as_deref())
            .collect();
        let caller_owns_a_seat = caller_player_id
            .as_deref()
            .is_some_and(|id| claimed_owners.contains(&id));
        if !claimed_owners.is_empty() && !caller_owns_a_seat {
            return Err(ApiProblem::unauthorized(
                "Only a player with a claimed seat in this game can start it",
            ));
        }

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
    headers: HeaderMap,
    Json(request): Json<GameActionRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    let dto = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        // Unclaimed seats (no player_id bound at game creation) stay open
        // to anyone, same as before this change. A claimed seat can only
        // be acted on by the player who owns it.
        if let Some(owner_id) = game
            .participants
            .iter()
            .find(|participant| participant.seat_number == request.seat_number)
            .and_then(|participant| participant.player_id.as_ref())
        {
            if caller_player_id.as_deref() != Some(owner_id.as_str()) {
                return Err(ApiProblem::unauthorized(
                    "This seat belongs to a different player",
                ));
            }
        }

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
    headers: HeaderMap,
    Json(request): Json<PreviewMoveRequest>,
) -> Result<Json<api::PreviewMoveResponse>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;
    let games = state.games.read().await;
    let game = games
        .get(&game_id)
        .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

    // Previewing a seat you don't own would otherwise let a caller probe
    // an opponent's exact rack contents by repeatedly guessing candidate
    // placements and reading back legality/score.
    if let Some(owner_id) = game
        .participants
        .get(request.seat_number as usize)
        .and_then(|participant| participant.player_id.as_ref())
    {
        if caller_player_id.as_deref() != Some(owner_id.as_str()) {
            return Err(ApiProblem::unauthorized(
                "This seat belongs to a different player",
            ));
        }
    }

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
            headline: format_move_error(&error),
            detail: String::new(),
            score: None,
        },
    };

    Ok(Json(response))
}

async fn suggest_move(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

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
        if let Some(owner_id) = participant.player_id.as_ref() {
            if caller_player_id.as_deref() != Some(owner_id.as_str()) {
                return Err(ApiProblem::unauthorized(
                    "This seat belongs to a different player",
                ));
            }
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

async fn game_events(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    websocket: WebSocketUpgrade,
) -> impl IntoResponse {
    websocket.on_upgrade(move |socket| stream_events(socket, game_id, state.events.subscribe()))
}

/// `state.events` is a single broadcast channel shared by every game on the
/// server, not one per game — every subscriber sees every game's events
/// unless it filters, which this previously didn't do at all (the path's
/// `game_id` was captured but never used). That meant a socket opened for
/// one game silently received full state — including every seat's rack —
/// for every other game in progress too. The client already discarded
/// non-matching events, so this was invisible in normal play, but the
/// leak was real on the wire.
fn event_belongs_to_game(event: &GameEventDto, game_id: &str) -> bool {
    let game = match event {
        GameEventDto::StateUpdated { game }
        | GameEventDto::GameStarted { game }
        | GameEventDto::GameFinished { game } => game,
    };
    game.id == game_id
}

async fn stream_events(mut socket: WebSocket, game_id: String, mut rx: broadcast::Receiver<GameEventDto>) {
    while let Ok(event) = rx.recv().await {
        if !event_belongs_to_game(&event, &game_id) {
            continue;
        }

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
    let display_name = request.display_name.trim();
    let email = request.email.trim();
    if display_name.is_empty() || email.is_empty() || request.password.is_empty() {
        return Err(ApiProblem::bad_request(
            "Display name, email, and password are all required",
        ));
    }

    if persistence::get_player_by_name(&state.db, display_name)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .is_some()
    {
        return Err(ApiProblem::bad_request(
            "That display name is already taken",
        ));
    }

    let password_hash = hash_password(&request.password)
        .map_err(|_| ApiProblem::bad_request("Could not process that password"))?;

    let player_id = Uuid::new_v4().to_string();
    persistence::create_player(&state.db, &player_id, display_name, email, &password_hash)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    let session_token = Uuid::new_v4().to_string();
    persistence::create_session(
        &state.db,
        &Uuid::new_v4().to_string(),
        &player_id,
        &hash_token(&session_token),
        None,
    )
    .await
    .map_err(ApiProblem::from_sqlx)?;

    Ok(Json(PlayerSessionDto {
        player_id,
        session_token,
        display_name: display_name.to_string(),
        email: email.to_string(),
    }))
}

async fn login_player(
    State(state): State<AppState>,
    Json(request): Json<LoginPlayerRequest>,
) -> Result<Json<PlayerSessionDto>, ApiProblem> {
    // The same error is returned whether the name doesn't exist or the
    // password is wrong, so a caller can't use this endpoint to discover
    // which display names are registered.
    let mismatch = || ApiProblem::bad_request("Incorrect name or password");

    let player = persistence::get_player_by_name(&state.db, request.display_name.trim())
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(mismatch)?;

    if !verify_password(&request.password, &player.password_hash) {
        return Err(mismatch());
    }

    let session_token = Uuid::new_v4().to_string();
    persistence::create_session(
        &state.db,
        &Uuid::new_v4().to_string(),
        &player.id,
        &hash_token(&session_token),
        None,
    )
    .await
    .map_err(ApiProblem::from_sqlx)?;

    Ok(Json(PlayerSessionDto {
        player_id: player.id,
        session_token,
        display_name: player.display_name,
        email: player.email,
    }))
}

async fn validate_session(
    State(state): State<AppState>,
    Json(request): Json<ValidateSessionRequest>,
) -> Result<Json<PlayerDto>, ApiProblem> {
    let session = persistence::get_session_by_token_hash(&state.db, &hash_token(&request.session_token))
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| ApiProblem::unauthorized("Session is invalid or has expired"))?;

    let player = persistence::get_player_by_id(&state.db, &session.player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| ApiProblem::unauthorized("Session is invalid or has expired"))?;

    Ok(Json(PlayerDto {
        id: player.id,
        display_name: player.display_name,
        email: player.email,
        created_at: player.created_at,
        last_seen_at: player.last_seen_at,
    }))
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

/// Argon2 is deliberately slow, which is exactly right for a human-chosen
/// password (resists brute-force guessing) but wrong for a session token
/// looked up on every request — that uses `hash_token` (sha256) instead,
/// since a UUIDv4 token already has enough entropy that a fast hash is safe.
fn hash_password(password: &str) -> Result<String, String> {
    use argon2::password_hash::{SaltString, rand_core::OsRng};
    use argon2::{Argon2, PasswordHasher};

    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| error.to_string())
}

fn verify_password(password: &str, stored_hash: &str) -> bool {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};

    let Ok(parsed_hash) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Resolves the `Authorization: Bearer <token>` header (if present and
/// valid) to a player id. Returns `None` rather than an error for any
/// missing/malformed/unknown/expired token — callers decide whether an
/// absent identity is acceptable for the action they're guarding.
async fn authenticated_player_id(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;

    let session = persistence::get_session_by_token_hash(&state.db, &hash_token(token))
        .await
        .ok()??;

    if let Some(expires_at) = &session.expires_at {
        if let (Ok(expiry), Ok(now)) = (expires_at.parse::<u64>(), now_iso().parse::<u64>()) {
            if now > expiry {
                return None;
            }
        }
    }

    Some(session.player_id)
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

/// Safety ceiling on engine turns run per trigger (a `/start` or `/actions`
/// call). This isn't meant to be hit in practice: `maybe_run_engine_turn`
/// already stops as soon as the current seat isn't an engine, and a real
/// game is bounded by its tile bag (well under 200 turns even worst-case).
/// It exists only to keep a future buggy engine from hanging a request
/// forever in an all-engine game, where there's no human seat to naturally
/// break the loop.
const MAX_ENGINE_TURNS_PER_TRIGGER: usize = 400;

async fn run_engine_turns(game: &mut GameSession, engines: &EngineRegistry) -> Result<(), ApiProblem> {
    for _ in 0..MAX_ENGINE_TURNS_PER_TRIGGER {
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

// ========== Admin Handlers ==========
//
// Reachable only from loopback (see `require_loopback`) — an operator with
// terminal access to the server, not player-facing, hence no per-account
// auth here.

async fn admin_list_users(State(state): State<AppState>) -> Result<Json<Vec<PlayerDto>>, ApiProblem> {
    let players = persistence::list_players(&state.db)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    Ok(Json(
        players
            .into_iter()
            .map(|player| PlayerDto {
                id: player.id,
                display_name: player.display_name,
                email: player.email,
                created_at: player.created_at,
                last_seen_at: player.last_seen_at,
            })
            .collect(),
    ))
}

async fn admin_delete_user(
    State(state): State<AppState>,
    Path(player_id): Path<String>,
) -> Result<StatusCode, ApiProblem> {
    let deleted = persistence::delete_player(&state.db, &player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    if !deleted {
        return Err(ApiProblem::not_found("Player not found"));
    }
    // The DB row is unclaimed already (see `delete_player`); every loaded
    // `GameSession` is a separate in-memory copy that needs the same
    // update, or a still-running server would keep serving the seat as
    // claimed by a player that no longer exists.
    for game in state.games.write().await.values_mut() {
        for participant in &mut game.participants {
            if participant.player_id.as_deref() == Some(player_id.as_str()) {
                participant.player_id = None;
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_reset_password(
    State(state): State<AppState>,
    Path(player_id): Path<String>,
    Json(request): Json<AdminResetPasswordRequest>,
) -> Result<StatusCode, ApiProblem> {
    if request.new_password.is_empty() {
        return Err(ApiProblem::bad_request("A new password is required"));
    }
    let password_hash = hash_password(&request.new_password)
        .map_err(|_| ApiProblem::bad_request("Could not process that password"))?;
    let updated = persistence::update_player_password(&state.db, &player_id, &password_hash)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    if !updated {
        return Err(ApiProblem::not_found("Player not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
struct AdminGamesQuery {
    status: Option<String>,
    older_than_days: Option<i64>,
}

async fn admin_list_games(
    State(state): State<AppState>,
    Query(query): Query<AdminGamesQuery>,
) -> Result<Json<Vec<AdminGameSummaryDto>>, ApiProblem> {
    let created_at = persistence::created_at_by_game(&state.db)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    let last_activity = persistence::last_activity_by_game(&state.db)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    let status_filter = match query.status.as_deref() {
        Some("waiting") => Some(api::GameStatus::Waiting),
        Some("active") => Some(api::GameStatus::Active),
        Some("finished") => Some(api::GameStatus::Finished),
        Some(other) => {
            return Err(ApiProblem::bad_request(format!(
                "Unknown status '{other}', expected waiting/active/finished"
            )));
        }
        None => None,
    };
    let cutoff = query.older_than_days.map(|days| {
        let now_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before epoch")
            .as_secs() as i64;
        now_seconds - days * 86_400
    });

    let games = state.games.read().await;
    let mut summaries: Vec<AdminGameSummaryDto> = games
        .values()
        .filter(|game| status_filter.as_ref().is_none_or(|status| &game.status == status))
        .filter(|game| {
            let Some(cutoff) = cutoff else {
                return true;
            };
            created_at
                .get(&game.id)
                .and_then(|value| value.parse::<i64>().ok())
                .is_some_and(|created| created <= cutoff)
        })
        .map(|game| AdminGameSummaryDto {
            id: game.id.clone(),
            status: game.status.clone(),
            created_at: created_at
                .get(&game.id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            last_activity_at: last_activity
                .get(&game.id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            participants: game
                .participants
                .iter()
                .map(|participant| api::ParticipantDto {
                    seat_number: participant.seat_number,
                    kind: participant.kind.clone(),
                    display_name: participant.display_name.clone(),
                    player_id: participant.player_id.clone(),
                    engine_id: participant.engine_id.clone(),
                    score: participant.score,
                })
                .collect(),
        })
        .collect();
    summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(Json(summaries))
}

async fn admin_delete_game(
    State(state): State<AppState>,
    Path(game_id): Path<String>,
) -> Result<StatusCode, ApiProblem> {
    let deleted = persistence::delete_game(&state.db, &game_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    if !deleted {
        return Err(ApiProblem::not_found("Game not found"));
    }
    state.games.write().await.remove(&game_id);
    Ok(StatusCode::NO_CONTENT)
}

/// Directly marks a game `Finished` without going through per-seat
/// resignation — for an operator to clear out a stuck or abandoned game
/// (e.g. a human seat that will never act again). Doesn't touch scores or
/// `winner_seat`.
async fn admin_force_end_game(
    State(state): State<AppState>,
    Path(game_id): Path<String>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let dto = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;
        game.status = api::GameStatus::Finished;
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        dto
    };
    let _ = state.events.send(GameEventDto::GameFinished { game: dto.clone() });
    Ok(Json(dto))
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

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
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
            Some(payload) => {
                Body::from(serde_json::to_vec(payload).expect("payload should serialize"))
            }
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
                    },
                    CreateSeatRequest {
                        kind: SeatKind::Engine,
                        display_name: "Greedy".to_string(),
                        engine_id: Some("greedy-v1".to_string()),
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
    async fn games_created_without_a_seed_get_different_racks() {
        // Regression test: the default seed used to be a fixed constant, so
        // every game created without an explicit seed (i.e. every real game
        // through the UI, which never sends one) dealt the exact same racks
        // in the exact same order, every single time.
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state);

        let new_game = CreateGameRequest {
            seats: vec![
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Alice".to_string(),
                    engine_id: None,
                },
                CreateSeatRequest {
                    kind: SeatKind::Human,
                    display_name: "Bob".to_string(),
                    engine_id: None,
                },
            ],
            seed: None,
            variant: None,
            language: None,
            board_layout: None,
        };

        let first: GameStateDto =
            read_json(send_json(app.clone(), Method::POST, "/games", &new_game).await).await;
        let second: GameStateDto =
            read_json(send_json(app.clone(), Method::POST, "/games", &new_game).await).await;

        let first_started: GameStateDto = read_json(
            send_json(
                app.clone(),
                Method::POST,
                &format!("/games/{}/start", first.id),
                &StartGameRequest::default(),
            )
            .await,
        )
        .await;
        let second_started: GameStateDto = read_json(
            send_json(
                app,
                Method::POST,
                &format!("/games/{}/start", second.id),
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
                        },
                        CreateSeatRequest {
                            kind: SeatKind::Engine,
                            display_name: "Greedy".to_string(),
                            engine_id: Some("greedy-v1".to_string()),
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
    async fn human_going_out_with_empty_bag_finishes_game_with_rack_penalty() {
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
                        },
                        CreateSeatRequest {
                            kind: SeatKind::Human,
                            display_name: "Bob".to_string(),
                            engine_id: None,
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

        let started: GameStateDto = read_json(
            send_json(
                app.clone(),
                Method::POST,
                &format!("/games/{}/start", created.id),
                &StartGameRequest::default(),
            )
            .await,
        )
        .await;
        assert_eq!(started.status, api::GameStatus::Active);

        // Empty bag + Alice's rack holding exactly the tiles she's about to
        // play means she goes out this move: the game should end
        // immediately (no engine/Bob follow-up turn) with the standard
        // rack-penalty adjustment — Bob loses the value of his leftover
        // rack, Alice gains it.
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
                        },
                        CreateSeatRequest {
                            kind: SeatKind::Human,
                            display_name: "Bob".to_string(),
                            engine_id: None,
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
                        },
                        CreateSeatRequest {
                            kind: SeatKind::Engine,
                            display_name: "Greedy".to_string(),
                            engine_id: Some("greedy-v1".to_string()),
                        },
                    ],
                    seed: Some(7),
                    variant: None,
                    language: None,
                    board_layout: None,
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
                },
            )
            .await,
        )
        .await;

        let seat_one = match seat_one_kind {
            SeatKind::Human => CreateSeatRequest {
                kind: SeatKind::Human,
                display_name: "Player 2".to_string(),
                engine_id: None,
            },
            SeatKind::Engine => CreateSeatRequest {
                kind: SeatKind::Engine,
                display_name: "Greedy".to_string(),
                engine_id: Some("greedy-v1".to_string()),
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
                        },
                        seat_one,
                    ],
                    seed: Some(7),
                    variant: None,
                    language: None,
                    board_layout: None,
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
    async fn anonymous_game_start_still_works_for_anyone() {
        // Games created without being logged in have no claimed seats, so
        // starting one must stay open — same backward-compatibility
        // guarantee as anonymous creation and unclaimed-seat actions.
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state);

        let started = create_two_human_game(app).await;
        assert_eq!(started.status, api::GameStatus::Active);
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
                tile: api::TileDto::Letter { letter: 'A' },
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

    fn empty_live_game_for_test(id: &str) -> GameStateDto {
        GameStateDto {
            id: id.to_string(),
            status: api::GameStatus::Waiting,
            variant: "official".to_string(),
            language: "sowpods".to_string(),
            board_layout: "official".to_string(),
            turn_number: 0,
            current_seat: 0,
            winner_seat: None,
            bag_count: 100,
            participants: Vec::new(),
            board: Vec::new(),
            racks: Vec::new(),
            moves: Vec::new(),
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
            },
        )
        .await;
        assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn engine_vs_engine_game_runs_to_completion() {
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
                            kind: SeatKind::Engine,
                            display_name: "Greedy One".to_string(),
                            engine_id: Some("greedy-v1".to_string()),
                        },
                        CreateSeatRequest {
                            kind: SeatKind::Engine,
                            display_name: "Greedy Two".to_string(),
                            engine_id: Some("greedy-v1".to_string()),
                        },
                    ],
                    seed: Some(777),
                    variant: None,
                    language: None,
                    board_layout: None,
                },
            )
            .await,
        )
        .await;
        assert_eq!(created.status, api::GameStatus::Waiting);

        // A single /start call should drive both engine seats all the way
        // to game-over: no human ever exists to trigger a follow-up round,
        // so `run_engine_turns` has to run the whole game in one go.
        let response = send_json(
            app,
            Method::POST,
            &format!("/games/{}/start", created.id),
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
            finished.moves.iter().any(|record| record.move_type == "place"),
            "expected at least one engine to place tiles rather than only pass, got moves: {:?}",
            finished.moves
        );
        assert!(
            finished.participants.iter().any(|participant| participant.score != 0),
            "expected at least one participant to have a non-zero score by game end"
        );
    }

    #[tokio::test]
    async fn admin_endpoints_reject_non_loopback_callers() {
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state);

        let response =
            send_admin::<()>(app, Method::GET, "/admin/users", remote_peer(), None).await;
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
                },
            )
            .await,
        )
        .await;

        let listed: Vec<PlayerDto> = read_json(
            send_admin::<()>(app.clone(), Method::GET, "/admin/users", loopback_peer(), None)
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
        assert!(!listed_after.iter().any(|player| player.id == alice.player_id));
    }

    #[tokio::test]
    async fn admin_deleting_a_user_unclaims_their_seat_but_keeps_the_game() {
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
                        },
                        CreateSeatRequest {
                            kind: SeatKind::Engine,
                            display_name: "Greedy".to_string(),
                            engine_id: Some("greedy-v1".to_string()),
                        },
                    ],
                    seed: Some(7),
                    variant: None,
                    language: None,
                    board_layout: None,
                },
            )
            .await,
        )
        .await;
        assert_eq!(created.participants[0].player_id.as_deref(), Some(alice.player_id.as_str()));

        let delete_response = send_admin::<()>(
            app.clone(),
            Method::DELETE,
            &format!("/admin/users/{}", alice.player_id),
            loopback_peer(),
            None,
        )
        .await;
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

        let fetched: GameStateDto =
            read_json(send_empty(app, Method::GET, &format!("/games/{}", created.id)).await).await;
        assert_eq!(fetched.id, created.id, "the game itself should survive");
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
            send_admin::<()>(app.clone(), Method::GET, "/admin/users", loopback_peer(), None)
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
            send_admin::<()>(app.clone(), Method::GET, "/admin/games", loopback_peer(), None)
                .await,
        )
        .await;
        let listed_game = listed
            .iter()
            .find(|game| game.id == created.id)
            .expect("created game should be listed");
        assert!(!listed_game.created_at.is_empty());

        let delete_response = send_admin::<()>(
            app.clone(),
            Method::DELETE,
            &format!("/admin/games/{}", created.id),
            loopback_peer(),
            None,
        )
        .await;
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

        let fetch_after = send_empty(app.clone(), Method::GET, &format!("/games/{}", created.id)).await;
        assert_eq!(fetch_after.status(), StatusCode::NOT_FOUND);

        let listed_after: Vec<AdminGameSummaryDto> = read_json(
            send_admin::<()>(app, Method::GET, "/admin/games", loopback_peer(), None).await,
        )
        .await;
        assert!(!listed_after.iter().any(|game| game.id == created.id));
    }

    #[tokio::test]
    async fn admin_can_force_end_a_stuck_game() {
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state);

        let started = create_two_human_game(app.clone()).await;
        assert_eq!(started.status, api::GameStatus::Active);

        let response = send_admin::<()>(
            app.clone(),
            Method::POST,
            &format!("/admin/games/{}/force-end", started.id),
            loopback_peer(),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let ended: GameStateDto = read_json(response).await;
        assert_eq!(ended.status, api::GameStatus::Finished);

        let fetched: GameStateDto =
            read_json(send_empty(app, Method::GET, &format!("/games/{}", started.id)).await).await;
        assert_eq!(fetched.status, api::GameStatus::Finished);
    }
}
