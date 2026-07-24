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
    move_candidate_from_dto, now_unix_seconds, redact_game_state, resolve_viewer_access,
    tile_from_dto,
};
use crate::persistence;
use crate::stats;
use rules_shared::format_move_error;

// Handler modules split out of what was once one ~8.6k-line file. Each
// submodule does `use super::*;` to pull in this module's imports,
// `AppState`, and its sibling handlers/helpers; `build_router` wires them
// together. See docs/1.2-components-and-interactions.md.
mod admin;
mod auth;
mod catalog;
mod common;
mod error;
mod events;
mod games;
mod invitations;
mod ratings;
mod roster;
mod sweeps;
#[cfg(test)]
mod tests;

use self::admin::*;
use self::auth::*;
use self::catalog::*;
use self::common::*;
use self::error::*;
use self::events::*;
use self::games::*;
use self::invitations::*;
use self::ratings::*;
use self::roster::*;
use self::sweeps::*;

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
    /// own doc comment in `docs/4.1-configuration.md` for why the *client* doesn't
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
    // set to (docs/3.2-development.md documents binding to 0.0.0.0 for LAN
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
        .route(
            "/admin/games/{game_id}/force-end",
            post(admin_force_end_game),
        )
        .layer(middleware::from_fn(require_loopback));

    Router::new()
        .route("/health", get(health))
        .route("/engines", get(list_engines))
        .route("/dictionaries/{name}", get(get_dictionary))
        // Authentication
        .route("/auth/register", post(register_player))
        .route("/auth/login", post(login_player))
        .route("/auth/validate", post(validate_session))
        .route("/auth/logout", post(logout))
        .route("/auth/change-password", post(change_password))
        .route("/auth/update-details", post(update_player_details))
        .route("/auth/forgot-password", post(request_password_reset))
        .route("/auth/reset-password", post(reset_password))
        .route("/players/search", get(search_players))
        // Rating & Stats
        .route("/players/{player_id}/stats", get(get_player_stats))
        .route(
            "/players/{player_id}/rating-history",
            get(get_player_rating_history),
        )
        .route("/engines/{engine_id}/stats", get(get_engine_stats))
        .route(
            "/engines/{engine_id}/rating-history",
            get(get_engine_rating_history),
        )
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
        .route("/games/{game_id}/abort", post(abort_game))
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
    format_app_version(
        env!("CARGO_PKG_VERSION"),
        option_env!("TILE_LITE_ELITE_BUILD_ID"),
    )
}

fn format_app_version(pkg_version: &str, build_id: Option<&str>) -> String {
    match build_id {
        Some(id) if !id.is_empty() => format!("{pkg_version}+{id}"),
        _ => pkg_version.to_string(),
    }
}
