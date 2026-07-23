use super::*;

pub(crate) async fn list_games(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<api::GameSummaryDto>>, ApiProblem> {
    // The list is inherently personal — which games show up depends on who's
    // asking — so there's no meaningful "browse everything" mode anymore.
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to see your games"))?;

    expire_overdue_turns(&state).await;
    send_move_time_reminders(&state).await;
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
        let last_activity_at = last_activity.get(&game.id).copied().unwrap_or(0);

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

    summaries.sort_by_key(|s| std::cmp::Reverse(s.last_activity_at));

    Ok(Json(summaries))
}

pub(crate) async fn create_game(
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
        .filter(|seat| {
            seat.kind == api::SeatKind::Human && matches!(seat.claim, Some(api::SeatClaim::Creator))
        })
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
                .ok_or_else(|| {
                    ApiProblem::not_found(format!("No player named '{display_name}'"))
                })?;
            named_invitees.insert(seat_number as u8, player);
        }
    }

    let variant_name = request.variant.as_deref().unwrap_or("official");
    let rules = rules_shared::VariantRules::by_name(variant_name)
        .ok_or_else(|| ApiProblem::bad_request(format!("Unknown game variant '{variant_name}'")))?;
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
                reminder_sent_turn: None,
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

        if let (Some(invited_player), Some(creator_display_name)) =
            (invited_player, &creator_display_name)
        {
            crate::email::send_invitation(
                &state.email,
                &invited_player.email,
                &invited_player.display_name,
                creator_display_name,
                &state.public_base_url,
            )
            .await;
        } else if let (Some(invited_email), Some(creator_display_name)) =
            (invited_email, &creator_display_name)
        {
            let join_url = format!("{}/invite?id={}", state.public_base_url, record.id);
            crate::email::send_join_invitation(
                &state.email,
                invited_email,
                creator_display_name,
                &join_url,
            )
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
    let mut dto = game_dto_with_invitation_status(&state, &game).await?;
    stats::attach_current_ratings(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    let dto = redact_game_state(dto, &access);
    state.games.write().await.insert(game.id.clone(), game);
    Ok(Json(dto))
}

pub(crate) async fn get_game(
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
    let mut dto = game_dto_with_invitation_status(&state, game).await?;
    drop(games);
    stats::attach_current_ratings(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    // Not just for the moment a game finishes — reopening an old finished
    // game later should still show what it did to your rating, not only
    // the one-time broadcast at the moment it happened. A no-op query
    // whenever `dto.status` isn't `Finished`.
    stats::attach_rating_deltas(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    Ok(Json(redact_game_state(dto, &access)))
}

pub(crate) async fn start_game(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(_request): Json<StartGameRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    {
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
    }

    // Deliberately released before this — see `run_engine_turns`'s own doc
    // comment for why holding the lock across the whole multi-turn loop
    // would starve a WebSocket connection's own read lock for the entire
    // duration of an all-engine game.
    run_engine_turns(&state, &game_id).await?;

    let (mut dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;
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
    stats::attach_current_ratings(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    // An all-engine game (e.g. Bot Showdown) can finish entirely within
    // `run_engine_turns` above, before this handler ever gets to broadcast
    // anything — so `dto.status` may already be `Finished` here, and a
    // seated participant's rating may have just moved.
    stats::attach_rating_deltas(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;

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

pub(crate) async fn submit_action(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<GameActionRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    expire_overdue_turn(&state, &game_id).await;

    {
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
            && seat.kind == api::SeatKind::Human
            && caller_player_id.as_deref() != seat.player_id.as_deref()
        {
            return Err(ApiProblem::unauthorized(
                "This seat belongs to a different player",
            ));
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
    }

    // Deliberately released before this — see `run_engine_turns`'s own doc
    // comment for why holding the lock across the whole multi-turn loop
    // would starve a WebSocket connection's own read lock.
    run_engine_turns(&state, &game_id).await?;

    let (mut dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;
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
    // A no-op unless this move (or a follow-on engine turn triggered by
    // `run_engine_turns` above) just finished the game via a normal
    // ending or a voluntary resignation — the only endings that move
    // rating (see `stats::settle_ratings`).
    stats::attach_rating_deltas(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;

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
pub(crate) async fn post_chat_message(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PostChatMessageRequest>,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to chat"))?;

    let (mut dto, access) = {
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

/// Hides a finished game from the caller's own games list — see
/// `GameSession::remove_for_player`. Not broadcast over the WebSocket: this
/// is purely a per-viewer concern, so nobody else's view of the game (or
/// even this same player's other logged-in devices, until they next reload
/// their own list) needs to change in response.
pub(crate) async fn remove_game_for_player(
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

pub(crate) async fn preview_move(
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
    if let Some(seat) = game.participants.get(request.seat_number as usize)
        && seat.kind == api::SeatKind::Human
        && caller_player_id.as_deref() != seat.player_id.as_deref()
    {
        return Err(ApiProblem::unauthorized(
            "This seat belongs to a different player",
        ));
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

pub(crate) async fn suggest_move(
    Path(game_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<api::GameStateDto>, ApiProblem> {
    let caller_player_id = authenticated_player_id(&state, &headers).await;

    expire_overdue_turn(&state, &game_id).await;

    {
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
            if let Ok(validated) = engine.validate_game_move(&game.state, Some(&rack), &candidate)
                && validated.score.total > best_score
            {
                best_score = validated.score.total;
                best_candidate = Some(candidate);
            }
        }

        let seat = game.current_seat;
        match best_candidate {
            Some(candidate) => game
                .apply_place_move(seat, candidate)
                .map_err(ApiProblem::bad_request)?,
            None => game.apply_pass(seat).map_err(ApiProblem::bad_request)?,
        }
    }

    // Deliberately released before this — see `run_engine_turns`'s own doc
    // comment for why holding the lock across the whole multi-turn loop
    // would starve a WebSocket connection's own read lock.
    run_engine_turns(&state, &game_id).await?;

    let (mut dto, access) = {
        let mut games = state.games.write().await;
        let game = games
            .get_mut(&game_id)
            .ok_or_else(|| ApiProblem::not_found("Game not found"))?;
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
    // A no-op unless this move (or a follow-on engine turn triggered by
    // `run_engine_turns` above) just finished the game via a normal
    // ending or a voluntary resignation — the only endings that move
    // rating (see `stats::settle_ratings`).
    stats::attach_rating_deltas(&state.db, &mut dto)
        .await
        .map_err(ApiProblem::from_sqlx)?;

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

/// How long an engine gets to choose an action before the seat auto-passes.
/// Hobby-project default; not yet configurable per engine or per game.
pub(crate) const ENGINE_TURN_TIMEOUT: Duration = Duration::from_secs(5);

/// Safety ceiling on engine turns run per trigger (a `/start` or `/actions`
/// call). This isn't meant to be hit in practice: `maybe_run_engine_turn`
/// already stops as soon as the current seat isn't an engine, and a real
/// game is bounded by its tile bag (well under 200 turns even worst-case).
/// It exists only to keep a future buggy engine from hanging a request
/// forever in an all-engine game, where there's no human seat to naturally
/// break the loop.
pub(crate) const MAX_ENGINE_TURNS_PER_TRIGGER: usize = 400;

/// Paced gap between broadcasting one engine turn and computing the next.
/// A greedy move search on a 15x15 board is fast enough (single-digit
/// milliseconds) that an all-engine game would otherwise finish before a
/// freshly-opened WebSocket even completes its handshake, let alone before
/// the client can render each state — so *some* artificial pacing is
/// required for the moves to be visible at all, not just theoretically
/// broadcast. Short enough that a full engine-vs-engine game (well under
/// `MAX_ENGINE_TURNS_PER_TRIGGER`) still finishes in a few seconds.
pub(crate) const ENGINE_TURN_BROADCAST_DELAY: Duration = Duration::from_millis(120);

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
///
/// Takes `&AppState`/`game_id` and reacquires `state.games`'s write lock
/// fresh each turn, rather than taking an already-locked `&mut GameSession`
/// and holding that one lock for the whole loop — holding it continuously
/// starved every other request needing this lock (most importantly a
/// WebSocket connection's own read lock in `game_events`) for the entire
/// multi-turn duration, silently defeating the "subscribe before
/// triggering" design above: the socket's `state.games.read()` would block
/// until the whole game had already finished and every broadcast already
/// sent to nobody. A turn is single-digit milliseconds against the 120ms
/// pacing gap below, so releasing the lock between turns (before that sleep
/// and before the broadcast send, both lock-free) still leaves plenty of
/// room for a pending reader to get in.
pub(crate) async fn run_engine_turns(state: &AppState, game_id: &str) -> Result<(), ApiProblem> {
    for _ in 0..MAX_ENGINE_TURNS_PER_TRIGGER {
        let outcome = {
            let mut games = state.games.write().await;
            let Some(game) = games.get_mut(game_id) else {
                // Game vanished from under us (e.g. deleted concurrently) —
                // nothing left to advance.
                return Ok(());
            };
            let advanced = game
                .maybe_run_engine_turn(&state.engines, ENGINE_TURN_TIMEOUT)
                .await
                .map_err(ApiProblem::bad_request)?;
            advanced.then(|| {
                let dto = game.to_dto();
                let finished = game.status == api::GameStatus::Finished;
                (dto, finished)
            })
        };
        let Some((dto, finished)) = outcome else {
            break;
        };
        let event = if finished {
            GameEventDto::GameFinished { game: dto }
        } else {
            GameEventDto::StateUpdated { game: dto }
        };
        let _ = state.events.send(event);
        if finished {
            break;
        }
        tokio::time::sleep(ENGINE_TURN_BROADCAST_DELAY).await;
    }
    Ok(())
}
