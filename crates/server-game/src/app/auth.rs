use super::*;

// ========== Authentication Handlers ==========

pub(crate) async fn register_player(
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
    let expires_at = session_expiry();
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

pub(crate) async fn login_player(
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
    let expires_at = session_expiry();
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

pub(crate) async fn validate_session(
    State(state): State<AppState>,
    Json(request): Json<ValidateSessionRequest>,
) -> Result<Json<PlayerDto>, ApiProblem> {
    // Routed through the shared token check so startup validation enforces
    // the same absolute + idle limits (and refreshes `last_seen_at`) as
    // every other authenticated request — otherwise a dormant or expired
    // session would still validate here until the lazy sweep removed it.
    let player_id = player_id_for_token(&state, &request.session_token)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Session is invalid or has expired"))?;

    let player = persistence::get_player_by_id(&state.db, &player_id)
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

/// Explicit log-out: deletes the session behind the presented bearer token,
/// invalidating it immediately — the precise signal, versus idle expiry's
/// eventual guess. Always `204`: an unknown or already-gone token is a
/// no-op, so a client tearing down its own state never has to care whether
/// the row was still there.
pub(crate) async fn logout(State(state): State<AppState>, headers: HeaderMap) -> StatusCode {
    if let Some(token) = bearer_token(&headers) {
        let _ = persistence::delete_session_by_token_hash(&state.db, &hash_token(token)).await;
    }
    StatusCode::NO_CONTENT
}

#[derive(serde::Deserialize)]
pub(crate) struct SearchPlayersQuery {
    q: String,
}

/// "Invite by name" autocomplete — display names aren't sensitive (already
/// shown throughout every game's participant list), so this is only
/// gated on being signed in at all, not on any relationship to a specific
/// game or invitee.
pub(crate) async fn search_players(
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

pub(crate) async fn change_password(
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
        tracing::warn!(
            player_id,
            "password change rejected: wrong current password"
        );
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
pub(crate) async fn update_player_details(
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
    if request
        .display_name
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(ApiProblem::bad_request("Display name cannot be blank"));
    }
    if request
        .email
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
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
    if let Some(display_name) = display_name
        && let Some(existing) = persistence::get_player_by_name(&state.db, display_name)
            .await
            .map_err(ApiProblem::from_sqlx)?
        && existing.id != player_id
    {
        return Err(ApiProblem::bad_request(
            "That display name is already taken",
        ));
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
pub(crate) async fn request_password_reset(
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
pub(crate) async fn reset_password(
    State(state): State<AppState>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<StatusCode, ApiProblem> {
    if request.new_password.is_empty() {
        return Err(ApiProblem::bad_request("A new password is required"));
    }

    let invalid = || ApiProblem::bad_request("This reset link is invalid or has expired");

    let record =
        persistence::get_password_reset_token_by_hash(&state.db, &hash_token(&request.token))
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

// ========== Helper Functions ==========

/// Argon2 is deliberately slow, which is exactly right for a human-chosen
/// password (resists brute-force guessing) but wrong for a session token
/// looked up on every request — that uses `hash_token` (sha256) instead,
/// since a UUIDv4 token already has enough entropy that a fast hash is safe.
pub(crate) fn hash_password(password: &str) -> Result<String, String> {
    use argon2::password_hash::{SaltString, rand_core::OsRng};
    use argon2::{Argon2, PasswordHasher};

    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| error.to_string())
}

pub(crate) fn verify_password(password: &str, stored_hash: &str) -> bool {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};

    let Ok(parsed_hash) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

pub(crate) fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// How long a password-reset link stays valid after it's requested. Short
/// enough that a link sitting unread in an inbox for days can't be used,
/// long enough that it isn't a race against actually receiving the email.
pub(crate) const PASSWORD_RESET_TOKEN_TTL_SECS: u64 = 60 * 60;

/// A new session's absolute `expires_at`: `SESSION_MAX_LIFETIME_SECS` out
/// from now, the same for every session. Even a continuously-active session
/// must re-authenticate once it passes this; a dormant one is logged out
/// sooner by the idle window (enforced in `player_id_for_token`). The
/// `stay_logged_in` flag deliberately doesn't enter into it — that's now a
/// purely client-side concern (whether the token survives a browser
/// restart), not a server-side lifetime. Shared by `register_player` and
/// `login_player` so both compute it identically.
pub(crate) fn session_expiry() -> Option<String> {
    now_iso()
        .parse::<u64>()
        .map(|now| (now + persistence::SESSION_MAX_LIFETIME_SECS).to_string())
        .ok()
}
