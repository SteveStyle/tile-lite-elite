use super::*;

#[derive(Debug, serde::Deserialize)]
pub(crate) struct EventsQuery {
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
pub(crate) async fn game_events(
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
pub(crate) fn event_belongs_to_game(event: &GameEventDto, game_id: &str) -> bool {
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
pub(crate) fn redact_event(event: GameEventDto, access: &ViewerAccess) -> GameEventDto {
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

pub(crate) async fn stream_events(
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
