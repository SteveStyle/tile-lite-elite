use super::*;

/// Whether `caller_player_id` may perform a game-*management* action —
/// start, reorder, add/remove/invite a seat, force-resign. The creator is
/// the game's manager; everyone else (including a seated participant) is
/// not. A game persisted before `creator_player_id` existed (`None`) falls
/// back to the old, more permissive rule instead of becoming permanently
/// unmanageable: any claimed-seat owner, or (for an all-engine game, which
/// has no claimed seats at all) any signed-in caller.
pub(crate) fn caller_may_manage_game(game: &GameSession, caller_player_id: Option<&str>) -> bool {
    match game.creator_player_id.as_deref() {
        Some(creator_id) => caller_player_id == Some(creator_id),
        None => {
            let claimed_owners: Vec<&str> = game
                .participants
                .iter()
                .filter_map(|participant| participant.player_id.as_deref())
                .collect();
            claimed_owners.is_empty()
                || caller_player_id.is_some_and(|id| claimed_owners.contains(&id))
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
pub(crate) async fn game_dto_with_invitation_status(
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
pub(crate) async fn authenticated_player_id(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<String> {
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
pub(crate) async fn player_id_for_token(state: &AppState, token: &str) -> Option<String> {
    let session = persistence::get_session_by_token_hash(&state.db, &hash_token(token))
        .await
        .ok()??;

    if let Some(expires_at) = &session.expires_at
        && let (Ok(expiry), Ok(now)) = (expires_at.parse::<u64>(), now_iso().parse::<u64>())
        && now > expiry
    {
        return None;
    }

    Some(session.player_id)
}

pub(crate) fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs();
    seconds.to_string()
}
