use super::*;

/// There's no background scheduler in this server, so overdue-turn
/// retirement is checked lazily: call this at the top of any handler that
/// reads or acts on live games, and any seat that's overrun its
/// `move_time_limit_seconds` gets auto-retired (same effect as resigning)
/// before the rest of the handler runs. Persists and broadcasts every game
/// it changes.
pub(crate) async fn expire_overdue_turns(state: &AppState) {
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
    for dto in &mut finished {
        if let Err(error) = stats::attach_current_ratings(&state.db, dto).await {
            tracing::error!(game_id = %dto.id, %error, "failed to read current ratings after timeout retirement");
        }
    }
    // Always a no-op — a timeout never moves rating (see
    // `stats::settle_ratings`) — kept for consistency with every other
    // place a Finished game's DTO goes out.
    for dto in &mut finished {
        if let Err(error) = stats::attach_rating_deltas(&state.db, dto).await {
            tracing::error!(game_id = %dto.id, %error, "failed to read rating deltas after timeout retirement");
        }
    }
    for dto in finished {
        let _ = state.events.send(GameEventDto::GameFinished { game: dto });
    }
}

/// Move-time-limit fraction remaining at which a reminder email fires —
/// see `send_move_time_reminders`.
pub(crate) const REMINDER_REMAINING_FRACTION: u64 = 3;

/// Games with a same-day move-time-limit don't get reminders — a limit
/// that short doesn't leave enough runway for one to be useful.
pub(crate) const REMINDER_MIN_TIME_LIMIT_SECONDS: u64 = 24 * 60 * 60;

/// Same lazy-sweep pattern as `expire_overdue_turns` (no background
/// scheduler in this server — see its doc comment): called from
/// `list_games`, checks every active game whose `move_time_limit_seconds`
/// exceeds a day, and emails the seat on turn once its remaining time
/// drops to a third of that limit (e.g. 24h remaining on the default 72h
/// limit). Fires at most once per turn, tracked via
/// `ParticipantState::reminder_sent_turn`, and only for claimed human
/// seats — engines never run out the clock in a way anyone needs telling
/// about, and an unclaimed seat has no one to email.
pub(crate) async fn send_move_time_reminders(state: &AppState) {
    struct Reminder {
        game_id: String,
        seat: u8,
        player_id: String,
        display_name: String,
        remaining_seconds: u64,
    }

    let mut reminders = Vec::new();
    {
        let mut games = state.games.write().await;
        for game in games.values_mut() {
            if game.move_time_limit_seconds <= REMINDER_MIN_TIME_LIMIT_SECONDS {
                continue;
            }
            let Some(remaining) = game.seconds_remaining_on_turn() else {
                continue;
            };
            if remaining * REMINDER_REMAINING_FRACTION > game.move_time_limit_seconds {
                continue;
            }
            let seat = game.current_seat;
            let turn_number = game.turn_number;
            let Some(participant) = game.participants.get_mut(seat as usize) else {
                continue;
            };
            if participant.kind != api::SeatKind::Human
                || participant.reminder_sent_turn == Some(turn_number)
            {
                continue;
            }
            let Some(player_id) = participant.player_id.clone() else {
                continue;
            };
            participant.reminder_sent_turn = Some(turn_number);
            let display_name = participant.display_name.clone();

            if let Err(error) = persistence::save_game(&state.db, game).await {
                tracing::error!(game_id = %game.id, %error, "failed to persist move-time reminder flag");
            }
            reminders.push(Reminder {
                game_id: game.id.clone(),
                seat,
                player_id,
                display_name,
                remaining_seconds: remaining,
            });
        }
    }

    for reminder in reminders {
        let player = match persistence::get_player_by_id(&state.db, &reminder.player_id).await {
            Ok(Some(player)) => player,
            Ok(None) => continue,
            Err(error) => {
                tracing::error!(game_id = %reminder.game_id, seat = reminder.seat, %error, "failed to look up player for move-time reminder");
                continue;
            }
        };
        tracing::info!(game_id = %reminder.game_id, seat = reminder.seat, "move-time reminder sent");
        crate::email::send_move_time_reminder(
            &state.email,
            &player.email,
            &reminder.display_name,
            &format_duration_days_hours(reminder.remaining_seconds),
            &state.public_base_url,
        )
        .await;
    }
}

/// "1 day 4 hours" / "1 day" / "4 hours" style label for the reminder
/// email's body — coarser than the UI's `format_time_remaining` (spelled
/// out, not `d`/`h` shorthand) since this reads as a sentence.
pub(crate) fn format_duration_days_hours(total_seconds: u64) -> String {
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let day_part = (days > 0).then(|| format!("{days} day{}", if days == 1 { "" } else { "s" }));
    let hour_part =
        (hours > 0).then(|| format!("{hours} hour{}", if hours == 1 { "" } else { "s" }));
    match (day_part, hour_part) {
        (Some(d), Some(h)) => format!("{d} {h}"),
        (Some(d), None) => d,
        (None, Some(h)) => h,
        (None, None) => "less than an hour".to_string(),
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
pub(crate) async fn expire_old_finished_games(state: &AppState) {
    let now: u64 = now_iso().parse().unwrap_or(0);
    let cutoff = now.saturating_sub(7 * 24 * 60 * 60).to_string();
    let stale_ids = match persistence::list_finished_game_ids_older_than(&state.db, &cutoff).await {
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
pub(crate) async fn expire_overdue_turn(state: &AppState, game_id: &str) {
    let mut finished = {
        let mut games = state.games.write().await;
        let Some(game) = games.get_mut(game_id) else {
            return;
        };
        if !game.apply_move_timeout() {
            return;
        }
        tracing::info!(
            game_id,
            seat = game.current_seat,
            "seat auto-retired for exceeding the move time limit"
        );
        if let Err(error) = persistence::save_game(&state.db, game).await {
            tracing::error!(game_id, %error, "failed to persist timeout retirement");
        }
        game.to_dto()
    };
    if let Err(error) = stats::attach_current_ratings(&state.db, &mut finished).await {
        tracing::error!(game_id, %error, "failed to read current ratings after timeout retirement");
    }
    // Always a no-op — a timeout never moves rating (see
    // `stats::settle_ratings`) — kept for consistency with every other
    // place a Finished game's DTO goes out.
    if let Err(error) = stats::attach_rating_deltas(&state.db, &mut finished).await {
        tracing::error!(game_id, %error, "failed to read rating deltas after timeout retirement");
    }
    let _ = state
        .events
        .send(GameEventDto::GameFinished { game: finished });
}

// ========== Admin Handlers ==========
//
