use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use std::net::SocketAddr;

use api::{
    AdminGameSummaryDto, AdminResetPasswordRequest, ApiError, ChangePasswordRequest,
    CreateGameRequest, GameActionRequest, GameEventDto, GameInvitationDto, InvitationStatus,
    InvitePlayerRequest, LoginPlayerRequest, PlayerActionDto, PlayerDto, PlayerSessionDto,
    PostChatMessageRequest, PreviewMoveRequest, RegisterPlayerRequest, RequestPasswordResetRequest,
    ResetPasswordRequest, StartGameRequest, SwapSeatsRequest, ValidateSessionRequest,
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
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::game_state::{
    EngineRegistry, GameSession, ParticipantState, ViewerAccess, attach_invitation_status,
    move_candidate_from_dto, redact_game_state, resolve_viewer_access, tile_from_dto,
};
use crate::persistence;
use rules_shared::format_move_error;

#[derive(Clone)]
pub struct AppState {
    pub db: Pool<Sqlite>,
    pub games: Arc<RwLock<HashMap<String, GameSession>>>,
    pub events: broadcast::Sender<GameEventDto>,
    pub engines: EngineRegistry,
    /// Where the web client is actually served from — needed server-side
    /// only for building an absolute link into a password-reset email
    /// (`{public_base_url}/reset-password?token=...`). Everything else the
    /// server does is host-agnostic (see `TILE_LITE_ELITE_API_BASE_URL`'s
    /// own doc comment in `docs/operations.md` for why the *client* doesn't
    /// need this baked in), so this field exists solely for that one link.
    pub public_base_url: String,
    pub email: crate::email::EmailConfig,
}

impl AppState {
    pub async fn new(
        database_url: &str,
        public_base_url: String,
        email: crate::email::EmailConfig,
    ) -> Result<Self, sqlx::Error> {
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
            public_base_url,
            email,
        })
    }
}

pub fn build_router(state: AppState) -> Router {
    // Admin routes are for operating the server, not for players — no
    // token or account, just "you're on the same machine as the server."
    // The guard below enforces that regardless of what TILE_LITE_ELITE_BIND is
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
        .route("/dictionaries/{name}", get(get_dictionary))
        // Authentication
        .route("/auth/register", post(register_player))
        .route("/auth/login", post(login_player))
        .route("/auth/validate", post(validate_session))
        .route("/auth/change-password", post(change_password))
        .route("/auth/update-details", post(update_player_details))
        .route("/auth/forgot-password", post(request_password_reset))
        .route("/auth/reset-password", post(reset_password))
        .route("/players/search", get(search_players))
        // Games
        .route("/games", post(create_game).get(list_games))
        .route("/games/{game_id}", get(get_game))
        .route("/games/{game_id}/start", post(start_game))
        .route("/games/{game_id}/reorder-seats", post(swap_seats))
        .route("/games/{game_id}/seats", post(add_seat_to_game))
        .route(
            "/games/{game_id}/seats/{seat_number}/remove",
            post(remove_seat_from_game),
        )
        .route(
            "/games/{game_id}/seats/{seat_number}/withdraw",
            post(withdraw_from_seat),
        )
        .route(
            "/games/{game_id}/seats/{seat_number}/force-resign",
            post(force_resign_seat),
        )
        .route("/games/{game_id}/actions", post(submit_action))
        .route("/games/{game_id}/chat", post(post_chat_message))
        .route("/games/{game_id}/remove", post(remove_game_for_player))
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
            "/invitations/{invitation_id}/preview",
            get(preview_invitation),
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
        // One INFO-level span per request (method, path, status, latency)
        // with no per-handler code — this alone covers "what happened and
        // when" for the whole HTTP surface; the tracing calls sprinkled
        // through individual handlers below are for the *why* on top of it
        // (which player, which game, why a request was rejected).
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn require_loopback(ConnectInfo(addr): ConnectInfo<SocketAddr>, request: Request, next: Next) -> Response {
    if !addr.ip().is_loopback() {
        return ApiProblem::forbidden("Admin endpoints are only reachable from the server itself")
            .into_response();
    }
    next.run(request).await
}

/// The `Major.Minor.Patch` release version from Cargo.toml, plus an
/// optional build identifier appended as SemVer build metadata (`+<id>`)
/// when `TILE_LITE_ELITE_BUILD_ID` is set at compile time — e.g. a git short
/// SHA or CI run number, for telling internal/test builds apart. A
/// production release simply doesn't set that var, so it shows only the
/// three numbers. Distinct from `api::API_VERSION`: this is the build
/// identity, not the wire-contract version clients check on connect.
/// Logged at startup (`main.rs`) and served at `/health` — the latter is
/// what lets `scripts/deploy-staging.sh at prod` find out which commit is
/// actually live without SSHing in.
pub fn app_version() -> String {
    format_app_version(env!("CARGO_PKG_VERSION"), option_env!("TILE_LITE_ELITE_BUILD_ID"))
}

fn format_app_version(pkg_version: &str, build_id: Option<&str>) -> String {
    match build_id {
        Some(id) if !id.is_empty() => format!("{pkg_version}+{id}"),
        _ => pkg_version.to_string(),
    }
}

async fn health() -> Json<api::HealthDto> {
    Json(api::HealthDto {
        status: "ok".to_string(),
        api_version: api::API_VERSION,
        app_version: app_version(),
    })
}

async fn list_engines(State(state): State<AppState>) -> Json<Vec<api::EngineProfileDto>> {
    Json(state.engines.metadata())
}

/// Serves a dictionary's raw word-list text on request, for clients (the
/// wasm/web build specifically) that fetch it at runtime rather than
/// embedding it at compile time — the server already has this exact text
/// compiled in (`rules_shared::sowpods_word_list`), so this is just
/// re-serving it, not a second copy of the file anywhere. Unauthenticated,
/// same as `/health`/`/engines` — a word list isn't sensitive, and every
/// signed-in player's client needs it regardless of which game they're in.
async fn get_dictionary(Path(name): Path<String>) -> Result<String, ApiProblem> {
    match name.as_str() {
        "sowpods" => Ok(rules_shared::sowpods_word_list().to_string()),
        "enable2k" => Ok(rules_shared::enable2k_word_list().to_string()),
        "german" => Ok(rules_shared::german_word_list().to_string()),
        "spanish" => Ok(rules_shared::spanish_word_list().to_string()),
        _ => Err(ApiProblem::not_found(format!("Unknown dictionary '{name}'"))),
    }
}

async fn list_games(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<api::GameSummaryDto>>, ApiProblem> {
    // The list is inherently personal — which games show up depends on who's
    // asking — so there's no meaningful "browse everything" mode anymore.
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to see your games"))?;

    expire_overdue_turns(&state).await;
    expire_old_finished_games(&state).await;
    if let Err(error) = persistence::delete_expired_sessions(&state.db).await {
        tracing::error!(%error, "failed to delete expired sessions");
    }

    let last_activity = persistence::last_activity_by_game(&state.db)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    let named_invitations = persistence::get_invitations_for_player(&state.db, &caller_player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    let open_invitations = persistence::get_open_invitations(&state.db)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    let games = state.games.read().await;
    let mut summaries: Vec<api::GameSummaryDto> = Vec::new();

    for game in games.values() {
        let last_activity_at = last_activity
            .get(&game.id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        let is_participant = game
            .participants
            .iter()
            .any(|p| p.player_id.as_deref() == Some(caller_player_id.as_str()));
        if is_participant {
            let removed_by_caller = game.participants.iter().any(|p| {
                p.player_id.as_deref() == Some(caller_player_id.as_str()) && p.removed_by_player
            });
            if removed_by_caller {
                continue;
            }
            let relationship = if game.status == api::GameStatus::Active
                && game
                    .participants
                    .get(game.current_seat as usize)
                    .and_then(|p| p.player_id.as_deref())
                    == Some(caller_player_id.as_str())
            {
                api::GameRelationship::YourTurn
            } else {
                api::GameRelationship::Participant
            };
            let mut summary = game.to_summary_dto(last_activity_at);
            summary.relationship = relationship;
            summaries.push(summary);
            continue;
        }

        if let Some(invitation) = named_invitations
            .iter()
            .find(|inv| inv.game_id == game.id && inv.status == "pending")
        {
            let mut summary = game.to_summary_dto(last_activity_at);
            summary.relationship = api::GameRelationship::InvitedByName;
            summary.invitation_id = Some(invitation.id.clone());
            summaries.push(summary);
            continue;
        }

        if let Some(invitation) = open_invitations.iter().find(|inv| inv.game_id == game.id) {
            let mut summary = game.to_summary_dto(last_activity_at);
            summary.relationship = api::GameRelationship::InvitedOpen;
            summary.invitation_id = Some(invitation.id.clone());
            summaries.push(summary);
            continue;
        }

        // Not seated and not invited — still show it if the caller is the
        // one who created it (e.g. an Engine vs Engine game set up to
        // watch, where nobody is ever seated as a human) and they haven't
        // removed it (the unseated-creator counterpart to the seated
        // `removed_by_caller` check above).
        if game.creator_player_id.as_deref() == Some(caller_player_id.as_str())
            && !game.removed_by_creator
        {
            let mut summary = game.to_summary_dto(last_activity_at);
            summary.relationship = api::GameRelationship::Creator;
            summaries.push(summary);
        }
    }

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

    // Every seat now needs a real accepting/claiming party (the creator
    // themselves, a named invitee, or a stranger who accepts an open
    // invitation) — there's no more "anonymous, open to whoever clicks it"
    // seat, so creating a game requires being signed in.
    let creator_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to create a game"))?;

    let creator_claims = request
        .seats
        .iter()
        .filter(|seat| seat.kind == api::SeatKind::Human && matches!(seat.claim, Some(api::SeatClaim::Creator)))
        .count();
    if creator_claims > 1 {
        return Err(ApiProblem::bad_request(
            "Only one seat can be claimed by the creator",
        ));
    }
    if request
        .seats
        .iter()
        .any(|seat| seat.kind == api::SeatKind::Human && seat.claim.is_none())
    {
        return Err(ApiProblem::bad_request(
            "Every human seat needs a claim: creator, named, or open",
        ));
    }

    // Resolve every named invitee up front, before creating anything, so a
    // typo'd name fails cleanly instead of leaving a half-built game behind.
    let mut named_invitees: HashMap<u8, persistence::PlayerRecord> = HashMap::new();
    for (seat_number, seat) in request.seats.iter().enumerate() {
        if let Some(api::SeatClaim::Named { display_name }) = &seat.claim {
            let player = persistence::get_player_by_name(&state.db, display_name)
                .await
                .map_err(ApiProblem::from_sqlx)?
                .ok_or_else(|| ApiProblem::not_found(format!("No player named '{display_name}'")))?;
            named_invitees.insert(seat_number as u8, player);
        }
    }

    let variant_name = request.variant.as_deref().unwrap_or("official");
    let rules = rules_shared::VariantRules::by_name(variant_name).ok_or_else(|| {
        ApiProblem::bad_request(format!("Unknown game variant '{variant_name}'"))
    })?;
    let participants = request
        .seats
        .into_iter()
        .enumerate()
        .map(|(seat_number, seat)| {
            let player_id = match &seat.claim {
                Some(api::SeatClaim::Creator) => Some(creator_player_id.clone()),
                _ => None,
            };
            let invited_email = match &seat.claim {
                Some(api::SeatClaim::Email { email }) => Some(email.clone()),
                _ => None,
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
                removed_by_player: false,
                invited_email,
            }
        })
        .collect::<Vec<_>>();

    // A caller-supplied seed exists purely for deterministic tests; real
    // play must get a fresh shuffle each time. The old fallback was a fixed
    // constant, so every game created through the UI (which never sends a
    // seed) dealt the exact same racks in the exact same order, every game.
    let seed = request.seed.unwrap_or_else(rand::random);
    let move_time_limit_seconds = request
        .move_time_limit_seconds
        .unwrap_or(crate::game_state::DEFAULT_MOVE_TIME_LIMIT_SECONDS);
    let game = GameSession::new(
        Uuid::new_v4().to_string(),
        participants,
        Some(creator_player_id.clone()),
        seed,
        rules,
        move_time_limit_seconds,
    );
    let access = resolve_viewer_access(&game, Some(&creator_player_id));

    persistence::save_game(&state.db, &game)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    // Only fetched if there's at least one named or email invitee to notify
    // — the common case (no invitees, or open-only invitations) shouldn't
    // pay for a lookup nothing will use.
    let has_notifiable_invitee =
        !named_invitees.is_empty() || game.participants.iter().any(|p| p.invited_email.is_some());
    let creator_display_name = if has_notifiable_invitee {
        persistence::get_player_by_id(&state.db, &creator_player_id)
            .await
            .map_err(ApiProblem::from_sqlx)?
            .map(|player| player.display_name)
    } else {
        None
    };

    // Every Human seat that isn't the creator's needs a pending invitation:
    // named, email, or open. Named and email invitations have a specific
    // address to notify — matches invite_player_to_game's identical
    // reasoning for the same cases.
    for participant in &game.participants {
        if participant.kind != api::SeatKind::Human || participant.player_id.is_some() {
            continue;
        }
        let invited_player = named_invitees.get(&participant.seat_number);
        let invited_email = participant.invited_email.as_deref();
        let record = persistence::create_invitation(
            &state.db,
            &Uuid::new_v4().to_string(),
            &game.id,
            invited_player.map(|player| player.id.as_str()),
            &creator_player_id,
            participant.seat_number,
            invited_email,
        )
        .await
        .map_err(ApiProblem::from_sqlx)?;

        if let (Some(invited_player), Some(creator_display_name)) = (invited_player, &creator_display_name) {
            crate::email::send_invitation(
                &state.email,
                &invited_player.email,
                &invited_player.display_name,
                creator_display_name,
                &state.public_base_url,
            )
            .await;
        } else if let (Some(invited_email), Some(creator_display_name)) = (invited_email, &creator_display_name) {
            let join_url = format!("{}/invite?id={}", state.public_base_url, record.id);
            crate::email::send_join_invitation(&state.email, invited_email, creator_display_name, &join_url)
                .await;
        }
    }

    tracing::info!(
        game_id = %game.id,
        creator_player_id = %creator_player_id,
        seats = game.participants.len(),
        move_time_limit_seconds,
        "game created"
    );

    // Built after the invitation-creation loop above, not before — an
    // early `to_dto()` would show every seat as not-yet-sent even though
    // their invitations (and this response) go out together.
    let dto = redact_game_state(game_dto_with_invitation_status(&state, &game).await?, &access);
    state.games.write().await.insert(game.id.clone(), game);
    Ok(Json(dto))
}

async fn get_game(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    expire_overdue_turn(&state, &game_id).await;

    let games = state.games.read().await;
    let game = games
        .get(&game_id)
        .ok_or_else(|| ApiProblem::not_found("Game not found"))?;
    let access = resolve_viewer_access(game, caller_player_id.as_deref());
    if access == ViewerAccess::Rejected {
        return Err(ApiProblem::unauthorized(
            "Sign in and be part of this game to view it",
        ));
    }
    let dto = game_dto_with_invitation_status(&state, game).await?;
    Ok(Json(redact_game_state(dto, &access)))
}

async fn start_game(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(_request): Json<StartGameRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    let (dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        // Every human seat needs a real occupant (creator or an accepted
        // invitation) before play can start — an unclaimed human seat means
        // an invitation is still outstanding, not "open to anyone".
        if game
            .participants
            .iter()
            .any(|p| p.kind == api::SeatKind::Human && p.player_id.is_none())
        {
            return Err(ApiProblem::bad_request(
                "Every seat must be filled before the game can start",
            ));
        }

        if !caller_may_manage_game(game, caller_player_id.as_deref()) {
            return Err(ApiProblem::unauthorized(
                "Only the game's creator can start it",
            ));
        }

        game.start();
        run_engine_turns(game, &state.engines, &state.events).await?;
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        // For an all-engine game, `caller_player_id` may belong to nobody
        // tied to this game at all (any signed-in user may start it, per
        // the check above) — `resolve_viewer_access` correctly resolves
        // that case to `Rejected`, and `redact_game_state` already treats
        // `Rejected` the same as `Creator` (no racks, no chat) rather than
        // panicking, so this is safe to call unconditionally here.
        let access = resolve_viewer_access(game, caller_player_id.as_deref());
        (dto, access)
    };

    tracing::info!(game_id = %dto.id, status = ?dto.status, "game started");

    // Broadcast the *unredacted* dto — each connected socket redacts it to
    // its own viewer's tier in `stream_events`, right before sending. A
    // pre-redacted broadcast would mean every other connection's own
    // redaction step operates on already-stripped data (e.g. losing their
    // own rack because *this* caller's tier didn't include it).
    let _ = state
        .events
        .send(GameEventDto::GameStarted { game: dto.clone() });
    Ok(Json(redact_game_state(dto, &access)))
}

/// Reorders two seats before the game starts — see `GameSession::
/// swap_seats` for why this is only ever offered once every seat is
/// filled. Only the creator may reorder (`caller_may_manage_game`).
async fn swap_seats(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SwapSeatsRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    let (dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        if !caller_may_manage_game(game, caller_player_id.as_deref()) {
            return Err(ApiProblem::unauthorized(
                "Only the game's creator can reorder seats",
            ));
        }

        game.swap_seats(request.seat_a, request.seat_b)
            .map_err(ApiProblem::bad_request)?;
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let access = resolve_viewer_access(game, caller_player_id.as_deref());
        (dto, access)
    };

    // Broadcast the unredacted dto — per-connection redaction happens in
    // `stream_events`, not here (see the identical note in `start_game`).
    let _ = state
        .events
        .send(GameEventDto::StateUpdated { game: dto.clone() });
    Ok(Json(redact_game_state(dto, &access)))
}

/// Adds a new seat to an already-created `Waiting` game — creator-only.
/// Reuses `api::CreateSeatRequest`, the same shape `create_game` takes per
/// seat, rather than a bespoke request type. Doesn't send an invitation —
/// that's a separate, explicit follow-up call to `invite_player_to_game`,
/// so the creator can stage several additions before sending any of them.
async fn add_seat_to_game(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<api::CreateSeatRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to add a seat"))?;

    if matches!(request.claim, Some(api::SeatClaim::Creator)) {
        return Err(ApiProblem::bad_request(
            "Only the original creator seat may use the Creator claim",
        ));
    }
    if request.kind == api::SeatKind::Human && request.claim.is_none() {
        return Err(ApiProblem::bad_request(
            "A human seat needs a claim: named, open, or email",
        ));
    }

    let (dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        if !caller_may_manage_game(game, Some(&caller_player_id)) {
            return Err(ApiProblem::unauthorized(
                "Only the game's creator can add a seat",
            ));
        }

        let invited_email = match &request.claim {
            Some(api::SeatClaim::Email { email }) => Some(email.clone()),
            _ => None,
        };
        let participant = ParticipantState {
            seat_number: 0, // GameSession::add_seat assigns the real number
            kind: request.kind,
            display_name: request.display_name,
            player_id: None,
            engine_id: request.engine_id,
            score: 0,
            rack: rules_shared::Rack::default(),
            resigned: false,
            removed_by_player: false,
            invited_email,
        };
        game.add_seat(participant).map_err(ApiProblem::bad_request)?;

        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let dto = game_dto_with_invitation_status(&state, game).await?;
        let access = resolve_viewer_access(game, Some(&caller_player_id));
        (dto, access)
    };

    tracing::info!(game_id, "seat added");

    let _ = state
        .events
        .send(GameEventDto::StateUpdated { game: dto.clone() });
    Ok(Json(redact_game_state(dto, &access)))
}

/// Removes a seat entirely — creator-only, works on any non-creator seat
/// regardless of claim status (this is also how the creator kicks a
/// confirmed participant, not just cancels an outstanding invite). See
/// `GameSession::remove_seat` for why every subsequent seat's number
/// shifts down, and `persistence::shift_invitation_seat_numbers_down` for
/// keeping invitation history in sync with that shift.
async fn remove_seat_from_game(
    Path((game_id, seat_number)): Path<(String, u8)>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to remove a seat"))?;

    let (dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        if !caller_may_manage_game(game, Some(&caller_player_id)) {
            return Err(ApiProblem::unauthorized(
                "Only the game's creator can remove a seat",
            ));
        }

        game.remove_seat(seat_number)
            .map_err(ApiProblem::bad_request)?;

        if let Some(pending) =
            persistence::get_pending_invitation_for_seat(&state.db, &game_id, seat_number)
                .await
                .map_err(ApiProblem::from_sqlx)?
        {
            persistence::update_invitation_status(&state.db, &pending.id, "cancelled")
                .await
                .map_err(ApiProblem::from_sqlx)?;
        }
        persistence::shift_invitation_seat_numbers_down(&state.db, &game_id, seat_number)
            .await
            .map_err(ApiProblem::from_sqlx)?;

        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let dto = game_dto_with_invitation_status(&state, game).await?;
        let access = resolve_viewer_access(game, Some(&caller_player_id));
        (dto, access)
    };

    tracing::info!(game_id, seat_number, "seat removed");

    let _ = state
        .events
        .send(GameEventDto::StateUpdated { game: dto.clone() });
    Ok(Json(redact_game_state(dto, &access)))
}

/// Lets whoever holds a claimed non-creator seat give it back up before the
/// game starts — see `GameSession::withdraw_seat`. Flips that seat's most
/// recent invitation back to `"rejected"` (reusing the existing status
/// rather than adding a new one — an accepted deliberate blur between
/// "never said yes" and "said yes, then withdrew") so it behaves exactly
/// like any other declined seat afterward: the creator can resend or
/// remove it.
async fn withdraw_from_seat(
    Path((game_id, seat_number)): Path<(String, u8)>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to withdraw from a seat"))?;

    let (dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        let holds_seat = game.participants.get(seat_number as usize).is_some_and(|p| {
            p.player_id.as_deref() == Some(caller_player_id.as_str())
        });
        if !holds_seat {
            return Err(ApiProblem::unauthorized(
                "Only the player holding this seat can withdraw from it",
            ));
        }

        game.withdraw_seat(seat_number)
            .map_err(ApiProblem::bad_request)?;

        let invitations = persistence::get_invitations_for_game(&state.db, &game_id)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        if let Some(accepted) = invitations
            .iter()
            .filter(|invitation| invitation.seat_number == seat_number && invitation.status == "accepted")
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
        {
            persistence::update_invitation_status(&state.db, &accepted.id, "rejected")
                .await
                .map_err(ApiProblem::from_sqlx)?;
        }

        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let dto = game_dto_with_invitation_status(&state, game).await?;
        let access = resolve_viewer_access(game, Some(&caller_player_id));
        (dto, access)
    };

    tracing::info!(game_id, seat_number, player_id = %caller_player_id, "seat withdrawn");

    let _ = state
        .events
        .send(GameEventDto::StateUpdated { game: dto.clone() });
    Ok(Json(redact_game_state(dto, &access)))
}

/// The `Active`-game counterpart to remove/withdraw — creator-only, ends
/// the game early on behalf of a seat that's gone unresponsive. See
/// `GameSession::force_resign` for why this doesn't require it to be that
/// seat's turn, unlike self-resign.
async fn force_resign_seat(
    Path((game_id, seat_number)): Path<(String, u8)>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to force-resign a seat"))?;

    let (dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        if !caller_may_manage_game(game, Some(&caller_player_id)) {
            return Err(ApiProblem::unauthorized(
                "Only the game's creator can force-resign a seat",
            ));
        }

        game.force_resign(seat_number)
            .map_err(ApiProblem::bad_request)?;
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let access = resolve_viewer_access(game, Some(&caller_player_id));
        (dto, access)
    };

    // Same GameFinished-vs-StateUpdated broadcast pattern as submit_action —
    // force_resign always finishes the game, but mirroring the conditional
    // rather than hardcoding GameFinished keeps this consistent in case
    // that invariant ever loosens.
    let event = if dto.status == api::GameStatus::Finished {
        tracing::info!(game_id, seat_number, winner_seat = ?dto.winner_seat, "seat force-resigned by creator; game finished");
        GameEventDto::GameFinished { game: dto.clone() }
    } else {
        GameEventDto::StateUpdated { game: dto.clone() }
    };
    let _ = state.events.send(event);

    Ok(Json(redact_game_state(dto, &access)))
}

async fn submit_action(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<GameActionRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    expire_overdue_turn(&state, &game_id).await;

    let (dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        // A human seat can only be acted on by the player who owns it — an
        // unclaimed human seat means an invitation is still outstanding, not
        // "open to anyone" (engine seats have no owner and aren't reachable
        // through this endpoint in normal play).
        if let Some(seat) = game
            .participants
            .iter()
            .find(|participant| participant.seat_number == request.seat_number)
        {
            if seat.kind == api::SeatKind::Human
                && caller_player_id.as_deref() != seat.player_id.as_deref()
            {
                return Err(ApiProblem::unauthorized(
                    "This seat belongs to a different player",
                ));
            }
        }

        let action_alphabet = game.rules.alphabet.clone();
        match request.action {
            PlayerActionDto::Place { candidate } => game
                .apply_place_move(
                    request.seat_number,
                    move_candidate_from_dto(candidate, &action_alphabet),
                )
                .map_err(ApiProblem::bad_request)?,
            PlayerActionDto::Pass => game
                .apply_pass(request.seat_number)
                .map_err(ApiProblem::bad_request)?,
            PlayerActionDto::Exchange { tiles } => game
                .apply_exchange(
                    request.seat_number,
                    tiles
                        .into_iter()
                        .map(|tile| tile_from_dto(tile, &action_alphabet))
                        .collect(),
                )
                .map_err(ApiProblem::bad_request)?,
            PlayerActionDto::Resign => game
                .apply_resign(request.seat_number)
                .map_err(ApiProblem::bad_request)?,
        }

        run_engine_turns(game, &state.engines, &state.events).await?;
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let access = resolve_viewer_access(game, caller_player_id.as_deref());
        (dto, access)
    };

    // Broadcast the unredacted dto — per-connection redaction happens in
    // `stream_events`, not here (see the identical note in `start_game`).
    let event = if dto.status == api::GameStatus::Finished {
        tracing::info!(game_id = %dto.id, winner_seat = ?dto.winner_seat, "game finished");
        GameEventDto::GameFinished { game: dto.clone() }
    } else {
        GameEventDto::StateUpdated { game: dto.clone() }
    };
    let _ = state.events.send(event);

    Ok(Json(redact_game_state(dto, &access)))
}

/// Not routed through `submit_action`/`PlayerActionDto` — that pipeline
/// enforces turn ownership (`seat_number` must match `current_seat`), and
/// chat must work regardless of whose turn it is, or even after the game
/// has finished. Not gated on game status for the same reason — players can
/// still chat during the week between a game finishing and its auto-expiry.
async fn post_chat_message(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PostChatMessageRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to chat"))?;

    let (dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        let display_name = game
            .participants
            .iter()
            .find(|participant| participant.player_id.as_deref() == Some(caller_player_id.as_str()))
            .map(|participant| participant.display_name.clone())
            .ok_or_else(|| ApiProblem::unauthorized("Only seated players can chat in this game"))?;

        game.post_chat_message(&caller_player_id, &display_name, request.body)
            .map_err(ApiProblem::bad_request)?;

        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let access = resolve_viewer_access(game, Some(&caller_player_id));
        (dto, access)
    };

    // Broadcast the unredacted dto — per-connection redaction happens in
    // `stream_events`, not here (see the identical note in `start_game`).
    let _ = state.events.send(GameEventDto::StateUpdated { game: dto.clone() });
    Ok(Json(redact_game_state(dto, &access)))
}

/// Hides a finished game from the caller's own games list — see
/// `GameSession::remove_for_player`. Not broadcast over the WebSocket: this
/// is purely a per-viewer concern, so nobody else's view of the game (or
/// even this same player's other logged-in devices, until they next reload
/// their own list) needs to change in response.
async fn remove_game_for_player(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to remove this game from your list"))?;

    let mut games = state.games.write().await;
    let game = games
        .get_mut(&game_id)
        .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

    game.remove_for_player(&caller_player_id)
        .map_err(ApiProblem::bad_request)?;

    persistence::save_game(&state.db, game)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    Ok(Json(serde_json::json!({"status": "removed"})))
}

async fn preview_move(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PreviewMoveRequest>,
) -> Result<Json<api::PreviewMoveResponse>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;
    expire_overdue_turn(&state, &game_id).await;
    let games = state.games.read().await;
    let game = games
        .get(&game_id)
        .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

    // Previewing a seat you don't own would otherwise let a caller probe
    // an opponent's exact rack contents by repeatedly guessing candidate
    // placements and reading back legality/score. An unclaimed human seat
    // means an invitation is still outstanding, so it's nobody's to preview.
    if let Some(seat) = game.participants.get(request.seat_number as usize) {
        if seat.kind == api::SeatKind::Human
            && caller_player_id.as_deref() != seat.player_id.as_deref()
        {
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

    let candidate = move_candidate_from_dto(request.candidate, &game.rules.alphabet);
    let engine = rules_shared::RulesEngine {
        rules: &game.rules,
        dictionary: rules_shared::dictionary_by_name(&game.rules.language)
            .expect("game rules should reference a known dictionary"),
    };

    let response = match engine.validate_game_move(&game.state, Some(&rack), &candidate) {
        Ok(validated) => api::PreviewMoveResponse {
            is_legal: true,
            headline: format!(
                "{} for {} points",
                validated.preview.main_word, validated.score.total
            ),
            detail: if validated.preview.cross_words.is_empty() {
                String::new()
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

    expire_overdue_turn(&state, &game_id).await;

    let (dto, access) = {
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
        if caller_player_id.as_deref() != participant.player_id.as_deref() {
            return Err(ApiProblem::unauthorized(
                "This seat belongs to a different player",
            ));
        }

        let rack = participant.rack;
        let engine = rules_shared::RulesEngine {
            rules: &game.rules,
            dictionary: rules_shared::dictionary_by_name(&game.rules.language)
                .expect("game rules should reference a known dictionary"),
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

        run_engine_turns(game, &state.engines, &state.events).await?;
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let access = resolve_viewer_access(game, caller_player_id.as_deref());
        (dto, access)
    };

    // Broadcast the unredacted dto — per-connection redaction happens in
    // `stream_events`, not here (see the identical note in `start_game`).
    let event = if dto.status == api::GameStatus::Finished {
        tracing::info!(game_id = %dto.id, winner_seat = ?dto.winner_seat, "game finished");
        GameEventDto::GameFinished { game: dto.clone() }
    } else {
        GameEventDto::StateUpdated { game: dto.clone() }
    };
    let _ = state.events.send(event);

    Ok(Json(redact_game_state(dto, &access)))
}

#[derive(Debug, serde::Deserialize)]
struct EventsQuery {
    token: Option<String>,
}

/// Browsers' native `WebSocket` API can't set custom headers on the
/// handshake, so unlike every other endpoint the session token travels as
/// a query parameter here instead of `Authorization: Bearer` — see
/// `player_id_for_token`. Previously this endpoint had no auth check at
/// all: anyone who knew or guessed a `game_id`, logged in or not, could
/// connect and receive that game's full state, including every seat's
/// rack. Now it's gated by the same `resolve_viewer_access` rule as every
/// other game-state endpoint, and `stream_events` redacts each outgoing
/// event to this specific connection's resolved tier.
async fn game_events(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
    websocket: WebSocketUpgrade,
) -> Response {
    let player_id = match query.token {
        Some(token) => player_id_for_token(&state, &token).await,
        None => None,
    };

    let access = {
        let games = state.games.read().await;
        let Some(game) = games.get(&game_id) else {
            return (StatusCode::NOT_FOUND, "Game not found").into_response();
        };
        resolve_viewer_access(game, player_id.as_deref())
    };
    if access == ViewerAccess::Rejected {
        return (
            StatusCode::UNAUTHORIZED,
            "Sign in and be part of this game to watch it live",
        )
            .into_response();
    }

    websocket
        .on_upgrade(move |socket| stream_events(socket, game_id, access, state.events.subscribe()))
}

/// `state.events` is a single broadcast channel shared by every game on the
/// server, not one per game — every subscriber sees every game's events
/// unless it filters, which this previously didn't do at all (the path's
/// `game_id` was captured but never used). That meant a socket opened for
/// one game silently received full state for every other game in progress
/// too. The client already discarded non-matching events, so this was
/// invisible in normal play, but the leak was real on the wire.
fn event_belongs_to_game(event: &GameEventDto, game_id: &str) -> bool {
    let game = match event {
        GameEventDto::StateUpdated { game }
        | GameEventDto::GameStarted { game }
        | GameEventDto::GameFinished { game } => game,
    };
    game.id == game_id
}

/// Redacts an event's embedded `GameStateDto` to `access`'s tier before
/// forwarding it to this specific connection. The broadcast itself always
/// carries the full, unredacted state (every HTTP handler broadcasts that
/// way too — see the "broadcast the unredacted dto" notes throughout this
/// file); redaction only ever happens at the point data actually leaves the
/// server to a specific caller, which for a WebSocket is here, per message,
/// not once at connection time.
fn redact_event(event: GameEventDto, access: &ViewerAccess) -> GameEventDto {
    match event {
        GameEventDto::StateUpdated { game } => GameEventDto::StateUpdated {
            game: redact_game_state(game, access),
        },
        GameEventDto::GameStarted { game } => GameEventDto::GameStarted {
            game: redact_game_state(game, access),
        },
        GameEventDto::GameFinished { game } => GameEventDto::GameFinished {
            game: redact_game_state(game, access),
        },
    }
}

async fn stream_events(
    mut socket: WebSocket,
    game_id: String,
    access: ViewerAccess,
    mut rx: broadcast::Receiver<GameEventDto>,
) {
    while let Ok(event) = rx.recv().await {
        if !event_belongs_to_game(&event, &game_id) {
            continue;
        }
        let event = redact_event(event, &access);

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
    let expires_at = session_expiry(request.stay_logged_in);
    persistence::create_session(
        &state.db,
        &Uuid::new_v4().to_string(),
        &player_id,
        &hash_token(&session_token),
        request.stay_logged_in,
        expires_at.as_deref(),
    )
    .await
    .map_err(ApiProblem::from_sqlx)?;

    tracing::info!(player_id, display_name, "player registered");

    crate::email::send_welcome(&state.email, email, display_name, &state.public_base_url).await;

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
    // which display names are registered. Logging the attempted name
    // server-side (never the password) doesn't weaken that — it's only
    // visible to whoever can already read the server's own logs, and gives
    // an audit trail for spotting repeated failed attempts.
    let display_name = request.display_name.trim().to_string();
    let mismatch = |reason: &'static str| {
        tracing::warn!(display_name = %display_name, reason, "login rejected");
        ApiProblem::bad_request("Incorrect name or password")
    };

    let player = persistence::get_player_by_name(&state.db, &display_name)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| mismatch("unknown display name"))?;

    if !verify_password(&request.password, &player.password_hash) {
        return Err(mismatch("wrong password"));
    }

    tracing::info!(player_id = %player.id, display_name = %display_name, "player logged in");

    let session_token = Uuid::new_v4().to_string();
    let expires_at = session_expiry(request.stay_logged_in);
    persistence::create_session(
        &state.db,
        &Uuid::new_v4().to_string(),
        &player.id,
        &hash_token(&session_token),
        request.stay_logged_in,
        expires_at.as_deref(),
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

#[derive(serde::Deserialize)]
struct SearchPlayersQuery {
    q: String,
}

/// "Invite by name" autocomplete — display names aren't sensitive (already
/// shown throughout every game's participant list), so this is only
/// gated on being signed in at all, not on any relationship to a specific
/// game or invitee.
async fn search_players(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchPlayersQuery>,
) -> Result<Json<Vec<String>>, ApiProblem> {
    authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to search players"))?;

    let prefix = query.q.trim();
    if prefix.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let names = persistence::search_players_by_name_prefix(&state.db, prefix, 8)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    Ok(Json(names))
}

async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiProblem> {
    let player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to change your password"))?;

    if request.new_password.is_empty() {
        return Err(ApiProblem::bad_request("A new password is required"));
    }

    let player = persistence::get_player_by_id(&state.db, &player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| ApiProblem::unauthorized("Session is invalid or has expired"))?;

    // Requiring the current password (rather than trusting the session
    // token alone) matters specifically for "remember me" sessions, which
    // can sit valid on a device for a long time — proving you still know
    // the password is what makes this a deliberate account action rather
    // than something a stolen token alone can do.
    if !verify_password(&request.current_password, &player.password_hash) {
        tracing::warn!(player_id, "password change rejected: wrong current password");
        return Err(ApiProblem::bad_request("Current password is incorrect"));
    }

    let new_hash = hash_password(&request.new_password)
        .map_err(|_| ApiProblem::bad_request("Could not process that password"))?;
    persistence::update_player_password(&state.db, &player_id, &new_hash)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    // Signs the caller's own session out along with every other one — the
    // client is expected to send them back to the login screen. This is
    // deliberate: changing your password should mean starting fresh, not
    // silently keeping whatever session made the request.
    persistence::invalidate_sessions_for_player(&state.db, &player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    tracing::info!(player_id, "password changed; all sessions invalidated");

    Ok(StatusCode::NO_CONTENT)
}

/// Updates display name and/or email — see `api::UpdatePlayerDetailsRequest`'s
/// doc comment for why this doesn't require the current password the way
/// `change_password` does, and doesn't invalidate other sessions either.
async fn update_player_details(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<api::UpdatePlayerDetailsRequest>,
) -> Result<Json<api::PlayerDto>, ApiProblem> {
    let player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to update your details"))?;

    // `Some("")` (field present but blank) is a validation error; `None`
    // (field omitted) just means "leave this one alone" — trim first so a
    // whitespace-only value is treated the same as blank.
    if request.display_name.as_deref().is_some_and(|value| value.trim().is_empty()) {
        return Err(ApiProblem::bad_request("Display name cannot be blank"));
    }
    if request.email.as_deref().is_some_and(|value| value.trim().is_empty()) {
        return Err(ApiProblem::bad_request("Email cannot be blank"));
    }
    let display_name = request.display_name.as_deref().map(str::trim);
    let email = request.email.as_deref().map(str::trim);
    if display_name.is_none() && email.is_none() {
        return Err(ApiProblem::bad_request("Nothing to update"));
    }

    // Same uniqueness rule as registration — email deliberately isn't
    // checked here, matching how duplicate emails are already allowed at
    // registration (see `register_player`).
    if let Some(display_name) = display_name {
        if let Some(existing) = persistence::get_player_by_name(&state.db, display_name)
            .await
            .map_err(ApiProblem::from_sqlx)?
        {
            if existing.id != player_id {
                return Err(ApiProblem::bad_request("That display name is already taken"));
            }
        }
    }

    persistence::update_player_details(&state.db, &player_id, display_name, email)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    let player = persistence::get_player_by_id(&state.db, &player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| ApiProblem::unauthorized("Session is invalid or has expired"))?;

    tracing::info!(player_id, "player details updated");

    Ok(Json(api::PlayerDto {
        id: player.id,
        display_name: player.display_name,
        email: player.email,
        created_at: player.created_at,
        last_seen_at: player.last_seen_at,
    }))
}

/// "Forgot password" step 1: request a reset link by email.
///
/// Always returns `204` whether or not the email is registered — same
/// non-enumeration principle `login_player` already uses for its shared
/// error message, just with a shared *success* instead, since this endpoint
/// has no legitimate reason to distinguish the two outcomes to the caller.
///
/// The reset link only ever appears in a log line if `state.email` has no
/// provider configured (see `EmailConfig`'s doc comment) — with Resend
/// wired up, `crate::email::send_password_reset` delivers it and does not
/// log the link itself, so a live reset link never sits in server logs.
async fn request_password_reset(
    State(state): State<AppState>,
    Json(request): Json<RequestPasswordResetRequest>,
) -> Result<StatusCode, ApiProblem> {
    let email = request.email.trim().to_string();
    if email.is_empty() {
        return Err(ApiProblem::bad_request("An email address is required"));
    }

    if let Some(player) = persistence::get_player_by_email(&state.db, &email)
        .await
        .map_err(ApiProblem::from_sqlx)?
    {
        persistence::invalidate_password_reset_tokens_for_player(&state.db, &player.id)
            .await
            .map_err(ApiProblem::from_sqlx)?;

        let token = Uuid::new_v4().to_string();
        let expires_at = now_iso()
            .parse::<u64>()
            .map(|now| (now + PASSWORD_RESET_TOKEN_TTL_SECS).to_string())
            .unwrap_or_default();
        persistence::create_password_reset_token(
            &state.db,
            &Uuid::new_v4().to_string(),
            &player.id,
            &hash_token(&token),
            &expires_at,
        )
        .await
        .map_err(ApiProblem::from_sqlx)?;

        let reset_url = format!("{}/reset-password?token={}", state.public_base_url, token);
        tracing::info!(player_id = %player.id, "password reset requested");
        crate::email::send_password_reset(&state.email, &email, &reset_url).await;
    } else {
        tracing::info!(email = %email, "password reset requested for unknown email");
    }

    Ok(StatusCode::NO_CONTENT)
}

/// "Forgot password" step 2: consume the token from the emailed link.
///
/// Deliberately doesn't distinguish "token doesn't exist" from "token
/// already consumed" from "token expired" in the *response* (all three are
/// the same generic `bad_request` a stale/reused browser tab would hit
/// legitimately) but does distinguish them in the log line, for anyone
/// debugging a specific report of the flow not working.
async fn reset_password(
    State(state): State<AppState>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<StatusCode, ApiProblem> {
    if request.new_password.is_empty() {
        return Err(ApiProblem::bad_request("A new password is required"));
    }

    let invalid = || ApiProblem::bad_request("This reset link is invalid or has expired");

    let record = persistence::get_password_reset_token_by_hash(&state.db, &hash_token(&request.token))
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| {
            tracing::warn!("password reset rejected: unknown token");
            invalid()
        })?;

    if record.consumed_at.is_some() {
        tracing::warn!(player_id = %record.player_id, "password reset rejected: token already used");
        return Err(invalid());
    }

    let expiry: u64 = record.expires_at.parse().unwrap_or(0);
    let now: u64 = now_iso().parse().unwrap_or(u64::MAX);
    if now > expiry {
        tracing::warn!(player_id = %record.player_id, "password reset rejected: token expired");
        return Err(invalid());
    }

    let new_hash = hash_password(&request.new_password)
        .map_err(|_| ApiProblem::bad_request("Could not process that password"))?;
    persistence::update_player_password(&state.db, &record.player_id, &new_hash)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    persistence::consume_password_reset_token(&state.db, &record.id)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    // Same reasoning as change_password: a password reset should mean
    // starting fresh, not silently keeping whatever session (if any)
    // happened to still be around on some other device.
    persistence::invalidate_sessions_for_player(&state.db, &record.player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    tracing::info!(player_id = %record.player_id, "password reset via emailed token; all sessions invalidated");

    Ok(StatusCode::NO_CONTENT)
}

// ========== Game Invitation Handlers ==========

async fn invite_player_to_game(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<InvitePlayerRequest>,
) -> Result<Json<GameInvitationDto>, ApiProblem> {
    let inviting_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to invite players"))?;

    let games = state.games.read().await;
    let game = games
        .get(&game_id)
        .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

    if game.status != api::GameStatus::Waiting {
        return Err(ApiProblem::bad_request(
            "Game must be in waiting state to invite players",
        ));
    }

    if !caller_may_manage_game(game, Some(&inviting_player_id)) {
        return Err(ApiProblem::unauthorized(
            "Only the game's creator can invite players",
        ));
    }

    let seat = game
        .participants
        .iter()
        .find(|p| p.seat_number == request.seat_number)
        .ok_or_else(|| ApiProblem::bad_request("No such seat"))?;
    if seat.kind != api::SeatKind::Human || seat.player_id.is_some() {
        return Err(ApiProblem::bad_request(
            "That seat is not open to be invited to",
        ));
    }
    if persistence::get_pending_invitation_for_seat(&state.db, &game_id, request.seat_number)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .is_some()
    {
        return Err(ApiProblem::bad_request(
            "This seat already has a pending invitation",
        ));
    }

    if request.invited_display_name.is_some() && request.invited_email.is_some() {
        return Err(ApiProblem::bad_request(
            "An invitation can target a display name or an email, not both",
        ));
    }

    // `None` means an open/stranger invitation — any logged-in player may
    // accept it, not just one specific invitee.
    let invited_player = match &request.invited_display_name {
        Some(display_name) => Some(
            persistence::get_player_by_name(&state.db, display_name)
                .await
                .map_err(ApiProblem::from_sqlx)?
                .ok_or_else(|| ApiProblem::not_found(format!("No player named '{display_name}'")))?,
        ),
        None => None,
    };
    let invited_email = request.invited_email.as_deref();

    let inviting_player = persistence::get_player_by_id(&state.db, &inviting_player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| ApiProblem::unauthorized("Session is invalid or has expired"))?;

    let invitation_id = Uuid::new_v4().to_string();
    let record = persistence::create_invitation(
        &state.db,
        &invitation_id,
        &game_id,
        invited_player.as_ref().map(|p| p.id.as_str()),
        &inviting_player_id,
        request.seat_number,
        invited_email,
    )
    .await
    .map_err(ApiProblem::from_sqlx)?;

    tracing::info!(
        game_id,
        invitation_id,
        seat_number = request.seat_number,
        invited_player_id = record.invited_player_id.as_deref(),
        "invitation created"
    );

    // `game`/`seat` (borrowed from this guard) aren't needed past this
    // point — released explicitly rather than held across the email send
    // below, which talks to an external service and could be much slower
    // than the local DB awaits this guard was already (pre-existing
    // pattern) held across.
    drop(games);

    // Named and email invitations have a specific address to notify — an
    // open/stranger invitation has no invitee yet, just an open seat.
    if let Some(invited_player) = &invited_player {
        crate::email::send_invitation(
            &state.email,
            &invited_player.email,
            &invited_player.display_name,
            &inviting_player.display_name,
            &state.public_base_url,
        )
        .await;
    } else if let Some(invited_email) = invited_email {
        let join_url = format!("{}/invite?id={}", state.public_base_url, record.id);
        crate::email::send_join_invitation(&state.email, invited_email, &inviting_player.display_name, &join_url)
            .await;
    }

    Ok(Json(GameInvitationDto {
        id: record.id,
        game_id: record.game_id,
        invited_player_id: record.invited_player_id,
        inviting_player_id: record.inviting_player_id,
        seat_number: record.seat_number,
        status: InvitationStatus::Pending,
        created_at: record.created_at,
        responded_at: None,
        inviting_player_display_name: inviting_player.display_name,
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
            result.push(GameInvitationDto {
                id: inv.id,
                game_id: inv.game_id,
                invited_player_id: inv.invited_player_id,
                inviting_player_id: inv.inviting_player_id,
                seat_number: inv.seat_number,
                status: invitation_status_from_str(&inv.status),
                created_at: inv.created_at,
                responded_at: inv.responded_at,
                inviting_player_display_name: inviter.display_name,
            });
        }
    }

    Ok(Json(result))
}

fn invitation_status_from_str(status: &str) -> InvitationStatus {
    match status {
        "accepted" => InvitationStatus::Accepted,
        "rejected" => InvitationStatus::Rejected,
        "cancelled" => InvitationStatus::Cancelled,
        _ => InvitationStatus::Pending,
    }
}

/// Unauthenticated: the landing page an emailed join link opens on needs
/// this before the visitor has necessarily registered or logged in — see
/// `api::InvitationPreviewDto`'s doc comment for exactly what it does and
/// doesn't reveal.
async fn preview_invitation(
    Path(invitation_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<api::InvitationPreviewDto>, ApiProblem> {
    let invitation = persistence::get_invitation_by_id(&state.db, &invitation_id)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| ApiProblem::not_found("Invitation not found"))?;
    let inviter = persistence::get_player_by_id(&state.db, &invitation.inviting_player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| ApiProblem::not_found("Invitation not found"))?;

    Ok(Json(api::InvitationPreviewDto {
        inviting_player_display_name: inviter.display_name,
        status: invitation_status_from_str(&invitation.status),
    }))
}

async fn accept_invitation(
    Path(invitation_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to accept an invitation"))?;

    // Race-safe: for an open invitation, this is where "first to accept
    // wins" is actually decided (an atomic DB update, not a check-then-act
    // in application code) — see `claim_invitation`.
    let record = persistence::claim_invitation(&state.db, &invitation_id, &caller_player_id)
        .await
        .map_err(|error| match error {
            persistence::ClaimInvitationError::NotFound => {
                ApiProblem::not_found("Invitation not found")
            }
            persistence::ClaimInvitationError::NoLongerAvailable => ApiProblem::bad_request(
                "This invitation is no longer available — it may already have been claimed",
            ),
            persistence::ClaimInvitationError::NotYourInvitation => {
                ApiProblem::unauthorized("This invitation was not sent to you")
            }
        })?;

    // Needed to fill in the seat's real display name below — an Open
    // seat's participant row is created with a generic "Open seat"
    // placeholder (there's no invitee to name yet), and a Named seat's
    // already-correct name shouldn't silently diverge from the account
    // that actually claimed it (e.g. after a display-name change).
    let claimant = persistence::get_player_by_id(&state.db, &caller_player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?
        .ok_or_else(|| ApiProblem::unauthorized("Session is invalid or has expired"))?;

    let (dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&record.game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;
        if let Some(participant) = game
            .participants
            .iter_mut()
            .find(|p| p.seat_number == record.seat_number)
        {
            participant.player_id = Some(caller_player_id.clone());
            participant.display_name = claimant.display_name.clone();
        }
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let access = resolve_viewer_access(game, Some(&caller_player_id));
        (dto, access)
    };

    tracing::info!(
        invitation_id,
        game_id = %record.game_id,
        seat_number = record.seat_number,
        player_id = %caller_player_id,
        "invitation accepted; seat claimed"
    );

    // Broadcast the unredacted dto — per-connection redaction happens in
    // `stream_events`, not here (see the identical note in `start_game`).
    let _ = state.events.send(GameEventDto::StateUpdated { game: dto.clone() });
    Ok(Json(redact_game_state(dto, &access)))
}

async fn reject_invitation(
    Path(invitation_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to reject an invitation"))?;

    let invitations = persistence::get_invitations_for_player(&state.db, &caller_player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    let invitation = invitations
        .iter()
        .find(|inv| inv.id == invitation_id)
        .ok_or_else(|| {
            // Either it doesn't exist, or it's an open invitation with no
            // single invitee — either way, there's nothing for this caller
            // to reject.
            ApiProblem::not_found("Invitation not found")
        })?;
    if invitation.status != "pending" {
        return Err(ApiProblem::bad_request(
            "This invitation has already been responded to",
        ));
    }

    persistence::update_invitation_status(&state.db, &invitation_id, "rejected")
        .await
        .map_err(ApiProblem::from_sqlx)?;

    tracing::info!(invitation_id, player_id = %caller_player_id, "invitation rejected");

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

/// Whether `caller_player_id` may perform a game-*management* action —
/// start, reorder, add/remove/invite a seat, force-resign. The creator is
/// the game's manager; everyone else (including a seated participant) is
/// not. A game persisted before `creator_player_id` existed (`None`) falls
/// back to the old, more permissive rule instead of becoming permanently
/// unmanageable: any claimed-seat owner, or (for an all-engine game, which
/// has no claimed seats at all) any signed-in caller.
fn caller_may_manage_game(game: &GameSession, caller_player_id: Option<&str>) -> bool {
    match game.creator_player_id.as_deref() {
        Some(creator_id) => caller_player_id == Some(creator_id),
        None => {
            let claimed_owners: Vec<&str> = game
                .participants
                .iter()
                .filter_map(|participant| participant.player_id.as_deref())
                .collect();
            claimed_owners.is_empty() || caller_player_id.is_some_and(|id| claimed_owners.contains(&id))
        }
    }
}

/// `game.to_dto()`, plus each unclaimed seat's `invitation_status` filled
/// in — but only when the game is `Waiting`, the only status where an
/// unclaimed seat (and thus a meaningful status for it) can exist at all;
/// skipping the extra `game_invitations` fetch otherwise isn't a shortcut,
/// it's simply unnecessary work (see `attach_invitation_status`'s own doc
/// comment). Use this instead of a bare `game.to_dto()` in any handler
/// where a caller might reasonably want to see seat status: viewing a
/// game, creating one, or any of the seat-management endpoints.
async fn game_dto_with_invitation_status(
    state: &AppState,
    game: &GameSession,
) -> Result<api::GameStateDto, ApiProblem> {
    let mut dto = game.to_dto();
    if dto.status == api::GameStatus::Waiting {
        let invitations = persistence::get_invitations_for_game(&state.db, &game.id)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        attach_invitation_status(&mut dto, &invitations);
    }
    Ok(dto)
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
    player_id_for_token(state, token).await
}

/// Shared by `authenticated_player_id` (reads the token from the
/// `Authorization` header, used by every REST call) and `game_events`
/// (reads it from a query parameter instead — browsers' native `WebSocket`
/// API can't set custom headers on the handshake, so the token has to
/// travel some other way for that one endpoint).
async fn player_id_for_token(state: &AppState, token: &str) -> Option<String> {
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

/// How long a password-reset link stays valid after it's requested. Short
/// enough that a link sitting unread in an inbox for days can't be used,
/// long enough that it isn't a race against actually receiving the email.
const PASSWORD_RESET_TOKEN_TTL_SECS: u64 = 60 * 60;

/// How long an ordinary (not "stay logged in") session stays valid. Most
/// logins never check that box, so without a real expiry every one of
/// them leaves a permanent row — this is what actually bounds the
/// `sessions` table's growth; a `stay_logged_in` session gets no expiry
/// at all (see `session_expiry`), since that's the whole point of the box.
const SESSION_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// `None` for a `stay_logged_in` session (no expiry); otherwise
/// `SESSION_TTL_SECS` out from now. Shared by `register_player` and
/// `login_player` so both compute it identically.
fn session_expiry(stay_logged_in: bool) -> Option<String> {
    if stay_logged_in {
        return None;
    }
    now_iso()
        .parse::<u64>()
        .map(|now| (now + SESSION_TTL_SECS).to_string())
        .ok()
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

/// Paced gap between broadcasting one engine turn and computing the next.
/// A greedy move search on a 15x15 board is fast enough (single-digit
/// milliseconds) that an all-engine game would otherwise finish before a
/// freshly-opened WebSocket even completes its handshake, let alone before
/// the client can render each state — so *some* artificial pacing is
/// required for the moves to be visible at all, not just theoretically
/// broadcast. Short enough that a full engine-vs-engine game (well under
/// `MAX_ENGINE_TURNS_PER_TRIGGER`) still finishes in a few seconds.
const ENGINE_TURN_BROADCAST_DELAY: Duration = Duration::from_millis(120);

/// Broadcasts a `StateUpdated`/`GameFinished` event after every individual
/// engine turn, not just once when the whole loop finishes. Without this, an
/// all-engine game (nothing left to block on a human) runs start-to-finish
/// inside a single `/start` (or `/actions`) request, and a client watching
/// it live never sees anything but the final state — the moves happen too
/// fast to be visible in the one HTTP response, but the WebSocket is a
/// separate connection, so a caller subscribed to this game *before*
/// issuing that request still receives every intermediate move as it's
/// computed, even though the request itself won't resolve until the whole
/// game is done.
async fn run_engine_turns(
    game: &mut GameSession,
    engines: &EngineRegistry,
    events: &broadcast::Sender<GameEventDto>,
) -> Result<(), ApiProblem> {
    for _ in 0..MAX_ENGINE_TURNS_PER_TRIGGER {
        let advanced = game
            .maybe_run_engine_turn(engines, ENGINE_TURN_TIMEOUT)
            .await
            .map_err(ApiProblem::bad_request)?;
        if !advanced {
            break;
        }
        let dto = game.to_dto();
        let finished = game.status == api::GameStatus::Finished;
        let event = if finished {
            GameEventDto::GameFinished { game: dto }
        } else {
            GameEventDto::StateUpdated { game: dto }
        };
        let _ = events.send(event);
        if finished {
            break;
        }
        tokio::time::sleep(ENGINE_TURN_BROADCAST_DELAY).await;
    }
    Ok(())
}

/// There's no background scheduler in this server, so overdue-turn
/// retirement is checked lazily: call this at the top of any handler that
/// reads or acts on live games, and any seat that's overrun its
/// `move_time_limit_seconds` gets auto-retired (same effect as resigning)
/// before the rest of the handler runs. Persists and broadcasts every game
/// it changes.
async fn expire_overdue_turns(state: &AppState) {
    let mut finished = Vec::new();
    {
        let mut games = state.games.write().await;
        for game in games.values_mut() {
            if game.apply_move_timeout() {
                tracing::info!(game_id = %game.id, seat = game.current_seat, "seat auto-retired for exceeding the move time limit");
                if let Err(error) = persistence::save_game(&state.db, game).await {
                    tracing::error!(game_id = %game.id, %error, "failed to persist timeout retirement");
                }
                finished.push(game.to_dto());
            }
        }
    }
    for dto in finished {
        let _ = state.events.send(GameEventDto::GameFinished { game: dto });
    }
}

/// Permanently deletes any game finished more than 7 days ago — chat,
/// moves, participants, and invitations all go with it (`persistence::delete_game`
/// is the same cascading delete admin's "delete game" uses). No background
/// scheduler: called lazily from `list_games`, same as `expire_overdue_turns`.
///
/// Concurrency: two callers racing into this (e.g. two participants both
/// hitting `GET /games` at once) can't corrupt anything or double-fire a
/// broadcast — (1) the write lock is held across the *entire* sweep,
/// including the awaited deletes, exactly like `expire_overdue_turns`
/// already does, so a second concurrent caller simply waits for the first
/// sweep to finish rather than running alongside it; (2) every step is
/// independently idempotent as a second line of defense regardless of
/// locking — a SQL `delete ... where id = ?` on an already-gone row affects
/// zero rows, and removing an already-removed key from the map is a no-op.
async fn expire_old_finished_games(state: &AppState) {
    let now: u64 = now_iso().parse().unwrap_or(0);
    let cutoff = now.saturating_sub(7 * 24 * 60 * 60).to_string();
    let stale_ids = match persistence::list_finished_game_ids_older_than(&state.db, &cutoff).await
    {
        Ok(ids) => ids,
        Err(error) => {
            tracing::error!(%error, "failed to query finished games for expiry");
            return;
        }
    };
    if stale_ids.is_empty() {
        return;
    }

    let mut games = state.games.write().await;
    for game_id in stale_ids {
        match persistence::delete_game(&state.db, &game_id).await {
            Ok(_) => {
                games.remove(&game_id);
                tracing::info!(game_id, "finished game auto-deleted after 7 days");
            }
            Err(error) => {
                tracing::error!(game_id, %error, "failed to auto-delete expired game");
            }
        }
    }
}

/// Same as `expire_overdue_turns` but scoped to one game — cheaper for
/// handlers that already know which game they care about.
async fn expire_overdue_turn(state: &AppState, game_id: &str) {
    let finished = {
        let mut games = state.games.write().await;
        let Some(game) = games.get_mut(game_id) else {
            return;
        };
        if !game.apply_move_timeout() {
            return;
        }
        tracing::info!(game_id, seat = game.current_seat, "seat auto-retired for exceeding the move time limit");
        if let Err(error) = persistence::save_game(&state.db, game).await {
            tracing::error!(game_id, %error, "failed to persist timeout retirement");
        }
        game.to_dto()
    };
    let _ = state.events.send(GameEventDto::GameFinished { game: finished });
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
    tracing::warn!(player_id, "admin: user deleted");
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
    tracing::warn!(player_id, "admin: password reset");
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
                    // Not meaningful in an admin summary view.
                    invitation_status: None,
                    invited_email: participant.invited_email.clone(),
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
    tracing::warn!(game_id, "admin: game deleted");
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
    tracing::warn!(game_id, "admin: game force-ended");
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
    use api::{CreateSeatRequest, GameStateDto, SeatClaim, SeatKind};
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use rules_shared::{
        Direction, ENABLE2K, GameState, GERMAN, Letter, MoveCandidate, MoveGenerator, Position,
        Rack, RulesEngine, SOWPODS, Tile, TilePlacement, VariantRules,
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
            crate::email::EmailConfig::new(None, "Tile Lite Elite <noreply@mail.tileliteelite.com>".to_string()),
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
            !summary.last_activity_at.is_empty() && summary.last_activity_at != "unknown",
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
        let board = board_from_dto(&started.board, &rules.alphabet)
            .expect("board dto should reconstruct");
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
        let board = board_from_dto(&started.game.board, &rules.alphabet)
            .expect("board dto should reconstruct");
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
        let board = board_from_dto(&started.game.board, &rules.alphabet)
            .expect("board dto should reconstruct");
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

        let official_game =
            create_and_start(app.clone(), &alice.session_token, None, 501).await;
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
        let board = board_from_dto(&started.board, &rules.alphabet)
            .expect("board dto should reconstruct");
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

        let board = board_from_dto(&started.board, &rules.alphabet)
            .expect("board dto should reconstruct");
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
        assert_eq!(updated.participants[0].score, (3 + 1 + 1 + 1 + 1 * 2) * 2);
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

        TwoHumanGame { game: created, alice, bob }
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
        TwoHumanGame { game: started, alice: waiting.alice, bob: waiting.bob }
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
                        api::TileDto::Letter { letter: "A".to_string() },
                        api::TileDto::Letter { letter: "T".to_string() },
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
            send_empty_auth(app.clone(), Method::GET, "/games", Some(&started.alice.session_token))
                .await,
        )
        .await;
        assert!(
            !alice_games.iter().any(|summary| summary.id == started.game.id),
            "Alice removed this game, it should no longer appear in her list"
        );

        let bob_games: Vec<api::GameSummaryDto> = read_json(
            send_empty_auth(app.clone(), Method::GET, "/games", Some(&started.bob.session_token)).await,
        )
        .await;
        assert!(
            bob_games.iter().any(|summary| summary.id == started.game.id),
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
                tile: api::TileDto::Letter { letter: "A".to_string() },
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

        let unauthenticated =
            send_empty(app.clone(), Method::GET, &format!("/games/{}", started.game.id)).await;
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
        assert!(as_alice.racks[1].counts.is_empty(), "opponent's rack must stay redacted");
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
        let now: u64 = now_iso().parse().unwrap_or(0);
        let eight_days_ago = (now - 8 * 24 * 60 * 60).to_string();
        let one_day_ago = (now - 24 * 60 * 60).to_string();
        sqlx::query("update games set status = 'finished', ended_at = ?1 where id = ?2")
            .bind(&eight_days_ago)
            .bind(&old.id)
            .execute(&state.db)
            .await
            .expect("update should succeed");
        sqlx::query("update games set status = 'finished', ended_at = ?1 where id = ?2")
            .bind(&one_day_ago)
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
            turn_started_at: "0".to_string(),
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
        let expires_at: u64 = record.expires_at.expect("should have an expiry").parse().unwrap();
        let now: u64 = super::now_iso().parse().unwrap();
        // Should be ~7 days out — just confirm it's in the right ballpark
        // rather than pinning an exact second.
        assert!(expires_at > now + 6 * 24 * 60 * 60);
        assert!(expires_at < now + 8 * 24 * 60 * 60);
    }

    #[tokio::test]
    async fn logging_in_with_stay_logged_in_gets_a_session_that_never_expires() {
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
        assert!(record.stay_logged_in);
        assert_eq!(record.expires_at, None);
    }

    #[tokio::test]
    async fn expired_session_token_is_rejected_and_a_stay_logged_in_one_is_not() {
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state.clone());

        let expired_token = "expired-token";
        persistence::create_session(
            &state.db,
            &Uuid::new_v4().to_string(),
            &register_player(app.clone(), "Alice").await.player_id,
            &hash_token(expired_token),
            false,
            Some("0"), // already expired (epoch second 0)
        )
        .await
        .expect("session should be created");

        let never_expires_token = "never-expires-token";
        persistence::create_session(
            &state.db,
            &Uuid::new_v4().to_string(),
            &register_player(app.clone(), "Bob").await.player_id,
            &hash_token(never_expires_token),
            true,
            None,
        )
        .await
        .expect("session should be created");

        let expired_response = send_empty_auth(app.clone(), Method::GET, "/games", Some(expired_token)).await;
        assert_eq!(expired_response.status(), StatusCode::UNAUTHORIZED);

        let stay_logged_in_response =
            send_empty_auth(app, Method::GET, "/games", Some(never_expires_token)).await;
        assert_eq!(stay_logged_in_response.status(), StatusCode::OK);
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

        // alice: already expired; bob: expires far in the future; carol:
        // stay_logged_in, no expiry at all. Only alice's should be deleted.
        persistence::create_session(
            &db,
            &Uuid::new_v4().to_string(),
            &alice.player_id,
            &hash_token("alice-expired"),
            false,
            Some("0"),
        )
        .await
        .expect("session should be created");
        persistence::create_session(
            &db,
            &Uuid::new_v4().to_string(),
            &bob.player_id,
            &hash_token("bob-future"),
            false,
            Some("9999999999"),
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

    #[tokio::test]
    async fn search_players_matches_by_prefix_case_insensitively_and_requires_auth() {
        let database_url = test_database_url();
        let state = create_test_state(&database_url).await;
        let app = build_router(state);

        let alice = register_player(app.clone(), "Alice").await;
        register_player(app.clone(), "Alicia").await;
        register_player(app.clone(), "Bob").await;

        let unauthenticated = send_empty_auth(app.clone(), Method::GET, "/players/search?q=ali", None).await;
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let names: Vec<String> = read_json(
            send_empty_auth(app, Method::GET, "/players/search?q=ALI", Some(&alice.session_token)).await,
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
            send_empty_auth(app.clone(), Method::GET, "/games", Some(&owner.session_token)).await,
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
            &api::SwapSeatsRequest { seat_a: 0, seat_b: 1 },
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
            &api::SwapSeatsRequest { seat_a: 0, seat_b: 1 },
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
                claim: Some(SeatClaim::Named { display_name: "Carol".to_string() }),
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
                claim: Some(SeatClaim::Named { display_name: "Carol".to_string() }),
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
                    claim: Some(SeatClaim::Named { display_name: "Carol".to_string() }),
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
                claim: Some(SeatClaim::Named { display_name: "Carol".to_string() }),
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
        assert_eq!(updated.participants.len(), 1, "Bob's claimed seat is gone entirely");

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
            &api::SwapSeatsRequest { seat_a: 0, seat_b: 1 },
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
            &api::SwapSeatsRequest { seat_a: 0, seat_b: 1 },
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
                    stay_logged_in: false,
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

        // Alice's session was deleted along with her account, and nobody
        // else is tied to this game (the seat is now unclaimed, and admin
        // deletion doesn't touch `creator_player_id` — see its own doc
        // comment), so there's no longer a legitimate caller who could
        // fetch it through the normal player-facing endpoint. Assert
        // directly on the in-memory state instead, which is also a more
        // direct check of what this test actually cares about.
        let games = state.games.read().await;
        let fetched = games.get(&created.id).expect("the game itself should survive");
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
            send_admin::<()>(app.clone(), Method::GET, "/admin/games", loopback_peer(), None)
                .await,
        )
        .await;
        let listed_game = listed
            .iter()
            .find(|game| game.id == created.game.id)
            .expect("created game should be listed");
        assert!(!listed_game.created_at.is_empty());

        let delete_response = send_admin::<()>(
            app.clone(),
            Method::DELETE,
            &format!("/admin/games/{}", created.game.id),
            loopback_peer(),
            None,
        )
        .await;
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

        let fetch_after =
            send_empty(app.clone(), Method::GET, &format!("/games/{}", created.game.id)).await;
        assert_eq!(fetch_after.status(), StatusCode::NOT_FOUND);

        let listed_after: Vec<AdminGameSummaryDto> = read_json(
            send_admin::<()>(app, Method::GET, "/admin/games", loopback_peer(), None).await,
        )
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

        let mallory_games: Vec<api::GameSummaryDto> = read_json(
            send_empty_auth(
                app,
                Method::GET,
                "/games",
                Some(&mallory.session_token),
            )
            .await,
        )
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
        let response = send_empty(app, Method::GET, &format!("/invitations/{invitation_id}/preview")).await;
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
        assert_eq!(joined.participants[1].player_id.as_deref(), Some(carol.player_id.as_str()));
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
        assert_eq!(accepted.participants[1].player_id.as_deref(), Some(bob.player_id.as_str()));
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
            game.turn_started_at = "0".to_string();
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
        let last_move = fetched.moves.last().expect("a timeout move should be recorded");
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
        let old_session_response =
            send_empty_auth(app.clone(), Method::GET, "/games", Some(&alice.session_token)).await;
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
        assert_eq!(rows.len(), 1, "still exactly one token after the second request");
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
            "0", // already expired (epoch second 0)
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
        let far_future = (now_iso().parse::<u64>().unwrap() + 3600).to_string();
        persistence::create_password_reset_token(
            &state.db,
            &Uuid::new_v4().to_string(),
            &alice.player_id,
            &hash_token(&token),
            &far_future,
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
        let far_future = (now_iso().parse::<u64>().unwrap() + 3600).to_string();
        persistence::create_password_reset_token(
            &state.db,
            &Uuid::new_v4().to_string(),
            &alice.player_id,
            &hash_token(&token),
            &far_future,
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
}
