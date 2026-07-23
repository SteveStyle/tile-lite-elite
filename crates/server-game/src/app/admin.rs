use super::*;

pub(crate) async fn require_loopback(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if !addr.ip().is_loopback() {
        return ApiProblem::forbidden("Admin endpoints are only reachable from the server itself")
            .into_response();
    }
    next.run(request).await
}

// Reachable only from loopback (see `require_loopback`) — an operator with
// terminal access to the server, not player-facing, hence no per-account
// auth here.

pub(crate) async fn admin_list_users(
    State(state): State<AppState>,
) -> Result<Json<Vec<PlayerDto>>, ApiProblem> {
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

pub(crate) async fn admin_delete_user(
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

pub(crate) async fn admin_reset_password(
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
pub(crate) struct AdminGamesQuery {
    status: Option<String>,
    older_than_days: Option<i64>,
}

pub(crate) async fn admin_list_games(
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
    let cutoff = query
        .older_than_days
        .map(|days| now_unix_seconds() - days * 86_400);

    let games = state.games.read().await;
    let mut summaries: Vec<AdminGameSummaryDto> = games
        .values()
        .filter(|game| {
            status_filter
                .as_ref()
                .is_none_or(|status| &game.status == status)
        })
        .filter(|game| {
            let Some(cutoff) = cutoff else {
                return true;
            };
            created_at
                .get(&game.id)
                .is_some_and(|created| *created <= cutoff)
        })
        .map(|game| AdminGameSummaryDto {
            id: game.id.clone(),
            status: game.status,
            created_at: created_at.get(&game.id).copied().unwrap_or(0),
            last_activity_at: last_activity.get(&game.id).copied().unwrap_or(0),
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
                    rating_before: None,
                    rating_after: None,
                    current_rating: None,
                    resigned: participant.resigned,
                })
                .collect(),
        })
        .collect();
    summaries.sort_by_key(|s| std::cmp::Reverse(s.created_at));

    Ok(Json(summaries))
}

pub(crate) async fn admin_delete_game(
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
pub(crate) async fn admin_force_end_game(
    State(state): State<AppState>,
    Path(game_id): Path<String>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let mut dto = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;
        game.admin_force_finish();
        let dto = game.to_dto();
        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        dto
    };
    stats::attach_current_ratings(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    // Always a no-op in practice — an admin force-end never moves rating
    // (see `stats::settle_ratings`) — but calling it anyway keeps this
    // handler consistent with every other place a Finished game's DTO
    // goes out, rather than being a special case someone has to remember.
    stats::attach_rating_deltas(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    tracing::warn!(game_id, "admin: game force-ended");
    let _ = state
        .events
        .send(GameEventDto::GameFinished { game: dto.clone() });
    Ok(Json(dto))
}
