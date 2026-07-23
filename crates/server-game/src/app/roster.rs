use super::*;

/// Reorders two seats before the game starts — see `GameSession::
/// swap_seats` for why this is only ever offered once every seat is
/// filled. Only the creator may reorder (`caller_may_manage_game`).
pub(crate) async fn swap_seats(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SwapSeatsRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    let (mut dto, access) = {
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
    stats::attach_current_ratings(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;

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
pub(crate) async fn add_seat_to_game(
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

    let (mut dto, access) = {
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
            reminder_sent_turn: None,
        };
        game.add_seat(participant)
            .map_err(ApiProblem::bad_request)?;

        persistence::save_game(&state.db, game)
            .await
            .map_err(ApiProblem::from_sqlx)?;
        let dto = game_dto_with_invitation_status(&state, game).await?;
        let access = resolve_viewer_access(game, Some(&caller_player_id));
        (dto, access)
    };
    stats::attach_current_ratings(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;

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
pub(crate) async fn remove_seat_from_game(
    Path((game_id, seat_number)): Path<(String, u8)>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to remove a seat"))?;

    let (mut dto, access) = {
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
    stats::attach_current_ratings(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;

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
pub(crate) async fn withdraw_from_seat(
    Path((game_id, seat_number)): Path<(String, u8)>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to withdraw from a seat"))?;

    let (mut dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;

        let holds_seat = game
            .participants
            .get(seat_number as usize)
            .is_some_and(|p| p.player_id.as_deref() == Some(caller_player_id.as_str()));
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
            .filter(|invitation| {
                invitation.seat_number == seat_number && invitation.status == "accepted"
            })
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
    stats::attach_current_ratings(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;

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
pub(crate) async fn force_resign_seat(
    Path((game_id, seat_number)): Path<(String, u8)>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to force-resign a seat"))?;

    let (mut dto, access) = {
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
    stats::attach_current_ratings(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    // Always a no-op — a creator-forced resignation never moves rating
    // (see `stats::settle_ratings`) — kept for consistency with every
    // other place a Finished game's DTO goes out.
    stats::attach_rating_deltas(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;

    // Same GameFinished-vs-StateUpdated broadcast pattern as submit_action —
    // a force-resignation only finishes the whole game once at most one
    // active seat remains (see `GameSession::handle_seat_exit`); mirroring
    // the conditional rather than hardcoding GameFinished handles both
    // that case and a multi-player game simply continuing.
    let event = if dto.status == api::GameStatus::Finished {
        tracing::info!(game_id, seat_number, winner_seat = ?dto.winner_seat, "seat force-resigned by creator; game finished");
        GameEventDto::GameFinished { game: dto.clone() }
    } else {
        GameEventDto::StateUpdated { game: dto.clone() }
    };
    let _ = state.events.send(event);

    Ok(Json(redact_game_state(dto, &access)))
}
