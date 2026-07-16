use api::{
    BoardCellDto, CreateGameRequest, DirectionDto, GameActionRequest, GameEventDto, GameStateDto,
    GameStatus, MoveCandidateDto, ParticipantDto, PositionDto, PremiumDto, RackDto, SeatKind,
    StartGameRequest, TileDto, TilePlacementDto,
};
use dioxus::prelude::*;
use futures_util::StreamExt;
use std::collections::{HashMap, HashSet};

#[cfg(target_arch = "wasm32")]
use gloo_net::{
    http::Request,
    websocket::{Message as WsMessage, futures::WebSocket},
};

#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::connect_async;

use crate::components::auth_panel::AuthPanel;
use crate::components::games_panel::GamesPanel;
use crate::views::{Home, ResetPassword};

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
pub(crate) const BOARD_WIDTH: usize = 15;
const BOARD_HEIGHT: usize = 15;
/// How often the background reconnect loop pings `/health` while the
/// server is unreachable.
const RECONNECT_POLL_MS: u64 = 3000;
/// Delay between WebSocket reconnect attempts.
const WEBSOCKET_RETRY_MS: u64 = 3000;
/// How often the games list (and with it, unread-chat mail icons) is
/// re-fetched in the background. The live WebSocket only covers the one
/// game currently open, so activity in any other game — a new chat
/// message, an opponent's move — wouldn't otherwise show up until the
/// player manually hits Refresh.
const GAME_LIST_POLL_MS: u64 = 10_000;

/// Whether the app can currently reach the backend at all — set by the
/// HTTP helpers (`get_json`/`post_json`) and the WebSocket subscription the
/// moment either one fails at the network level (server unreachable, not a
/// legitimate rejection response), and cleared the moment either succeeds.
/// Read from anywhere (the topbar indicator, the button-disabling checks,
/// the background reconnect loop) without threading it through props.
static IS_ONLINE: GlobalSignal<bool> = Signal::global(|| true);

#[cfg(not(target_arch = "wasm32"))]
async fn sleep_ms(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

#[cfg(target_arch = "wasm32")]
async fn sleep_ms(ms: u64) {
    gloo_timers::future::TimeoutFuture::new(ms as u32).await;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RackTileView {
    pub id: usize,
    pub display: String,
    pub tile: TileDto,
    pub is_used: bool,
}

impl RackTileView {
    pub fn _is_blank(&self) -> bool {
        matches!(self.tile, TileDto::Blank { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedPlacementView {
    pub board_index: usize,
    pub rack_tile_id: usize,
    pub display: String,
    pub tile: TileDto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovePreviewView {
    pub is_legal: bool,
    pub headline: String,
    pub detail: String,
    /// `None` for an illegal arrangement — a rejected placement doesn't
    /// have a meaningful score. Shown as its own badge (see `Home`)
    /// rather than left buried inside `headline`'s prose.
    pub score: Option<i16>,
}

#[component]
pub fn RootApp() -> Element {
    // Web: set (even to an empty string) at build time — empty means "same
    // origin as whatever page this was served from" (see `websocket_url`),
    // used by the container deployment where a reverse proxy serves both
    // the static build and the API from one host. Unset (the default for
    // local dev, where `dx serve`'s web client and the backend run as two
    // separate origins) falls back to the explicit dev address.
    #[cfg(target_arch = "wasm32")]
    let server_url = option_env!("TILE_LITE_ELITE_API_BASE_URL")
        .map(str::to_string)
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());

    // Desktop has no browser origin to derive from, so it resolves from
    // `crate::config` instead: a compiled-in default environment, with a
    // `--server-url`/`--env` CLI override (see `main.rs` and `config.rs`).
    #[cfg(not(target_arch = "wasm32"))]
    let server_url = crate::config::server_url();
    let mut game = use_signal(|| None::<GameStateDto>);
    let mut game_summaries = use_signal(Vec::<api::GameSummaryDto>::new);
    // Per-game "last seen chat message" watermark, for the unread-messages
    // indicator in the games list — purely local to this device/browser,
    // no server-side read-receipt concept. Loaded once at startup; kept in
    // sync with local storage by the reactive block below.
    let mut chat_watermarks: Signal<HashMap<String, String>> =
        use_signal(|| crate::local_storage::load_chat_watermarks().last_seen);
    let mut session = use_signal(|| None::<api::PlayerSessionDto>);
    let mut is_loading = use_signal(|| false);
    let mut info_message = use_signal(|| Some("Loading games from server...".to_string()));
    let mut error_message = use_signal(|| None::<String>);
    let mut bootstrapped = use_signal(|| false);
    let mut game_list_polling_started = use_signal(|| false);
    // Loaded lazily, keyed by `VariantRules.language` — not fetched until a
    // game using that language is actually open, and cached for the rest
    // of the session once loaded (see the reactive block below, mirroring
    // `websocket_game_id`'s pattern). On native builds resolution is
    // instant (every dictionary is compiled in), on wasm it's a real async
    // fetch (see `load_client_dictionary`), so the preview just shows
    // nothing for that brief window rather than blocking on it.
    let mut client_dictionaries: Signal<HashMap<String, &'static rules_shared::WordListDictionary>> =
        use_signal(HashMap::new);
    // Which languages a fetch has already been dispatched for — set
    // synchronously (not inside the spawned future) so a re-render while
    // the first fetch is still in flight doesn't dispatch a second,
    // redundant one for the same language.
    let mut dictionary_fetch_started: Signal<HashSet<String>> = use_signal(HashSet::new);
    let mut websocket_game_id = use_signal(|| None::<String>);
    let mut dragging_tile_id = use_signal(|| None::<usize>);
    // `Some(index)` while dragging a tile that was already staged on the
    // board (picked up from that index), rather than a fresh one off the
    // rack — lets the drop handler tell "move" from "place" apart, and
    // lets on_drag_end return the tile to the rack when it isn't dropped
    // on another valid board cell (including off the board entirely).
    let mut dragging_from_board_index = use_signal(|| None::<usize>);
    let mut staged_placements = use_signal(Vec::<StagedPlacementView>::new);
    let mut selected_blank_letter = use_signal(|| None::<String>);
    let mut selected_cell = use_signal(|| None::<usize>);
    // Only meaningful while exactly one tile is staged (direction is
    // otherwise ambiguous) — set by the space-bar/button toggle, cleared
    // whenever the staged tiles are cleared out so it doesn't linger and
    // silently steer a later, unrelated word.
    let mut direction_override = use_signal(|| None::<DirectionDto>);
    let mut exchange_mode = use_signal(|| false);
    let mut exchange_selected = use_signal(HashSet::<usize>::new);
    // Purely a display order for the rack — reset to identity whenever the
    // rack's tile count changes (a new turn's refill), shuffled in place by
    // the Shuffle button otherwise. `rack_tiles_for_seat` always returns
    // tiles in the same alphabetical order, so without this the rack would
    // never actually look shuffled.
    let mut rack_order = use_signal(Vec::<usize>::new);

    if !bootstrapped() {
        bootstrapped.set(true);
        let server_url = server_url.clone();
        spawn(async move {
            match check_api_version(&server_url).await {
                VersionCheck::Compatible | VersionCheck::Unreachable => {}
                VersionCheck::MinorMismatch { server, client } => {
                    info_message.set(Some(format!(
                        "Server API v{server} differs from this client's v{client} (non-breaking) — some features may be unavailable until you update."
                    )));
                }
                VersionCheck::MajorMismatch { server, client } => {
                    error_message.set(Some(format!(
                        "This client (API v{client}) is incompatible with the server (API v{server}). Please update the app before continuing."
                    )));
                    return;
                }
            }

            let stored = crate::local_storage::load();
            let mut auth_token: Option<String> = None;
            if let Some(token) = stored.session_token.clone() {
                match validate_session(&server_url, &token).await {
                    Ok(player) => {
                        session.set(Some(api::PlayerSessionDto {
                            player_id: player.id,
                            session_token: token.clone(),
                            display_name: player.display_name,
                            email: player.email,
                        }));
                        auth_token = Some(token);
                    }
                    Err(_) => {
                        // Stored token is no longer valid — drop it, but
                        // keep any remembered display name.
                        crate::local_storage::save(&crate::local_storage::StoredAuth {
                            remembered_name: stored.remembered_name.clone(),
                            session_token: None,
                        });
                    }
                }
            }

            is_loading.set(true);
            error_message.set(None);
            load_summaries_and_game(
                &server_url,
                auth_token.as_deref(),
                None,
                game,
                game_summaries,
                info_message,
                error_message,
                dragging_tile_id,
                selected_blank_letter,
                staged_placements,
                selected_cell,
                exchange_mode,
                exchange_selected,
                direction_override,
            )
            .await;
            is_loading.set(false);
        });
    }

    // Once we know we're offline (a request failed at the network level —
    // see `mark_offline`/`IS_ONLINE`), keep pinging `/health` in the
    // background until it answers, then reload whatever was on screen. A
    // plain WebSocket reconnect (below) isn't enough on its own: the server
    // doesn't replay missed events to a freshly (re)connected socket, so
    // anything that happened while we were disconnected — including the
    // very first load if the server was down at launch — needs an explicit
    // reload to catch up.
    let mut is_reconnecting = use_signal(|| false);
    {
        let server_url = server_url.clone();
        use_effect(move || {
            if IS_ONLINE() || is_reconnecting() {
                return;
            }
            is_reconnecting.set(true);
            let server_url = server_url.clone();
            let current_game_id = game().as_ref().map(|current| current.id.clone());
            let auth_token = session().map(|current| current.session_token.clone());
            spawn(async move {
                loop {
                    sleep_ms(RECONNECT_POLL_MS).await;
                    if check_server_reachable(&server_url).await {
                        break;
                    }
                }
                *IS_ONLINE.write() = true;
                info_message.set(Some("Reconnected — catching up...".to_string()));
                load_summaries_and_game(
                    &server_url,
                    auth_token.as_deref(),
                    current_game_id,
                    game,
                    game_summaries,
                    info_message,
                    error_message,
                    dragging_tile_id,
                    selected_blank_letter,
                    staged_placements,
                    selected_cell,
                    exchange_mode,
                    exchange_selected,
                    direction_override,
                )
                .await;
                is_reconnecting.set(false);
            });
        });
    }

    if let Some(current_game) = game() {
        if websocket_game_id().as_deref() != Some(current_game.id.as_str()) {
            let game_id = current_game.id.clone();
            websocket_game_id.set(Some(game_id.clone()));
            let server_url = server_url.clone();
            let token = session().map(|current| current.session_token.clone());
            spawn(async move {
                // Keep retrying for as long as this is still the selected
                // game — a dropped connection (network blip, server
                // restart) shouldn't leave live updates dead for the rest
                // of the session. `subscribe_to_game_events` itself marks
                // `IS_ONLINE` on connect/disconnect (see `mark_online` /
                // `mark_offline`); this loop just keeps trying.
                while websocket_game_id().as_deref() == Some(game_id.as_str()) {
                    let _ = subscribe_to_game_events(
                        &server_url,
                        &game_id,
                        token.as_deref(),
                        game,
                        websocket_game_id,
                    )
                    .await;
                    if websocket_game_id().as_deref() != Some(game_id.as_str()) {
                        break;
                    }
                    sleep_ms(WEBSOCKET_RETRY_MS).await;
                }
            });
        }
    }

    if !game_list_polling_started() {
        game_list_polling_started.set(true);
        let server_url = server_url.clone();
        spawn(async move {
            loop {
                sleep_ms(GAME_LIST_POLL_MS).await;
                let Some(token) = session().map(|current| current.session_token.clone()) else {
                    continue;
                };
                if let Ok(summaries) = load_game_summaries(&server_url, Some(&token)).await {
                    game_summaries.set(summaries);
                }
            }
        });
    }

    if let Some(current_game) = game() {
        let language = current_game.language.clone();
        if !dictionary_fetch_started().contains(&language) {
            dictionary_fetch_started.with_mut(|started| {
                started.insert(language.clone());
            });
            let server_url = server_url.clone();
            spawn(async move {
                if let Some(dictionary) = load_client_dictionary(&server_url, &language).await {
                    client_dictionaries.with_mut(|dictionaries| {
                        dictionaries.insert(language, dictionary);
                    });
                }
            });
        }
    }

    // Marks the currently-open game's chat as seen — fires both when a game
    // is first opened (loading its messages for the first time) and when a
    // live update arrives for a game that's already open, so watching the
    // panel counts as reading it. Never touches watermarks for any other
    // game, since `game()` only ever holds the currently-selected one.
    if let Some(current_game) = game() {
        if let Some(latest) = current_game.messages.last() {
            if chat_watermarks().get(&current_game.id) != Some(&latest.created_at) {
                let game_id = current_game.id.clone();
                let created_at = latest.created_at.clone();
                chat_watermarks.with_mut(|marks| {
                    marks.insert(game_id, created_at);
                });
                crate::local_storage::save_chat_watermarks(&crate::local_storage::StoredChatWatermarks {
                    last_seen: chat_watermarks(),
                });
            }
        }
    }

    let game_for_view = game().clone().unwrap_or_else(empty_live_game);
    let viewer_player_id = session().map(|current| current.player_id.clone());
    let can_start = IS_ONLINE()
        && game()
            .as_ref()
            .is_some_and(|current| current.status == GameStatus::Waiting);
    let can_submit_human_action = IS_ONLINE()
        && game().as_ref().is_some_and(|current| {
            current.status == GameStatus::Active
                && current
                    .participants
                    .get(current.current_seat as usize)
                    .is_some_and(|participant| {
                        participant.kind == SeatKind::Human && seat_is_open_or_owned_by(participant, viewer_player_id.as_deref())
                    })
        });
    // Which seat's rack this viewer gets to see at all: their own claimed
    // seat if they have one (even on another player's turn — you can
    // always see your own tiles while you wait), else the current seat if
    // it's unclaimed (anonymous/open play, unchanged from before), else
    // nothing — a logged-in viewer who isn't a participant, or an
    // anonymous one looking at a game with claimed seats, sees no rack at
    // all rather than relying on the server to reject an attempted move
    // after the fact.
    let viewer_rack_seat = viewer_rack_seat(&game_for_view, viewer_player_id.as_deref());
    let can_view_rack = viewer_rack_seat.is_some();
    let unordered_rack_tiles =
        rack_tiles_for_seat(&game_for_view, viewer_rack_seat, &staged_placements());
    if rack_order().len() != unordered_rack_tiles.len() {
        rack_order.set((0..unordered_rack_tiles.len()).collect());
    }
    let rack_tiles = apply_rack_order(&unordered_rack_tiles, &rack_order());
    let can_submit_manual_action =
        can_submit_human_action && !exchange_mode() && !staged_placements().is_empty();

    // Computed straight from current state, not a signal — this runs the
    // same `RulesEngine::validate_game_move` the server does, entirely
    // locally, so there's no network round-trip and (since nothing here is
    // async) no possibility of a stale response landing after the state
    // it was computed from has moved on. Needs the active game's dictionary
    // to have finished loading first (instant on native, a real fetch on
    // wasm) — until then this just shows nothing, same as "nothing staged
    // yet".
    let staged_preview = match (
        can_submit_human_action && !staged_placements().is_empty(),
        client_dictionaries().get(&game_for_view.language).copied(),
    ) {
        (true, Some(dictionary)) => {
            let direction = infer_typing_direction(
                &game_for_view,
                &staged_placements(),
                selected_cell(),
                direction_override(),
            );
            compute_client_preview(&game_for_view, &staged_placements(), direction, dictionary)
        }
        _ => None,
    };
    let server_url_for_login = server_url.clone();
    let server_url_for_custom_create = server_url.clone();
    let server_url_for_accept = server_url.clone();
    let server_url_for_reject = server_url.clone();
    let server_url_for_refresh = server_url.clone();
    let server_url_for_select = server_url.clone();
    let server_url_for_start = server_url.clone();
    let server_url_for_exchange = server_url.clone();
    let server_url_for_pass = server_url.clone();
    let server_url_for_chat = server_url.clone();
    let server_url_for_remove = server_url.clone();
    let server_url_for_reorder = server_url.clone();
    let server_url_for_resign = server_url.clone();
    let server_url_for_manual = server_url.clone();
    let game_for_home = game_for_view.clone();
    let game_for_drop = game_for_view.clone();
    let game_for_select = game_for_view.clone();
    let game_for_move = game_for_view.clone();
    let game_for_click = game_for_view.clone();
    let game_for_type = game_for_view.clone();
    let game_for_backspace = game_for_view.clone();
    let game_for_toggle = game_for_view.clone();
    let can_toggle_direction = staged_placements().len() == 1;
    let current_typing_direction = infer_typing_direction(
        &game_for_view,
        &staged_placements(),
        selected_cell(),
        direction_override(),
    );

    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }

        if let Some(token) = reset_password_token_from_url() {
            ResetPassword { server_url: server_url.clone(), token }
        } else {
        div { class: "app-shell",
            header { class: "topbar",
                p { class: "topbar-kicker", "Tile Lite Elite" }
                if !IS_ONLINE() {
                    span { class: "offline-indicator", "Can't reach the server — reconnecting..." }
                }
                AuthPanel {
                    server_url: server_url.clone(),
                    session: session().clone(),
                    on_authenticated: move |(new_session, remember, stay): (api::PlayerSessionDto, bool, bool)| {
                        let stored = crate::local_storage::StoredAuth {
                            remembered_name: if remember {
                                Some(new_session.display_name.clone())
                            } else {
                                None
                            },
                            session_token: if stay {
                                Some(new_session.session_token.clone())
                            } else {
                                None
                            },
                        };
                        crate::local_storage::save(&stored);
                        let token = new_session.session_token.clone();
                        session.set(Some(new_session));

                        let server_url = server_url_for_login.clone();
                        spawn(async move {
                            is_loading.set(true);
                            load_summaries_and_game(
                                &server_url,
                                Some(&token),
                                None,
                                game,
                                game_summaries,
                                info_message,
                                error_message,
                                dragging_tile_id,
                                selected_blank_letter,
                                staged_placements,
                                selected_cell,
                                exchange_mode,
                                exchange_selected,
                                direction_override,
                            )
                            .await;
                            is_loading.set(false);
                        });
                    },
                    on_logout: move |_| {
                        let stored = crate::local_storage::load();
                        crate::local_storage::save(&crate::local_storage::StoredAuth {
                            remembered_name: stored.remembered_name,
                            session_token: None,
                        });
                        session.set(None);
                        info_message.set(Some("Logged out".to_string()));
                    },
                    on_password_changed: move |_| {
                        // The server invalidates every session for this
                        // player on a password change, including the one
                        // that made the request — so the client just needs
                        // to drop its own local copy, same as a manual
                        // logout, and prompt a fresh login.
                        let stored = crate::local_storage::load();
                        crate::local_storage::save(&crate::local_storage::StoredAuth {
                            remembered_name: stored.remembered_name,
                            session_token: None,
                        });
                        session.set(None);
                        info_message.set(Some("Password changed — please log in again.".to_string()));
                    },
                }
            }

            div { class: "workspace-shell",
                GamesPanel {
                    summaries: game_summaries().clone(),
                    selected_id: game().as_ref().map(|current| current.id.clone()),
                    current_game: game().clone(),
                    viewer_player_id: viewer_player_id.clone(),
                    chat_watermarks: chat_watermarks(),
                    is_loading: is_loading(),
                    my_display_name: session().map(|current| current.display_name.clone()),
                    can_start,
                    on_send_chat: move |body: String| {
                        let server_url = server_url_for_chat.clone();
                        let current_game = game().clone();
                        let token = session().map(|current| current.session_token.clone());
                        if let Some(current_game) = current_game {
                            spawn(async move {
                                error_message.set(None);
                                match submit_chat_message(&server_url, &current_game, body, token.as_deref())
                                    .await
                                {
                                    Ok(updated) => game.set(Some(updated)),
                                    Err(error) => error_message.set(Some(error)),
                                }
                            });
                        }
                    },
                    on_start: move |_| {
                        let server_url = server_url_for_start.clone();
                        let current_game = game().clone();
                        let token = session().map(|current| current.session_token.clone());
                        if let Some(current_game) = current_game {
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match start_game(&server_url, &current_game.id, token.as_deref()).await {
                                    Ok(updated) => {
                                        info_message.set(None);
                                        reset_composer_state(
                                            dragging_tile_id,
                                            selected_blank_letter,
                                            staged_placements,
                                            selected_cell,
                                            exchange_mode,
                                            exchange_selected,
                                            direction_override,
                                        );
                                        game.set(Some(updated));
                                        if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                            game_summaries.set(summaries);
                                        }
                                    }
                                    Err(error) => error_message.set(Some(error)),
                                }
                                is_loading.set(false);
                            });
                        }
                    },
                    on_select: move |game_id: String| {
                        let server_url = server_url_for_select.clone();
                        let token = session().map(|current| current.session_token.clone());
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            match load_game_by_id(&server_url, &game_id, token.as_deref()).await {
                                Ok(loaded) => {
                                    info_message.set(None);
                                    reset_composer_state(
                                        dragging_tile_id,
                                        selected_blank_letter,
                                        staged_placements,
                                        selected_cell,
                                        exchange_mode,
                                        exchange_selected,
                                        direction_override,
                                    );
                                    websocket_game_id.set(None);
                                    game.set(Some(loaded));
                                }
                                Err(error) => error_message.set(Some(error)),
                            }
                            is_loading.set(false);
                        });
                    },
                    on_custom_new_game: move |submission: crate::components::games_panel::CustomGameSubmission| {
                        let server_url = server_url_for_custom_create.clone();
                        let token = session().map(|current| current.session_token.clone());
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            let start_immediately = submission.start_immediately;
                            match create_custom_game(&server_url, token.as_deref(), &submission).await {
                                Ok(created) => {
                                    reset_composer_state(
                                        dragging_tile_id,
                                        selected_blank_letter,
                                        staged_placements,
                                        selected_cell,
                                        exchange_mode,
                                        exchange_selected,
                                        direction_override,
                                    );
                                    websocket_game_id.set(None);
                                    // Select the game *before* calling `/start` (below),
                                    // rather than after — this makes the reactive
                                    // WebSocket-subscription effect connect right away.
                                    // A roster with no invitation left to wait on starts
                                    // immediately, and for an all-engine game `/start`
                                    // runs the entire game to completion inside that one
                                    // request; without an already-open connection, the
                                    // moves would never be visible, only the final
                                    // state once the request finally resolves. With one
                                    // open, `run_engine_turns` broadcasting after every
                                    // individual engine turn means they stream in live
                                    // while the request is still in flight.
                                    game.set(Some(created.clone()));
                                    // The games list only renders a detail panel (where
                                    // live moves would actually show up) for a game that
                                    // has a matching entry in `game_summaries` — without
                                    // this early refresh, the newly created game has no
                                    // row to render into at all until the final refresh
                                    // below, defeating the point of subscribing early.
                                    if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                        game_summaries.set(summaries);
                                    }
                                    // A roster with no invitation left to wait on (every
                                    // seat already resolved) starts right away — the
                                    // "Start" label on the draft button promised that,
                                    // rather than leaving the game in `Waiting` behind a
                                    // second, redundant per-game Start click.
                                    let started = if start_immediately {
                                        start_game(&server_url, &created.id, token.as_deref()).await
                                    } else {
                                        Ok(created)
                                    };
                                    match started {
                                        Ok(game_state) => {
                                            info_message.set(None);
                                            game.set(Some(game_state));
                                            if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                                game_summaries.set(summaries);
                                            }
                                        }
                                        Err(error) => error_message.set(Some(error)),
                                    }
                                }
                                Err(error) => error_message.set(Some(error)),
                            }
                            is_loading.set(false);
                        });
                    },
                    on_accept_invitation: move |invitation_id: String| {
                        let server_url = server_url_for_accept.clone();
                        let token = session().map(|current| current.session_token.clone());
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            match accept_invitation(&server_url, &invitation_id, token.as_deref()).await {
                                Ok(joined) => {
                                    info_message.set(None);
                                    websocket_game_id.set(None);
                                    game.set(Some(joined));
                                    if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                        game_summaries.set(summaries);
                                    }
                                }
                                Err(error) => error_message.set(Some(error)),
                            }
                            is_loading.set(false);
                        });
                    },
                    on_reject_invitation: move |invitation_id: String| {
                        let server_url = server_url_for_reject.clone();
                        let token = session().map(|current| current.session_token.clone());
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            match reject_invitation(&server_url, &invitation_id, token.as_deref()).await {
                                Ok(_) => {
                                    if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                        game_summaries.set(summaries);
                                    }
                                }
                                Err(error) => error_message.set(Some(error)),
                            }
                            is_loading.set(false);
                        });
                    },
                    on_remove_game: move |game_id: String| {
                        let server_url = server_url_for_remove.clone();
                        let token = session().map(|current| current.session_token.clone());
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            match remove_game(&server_url, &game_id, token.as_deref()).await {
                                Ok(_) => {
                                    // The removed game's row is about to
                                    // disappear from the list — if it was
                                    // the one currently open, deselect it
                                    // rather than leaving a stale detail
                                    // panel open for a game no longer in
                                    // view.
                                    if game().as_ref().is_some_and(|current| current.id == game_id) {
                                        game.set(None);
                                    }
                                    if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                        game_summaries.set(summaries);
                                    }
                                }
                                Err(error) => error_message.set(Some(error)),
                            }
                            is_loading.set(false);
                        });
                    },
                    on_reorder_seats: move |(seat_a, seat_b): (u8, u8)| {
                        let server_url = server_url_for_reorder.clone();
                        let current_game = game().clone();
                        let token = session().map(|current| current.session_token.clone());
                        if let Some(current_game) = current_game {
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match swap_seats(&server_url, &current_game.id, seat_a, seat_b, token.as_deref())
                                    .await
                                {
                                    Ok(updated) => game.set(Some(updated)),
                                    Err(error) => error_message.set(Some(error)),
                                }
                                is_loading.set(false);
                            });
                        }
                    },
                    on_refresh: move |_| {
                        let server_url = server_url_for_refresh.clone();
                        let token = session().map(|current| current.session_token.clone());
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            match load_game_summaries(&server_url, token.as_deref()).await {
                                Ok(summaries) => game_summaries.set(summaries),
                                Err(error) => error_message.set(Some(error)),
                            }
                            is_loading.set(false);
                        });
                    },
                }

                Home {
                    game: game_for_home,
                    is_live: game().is_some(),
                    is_loading: is_loading(),
                    info_message: info_message().clone(),
                    error_message: error_message().clone(),
                    rack_tiles,
                    on_shuffle_rack: move |_| {
                        rack_order.with_mut(|order| shuffle_order(order));
                    },
                    can_view_rack,
                    staged_placements: staged_placements().clone(),
                    can_stage_moves: can_submit_human_action && !exchange_mode(),
                    selected_cell: selected_cell(),
                    can_toggle_direction,
                    current_typing_direction,
                    on_toggle_direction: move |_| {
                        toggle_direction_override(
                            &game_for_toggle,
                            staged_placements,
                            direction_override,
                            selected_cell,
                        );
                    },
                    on_drag_rack_tile: move |tile_id| {
                        dragging_tile_id.set(Some(tile_id));
                        dragging_from_board_index.set(None);
                    },
                    on_drag_end_rack_tile: move |_| {
                        dragging_tile_id.set(None);
                    },
                    on_drop_rack_tile: move |target_id: usize| {
                        if dragging_from_board_index().is_some() {
                            // A staged board tile dropped back onto the
                            // rack — leave dragging_from_board_index alone
                            // so on_drag_end_staged_tile's fallback still
                            // unstages it; nothing to reorder here.
                            return;
                        }
                        let Some(dragged_id) = dragging_tile_id() else {
                            return;
                        };
                        rack_order.with_mut(|order| {
                            *order = reorder_rack_order(order, dragged_id, target_id);
                        });
                        dragging_tile_id.set(None);
                    },
                    on_drag_staged_tile: move |board_index: usize| {
                        let tile_id = staged_placements()
                            .iter()
                            .find(|p| p.board_index == board_index)
                            .map(|p| p.rack_tile_id);
                        let Some(tile_id) = tile_id else { return };
                        dragging_tile_id.set(Some(tile_id));
                        dragging_from_board_index.set(Some(board_index));
                    },
                    on_drag_end_staged_tile: move |board_index: usize| {
                        // Fires whether or not the drop landed anywhere —
                        // if this same origin is still recorded, nothing
                        // claimed it (dropped off the board, or on an
                        // invalid cell), so it goes back to the rack. A
                        // successful move to another cell already clears
                        // this before drag-end fires.
                        if dragging_from_board_index() == Some(board_index) {
                            staged_placements
                                .with_mut(|placements| placements.retain(|p| p.board_index != board_index));
                            dragging_tile_id.set(None);
                            dragging_from_board_index.set(None);
                            // Same reasoning as `on_remove_staged` — the
                            // freed cell is the natural place to keep
                            // composing from.
                            selected_cell.set(Some(board_index));
                        }
                    },
                    on_drop_board_cell: move |board_index| {
                        if !can_submit_human_action || exchange_mode() {
                            return;
                        }
                        let target_is_taken = game_for_drop
                            .board
                            .get(board_index)
                            .is_some_and(|cell: &BoardCellDto| cell.letter.is_some())
                            || staged_placements()
                                .iter()
                                .any(|p| p.board_index == board_index);
                        if target_is_taken {
                            // Dropping on an occupied/staged cell just
                            // fails — if this drag picked up an existing
                            // placement, clear the "in flight" marker so
                            // on_drag_end_staged_tile (which fires next,
                            // regardless of drop outcome) sees it's already
                            // been dealt with and leaves the tile exactly
                            // where it was, rather than reading a failed
                            // drop here the same as a genuine drop off the
                            // board entirely.
                            dragging_tile_id.set(None);
                            dragging_from_board_index.set(None);
                            return;
                        }
                        let Some(tile_id) = dragging_tile_id() else {
                            return;
                        };
                        // Any change to the staged placements invalidates a
                        // previous submit/typing message — the live preview
                        // banner is the one source of truth for the current
                        // arrangement going forward.
                        error_message.set(None);
                        info_message.set(None);
                        if let Some(old_index) = dragging_from_board_index() {
                            // Moving an already-staged tile: carry over its
                            // existing display/tile (a resolved blank keeps
                            // its chosen letter) rather than re-deriving a
                            // fresh, unresolved one from the rack.
                            let existing = staged_placements()
                                .iter()
                                .find(|p| p.board_index == old_index && p.rack_tile_id == tile_id)
                                .cloned();
                            if let Some(existing) = existing {
                                staged_placements.with_mut(|placements| {
                                    placements.retain(|p| p.board_index != old_index);
                                    placements.push(StagedPlacementView {
                                        board_index,
                                        ..existing
                                    });
                                });
                            }
                            dragging_tile_id.set(None);
                            dragging_from_board_index.set(None);
                            return;
                        }
                        let Some(tile) = current_rack_tiles(&game_for_drop, &staged_placements())
                            .into_iter()
                            .find(|t| t.id == tile_id)
                            else {
                            dragging_tile_id.set(None);
                            return;
                        };
                        let placement = stage_tile_at_cell(board_index, &tile, None);
                        staged_placements
                            .with_mut(|placements| placements.push(placement));
                        dragging_tile_id.set(None);
                        advance_selection(
                            &game_for_drop,
                            staged_placements,
                            selected_cell,
                            direction_override(),
                            board_index,
                        );
                    },
                    on_select_cell: move |board_index: usize| {
                        if !can_submit_human_action || exchange_mode() {
                            return;
                        }
                        if game_for_select
                            .board
                            .get(board_index)
                            .is_some_and(|cell: &BoardCellDto| cell.letter.is_some())
                        {
                            return;
                        }
                        selected_cell.set(Some(board_index));
                    },
                    // Arrow-key navigation: `direction`/`forward` are a raw
                    // 2D step (Left/Right = Horizontal, Up/Down = Vertical),
                    // not this turn's inferred typing direction. Uses the
                    // wrapping variant, not `find_next_placeable_cell` —
                    // moving the selection by hand should cycle around the
                    // edge of the board rather than get stuck there (that
                    // "stop at the edge" behavior is deliberately kept for
                    // advancing through a word as it's typed/placed — see
                    // `advance_selection`). Still skips over any occupied
                    // square — a permanently-played letter or a tile staged
                    // earlier this turn — landing on the next free cell if
                    // there's room anywhere in the row/column, or leaving
                    // the selection where it is if the whole line is full.
                    on_move_selection: move |(direction, forward): (DirectionDto, bool)| {
                        if !can_submit_human_action || exchange_mode() {
                            return;
                        }
                        let Some(current) = selected_cell() else {
                            return;
                        };
                        if let Some(next) = find_next_placeable_cell_wrapping(
                            &game_for_move,
                            &staged_placements(),
                            current,
                            direction,
                            forward,
                        ) {
                            selected_cell.set(Some(next));
                        }
                    },
                    on_click_rack_tile: move |tile_id: usize| {
                        if !can_submit_human_action || exchange_mode() {
                            return;
                        }
                        let Some(cell_index) = selected_cell() else {
                            return;
                        };
                        if game_for_click
                            .board
                            .get(cell_index)
                            .is_some_and(|cell: &BoardCellDto| cell.letter.is_some())
                        {
                            return;
                        }
                        if staged_placements()
                            .iter()
                            .any(|p| p.board_index == cell_index)
                        {
                            return;
                        }
                        let Some(tile) = current_rack_tiles(&game_for_click, &staged_placements())
                            .into_iter()
                            .find(|t| t.id == tile_id)
                        else {
                            return;
                        };
                        error_message.set(None);
                        info_message.set(None);
                        let placement = stage_tile_at_cell(cell_index, &tile, None);
                        staged_placements
                            .with_mut(|placements| placements.push(placement));
                        advance_selection(
                            &game_for_click,
                            staged_placements,
                            selected_cell,
                            direction_override(),
                            cell_index,
                        );
                    },
                    on_type_letter: move |letter: char| {
                        if !can_submit_human_action || exchange_mode() {
                            return;
                        }
                        let Some(cell_index) = selected_cell() else {
                            return;
                        };
                        if game_for_type
                            .board
                            .get(cell_index)
                            .is_some_and(|cell: &BoardCellDto| cell.letter.is_some())
                        {
                            return;
                        }
                        if staged_placements()
                            .iter()
                            .any(|p| p.board_index == cell_index)
                        {
                            return;
                        }
                        let rack = current_rack_tiles(&game_for_type, &staged_placements());
                        // Prefer an exact unused letter tile; fall back to an
                        // unused blank, auto-resolved to the typed letter
                        // (skips the manual blank-letter picker, since the
                        // player already told us the letter by typing it).
                        let typed = letter.to_string();
                        let chosen = rack
                            .iter()
                            .find(|t| {
                                !t.is_used
                                    && matches!(&t.tile, TileDto::Letter { letter: l } if *l == typed)
                            })
                            .or_else(|| {
                                rack.iter()
                                    .find(|t| !t.is_used && matches!(t.tile, TileDto::Blank { .. }))
                            });
                        let Some(tile) = chosen else {
                            return;
                        };
                        let resolved =
                            matches!(tile.tile, TileDto::Blank { .. }).then(|| typed.clone());
                        let placement = stage_tile_at_cell(cell_index, tile, resolved);
                        error_message.set(None);
                        info_message.set(None);
                        staged_placements
                            .with_mut(|placements| placements.push(placement));
                        advance_selection(
                            &game_for_type,
                            staged_placements,
                            selected_cell,
                            direction_override(),
                            cell_index,
                        );
                    },
                    on_backspace: move |_| {
                        if !can_submit_human_action || exchange_mode() {
                            return;
                        }
                        let Some(cell_index) = selected_cell() else {
                            return;
                        };
                        // The cursor normally sits one past the last typed
                        // tile (ready for the next letter), so backspace
                        // targets the previous editable cell — the tile just
                        // behind the cursor — and removes/lands on exactly
                        // that one cell, rather than also skipping past it.
                        // If the cursor is already sitting directly on a
                        // staged tile (e.g. after clicking it), act on that
                        // cell in place instead of stepping back further.
                        let cursor_has_tile = staged_placements()
                            .iter()
                            .any(|p| p.board_index == cell_index);
                        let target = if cursor_has_tile {
                            Some(cell_index)
                        } else {
                            let direction = infer_typing_direction(
                                &game_for_backspace,
                                &staged_placements(),
                                Some(cell_index),
                                direction_override(),
                            );
                            find_previous_editable_cell(&game_for_backspace, cell_index, direction)
                        };
                        let Some(target) = target else {
                            return;
                        };
                        error_message.set(None);
                        info_message.set(None);
                        staged_placements
                            .with_mut(|placements| placements.retain(|p| p.board_index != target));
                        selected_cell.set(Some(target));
                    },
                    on_delete: move |_| {
                        if !can_submit_human_action || exchange_mode() {
                            return;
                        }
                        let Some(cell_index) = selected_cell() else {
                            return;
                        };
                        error_message.set(None);
                        info_message.set(None);
                        // Forward-delete: removes a staged tile at the
                        // cursor without moving the cursor, unlike backspace.
                        staged_placements
                            .with_mut(|placements| placements.retain(|p| p.board_index != cell_index));
                    },
                    on_clear_staged: move |_| {
                        dragging_tile_id.set(None);
                        selected_blank_letter.set(None);
                        staged_placements.set(Vec::new());
                        selected_cell.set(None);
                        direction_override.set(None);
                        error_message.set(None);
                        info_message.set(None);
                    },
                    on_remove_staged: move |board_index| {
                        error_message.set(None);
                        info_message.set(None);
                        staged_placements
                            .with_mut(|placements| {
                                placements.retain(|p| p.board_index != board_index);
                            });
                        // The cell that just gave up its tile is the most
                        // natural place to keep composing from — lets
                        // right-click-to-remove immediately be followed by
                        // typing a replacement letter, rather than leaving
                        // nothing selected.
                        selected_cell.set(Some(board_index));
                    },
                    on_set_blank_letter: move |letter: String| {
                        error_message.set(None);
                        info_message.set(None);
                        selected_blank_letter.set(Some(letter.clone()));
                        staged_placements
                            .with_mut(|placements| {
                                if let Some(placement) = placements
                                    .iter_mut()
                                    .find(|p| matches!(p.tile, TileDto::Blank { acting_as: None }))
                                {
                                    placement.display = letter.to_lowercase();
                                    placement.tile = TileDto::Blank {
                                        acting_as: Some(letter),
                                    };
                                }
                            });
                    },
                    selected_blank_letter: selected_blank_letter(),
                    staged_preview,
                    is_your_turn: can_submit_human_action,
                    can_pass: can_submit_human_action && !exchange_mode(),
                    on_pass: move |_| {
                        let server_url = server_url_for_pass.clone();
                        let current_game = game().clone();
                        let token = session().map(|current| current.session_token.clone());
                        if let Some(current_game) = current_game {
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match submit_pass(&server_url, &current_game, token.as_deref()).await {
                                    Ok(updated) => {
                                        info_message.set(None);
                                        reset_composer_state(
                                            dragging_tile_id,
                                            selected_blank_letter,
                                            staged_placements,
                                            selected_cell,
                                            exchange_mode,
                                            exchange_selected,
                                            direction_override,
                                        );
                                        game.set(Some(updated));
                                        if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                            game_summaries.set(summaries);
                                        }
                                    }
                                    Err(error) => error_message.set(Some(error)),
                                }
                                is_loading.set(false);
                            });
                        }
                    },
                    can_resign: can_submit_human_action && !exchange_mode(),
                    on_resign: move |_| {
                        let server_url = server_url_for_resign.clone();
                        let current_game = game().clone();
                        let token = session().map(|current| current.session_token.clone());
                        if let Some(current_game) = current_game {
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match submit_resign(&server_url, &current_game, token.as_deref()).await {
                                    Ok(updated) => {
                                        info_message.set(None);
                                        reset_composer_state(
                                            dragging_tile_id,
                                            selected_blank_letter,
                                            staged_placements,
                                            selected_cell,
                                            exchange_mode,
                                            exchange_selected,
                                            direction_override,
                                        );
                                        game.set(Some(updated));
                                        if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                            game_summaries.set(summaries);
                                        }
                                    }
                                    Err(error) => error_message.set(Some(error)),
                                }
                                is_loading.set(false);
                            });
                        }
                    },
                    can_submit_manual: can_submit_manual_action,
                    on_submit_manual: move |_| {
                        let server_url = server_url_for_manual.clone();
                        let current_game = game().clone();
                        let staged = staged_placements().clone();
                        let token = session().map(|current| current.session_token.clone());
                        if let Some(current_game) = current_game {
                            let direction = infer_typing_direction(
                                &current_game,
                                &staged,
                                selected_cell(),
                                direction_override(),
                            );
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match submit_manual_move(
                                        &server_url,
                                        &current_game,
                                        &staged,
                                        direction,
                                        token.as_deref(),
                                    )
                                    .await
                                {
                                    Ok(updated) => {
                                        info_message.set(None);
                                        reset_composer_state(
                                            dragging_tile_id,
                                            selected_blank_letter,
                                            staged_placements,
                                            selected_cell,
                                            exchange_mode,
                                            exchange_selected,
                                            direction_override,
                                        );
                                        game.set(Some(updated));
                                        if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                            game_summaries.set(summaries);
                                        }
                                    }
                                    Err(error) => error_message.set(Some(error)),
                                }
                                is_loading.set(false);
                            });
                        }
                    },
                    exchange_mode: exchange_mode(),
                    exchange_selected: exchange_selected().clone(),
                    can_toggle_exchange: can_submit_human_action,
                    on_toggle_exchange_mode: move |_| {
                        if !can_submit_human_action {
                            return;
                        }
                        let turning_on = !exchange_mode();
                        if turning_on {
                            // Placing and exchanging are mutually exclusive
                            // for a turn; drop any in-progress placement so
                            // the two states can never mix.
                            reset_composer_state(
                                dragging_tile_id,
                                selected_blank_letter,
                                staged_placements,
                                selected_cell,
                                exchange_mode,
                                exchange_selected,
                                direction_override,
                            );
                            exchange_mode.set(true);
                        } else {
                            exchange_mode.set(false);
                            exchange_selected.set(HashSet::new());
                        }
                    },
                    on_toggle_exchange_tile: move |tile_id: usize| {
                        exchange_selected
                            .with_mut(|selected| {
                                if !selected.remove(&tile_id) {
                                    selected.insert(tile_id);
                                }
                            });
                    },
                    can_confirm_exchange: IS_ONLINE() && exchange_mode() && !exchange_selected().is_empty(),
                    on_confirm_exchange: move |_| {
                        let server_url = server_url_for_exchange.clone();
                        let current_game = game().clone();
                        let token = session().map(|current| current.session_token.clone());
                        let selected_ids = exchange_selected().clone();
                        if let Some(current_game) = current_game {
                            let tiles: Vec<TileDto> = current_rack_tiles(&current_game, &Vec::new())
                                .into_iter()
                                .filter(|tile| selected_ids.contains(&tile.id))
                                .map(|tile| tile.tile)
                                .collect();
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match submit_exchange(&server_url, &current_game, tiles, token.as_deref()).await {
                                    Ok(updated) => {
                                        info_message.set(None);
                                        reset_composer_state(
                                            dragging_tile_id,
                                            selected_blank_letter,
                                            staged_placements,
                                            selected_cell,
                                            exchange_mode,
                                            exchange_selected,
                                            direction_override,
                                        );
                                        game.set(Some(updated));
                                        if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                            game_summaries.set(summaries);
                                        }
                                    }
                                    Err(error) => error_message.set(Some(error)),
                                }
                                is_loading.set(false);
                            });
                        }
                    },
                    on_cancel_exchange: move |_| {
                        exchange_mode.set(false);
                        exchange_selected.set(HashSet::new());
                    },
                }
            }
        }
        }
    }
}
fn empty_live_game() -> GameStateDto {
    GameStateDto {
        id: "not-connected".to_string(),
        status: GameStatus::Waiting,
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
        participants: vec![ParticipantDto {
            seat_number: 0,
            kind: SeatKind::Human,
            display_name: "Open a game".to_string(),
            player_id: None,
            engine_id: None,
            score: 0,
        }],
        board: empty_board(),
        racks: vec![RackDto {
            counts: Vec::new(),
            blanks: 0,
        }],
        moves: vec![],
        messages: vec![],
    }
}

fn empty_board() -> Vec<BoardCellDto> {
    let mut board = vec![
        BoardCellDto {
            premium: PremiumDto::Blank,
            letter: None,
            is_blank: false,
        };
        15 * 15
    ];

    for (x, y, premium) in [
        (0, 0, PremiumDto::TripleWord),
        (3, 0, PremiumDto::DoubleLetter),
        (7, 0, PremiumDto::TripleWord),
        (1, 1, PremiumDto::DoubleWord),
        (5, 1, PremiumDto::TripleLetter),
        (2, 2, PremiumDto::DoubleWord),
        (6, 2, PremiumDto::DoubleLetter),
        (0, 3, PremiumDto::DoubleLetter),
        (3, 3, PremiumDto::DoubleWord),
        (7, 3, PremiumDto::DoubleLetter),
        (4, 4, PremiumDto::DoubleWord),
        (1, 5, PremiumDto::TripleLetter),
        (5, 5, PremiumDto::TripleLetter),
        (2, 6, PremiumDto::DoubleLetter),
        (6, 6, PremiumDto::DoubleLetter),
        (0, 7, PremiumDto::TripleWord),
        (3, 7, PremiumDto::DoubleLetter),
        (7, 7, PremiumDto::DoubleWord),
    ] {
        for (mx, my) in mirrored_positions(x, y) {
            board[(my as usize) * 15 + (mx as usize)].premium = premium.clone();
        }
    }

    board
}

fn mirrored_positions(x: u8, y: u8) -> [(u8, u8); 4] {
    let max = 14;
    [(x, y), (max - x, y), (x, max - y), (max - x, max - y)]
}

async fn load_game_summaries(
    server_url: &str,
    token: Option<&str>,
) -> Result<Vec<api::GameSummaryDto>, String> {
    get_json_auth::<Vec<api::GameSummaryDto>>(&format!("{server_url}/games"), token).await
}

pub(crate) async fn register_player(
    server_url: &str,
    display_name: &str,
    email: &str,
    password: &str,
) -> Result<api::PlayerSessionDto, String> {
    let request = api::RegisterPlayerRequest {
        display_name: display_name.to_string(),
        email: email.to_string(),
        password: password.to_string(),
    };
    post_json(&format!("{server_url}/auth/register"), None, &request).await
}

pub(crate) async fn login_player(
    server_url: &str,
    display_name: &str,
    password: &str,
) -> Result<api::PlayerSessionDto, String> {
    let request = api::LoginPlayerRequest {
        display_name: display_name.to_string(),
        password: password.to_string(),
    };
    post_json(&format!("{server_url}/auth/login"), None, &request).await
}

pub(crate) async fn change_password(
    server_url: &str,
    token: &str,
    current_password: &str,
    new_password: &str,
) -> Result<(), String> {
    let request = api::ChangePasswordRequest {
        current_password: current_password.to_string(),
        new_password: new_password.to_string(),
    };
    post_no_content(
        &format!("{server_url}/auth/change-password"),
        Some(token),
        &request,
    )
    .await
}

pub(crate) async fn request_password_reset(server_url: &str, email: &str) -> Result<(), String> {
    let request = api::RequestPasswordResetRequest {
        email: email.to_string(),
    };
    post_no_content(
        &format!("{server_url}/auth/forgot-password"),
        None,
        &request,
    )
    .await
}

pub(crate) async fn reset_password(
    server_url: &str,
    token: &str,
    new_password: &str,
) -> Result<(), String> {
    let request = api::ResetPasswordRequest {
        token: token.to_string(),
        new_password: new_password.to_string(),
    };
    post_no_content(
        &format!("{server_url}/auth/reset-password"),
        None,
        &request,
    )
    .await
}

async fn validate_session(server_url: &str, session_token: &str) -> Result<api::PlayerDto, String> {
    let request = api::ValidateSessionRequest {
        session_token: session_token.to_string(),
    };
    post_json(&format!("{server_url}/auth/validate"), None, &request).await
}

async fn load_game_by_id(
    server_url: &str,
    game_id: &str,
    token: Option<&str>,
) -> Result<GameStateDto, String> {
    get_json_auth::<GameStateDto>(&format!("{server_url}/games/{game_id}"), token).await
}

/// A bare reachability probe — unlike `get_json`, doesn't care about the
/// response body (the `/health` endpoint returns plain text, not JSON),
/// only whether a response came back at all. Used by the reconnect loop to
/// poll without spamming `mark_offline`/`mark_online` state churn on every
/// attempt.
#[cfg(not(target_arch = "wasm32"))]
async fn check_server_reachable(server_url: &str) -> bool {
    reqwest::Client::new()
        .get(format!("{server_url}/health"))
        .send()
        .await
        .is_ok()
}

#[cfg(target_arch = "wasm32")]
async fn check_server_reachable(server_url: &str) -> bool {
    Request::get(&format!("{server_url}/health"))
        .send()
        .await
        .is_ok()
}

/// Result of comparing this client's compiled-in `api::API_VERSION`
/// against what the server reported at `/health` on first connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionCheck {
    Compatible,
    /// Non-breaking drift — old client still works, just without whatever
    /// the newer/older side added. Worth a soft, non-blocking notice.
    MinorMismatch {
        server: api::ApiVersion,
        client: api::ApiVersion,
    },
    /// Breaking drift — this client build can't be trusted to talk to this
    /// server correctly. Should block further use, not just warn.
    MajorMismatch {
        server: api::ApiVersion,
        client: api::ApiVersion,
    },
    /// `/health` didn't answer at all — not a version problem, the normal
    /// offline/reachability handling elsewhere in bootstrap covers it.
    Unreachable,
}

fn compare_api_version(server: api::ApiVersion, client: api::ApiVersion) -> VersionCheck {
    if server.major != client.major {
        VersionCheck::MajorMismatch { server, client }
    } else if server.minor != client.minor {
        VersionCheck::MinorMismatch { server, client }
    } else {
        VersionCheck::Compatible
    }
}

/// Checked once, at the start of bootstrap, before anything else talks to
/// the server — see the call site in `RootApp`.
async fn check_api_version(server_url: &str) -> VersionCheck {
    match get_json::<api::HealthDto>(&format!("{server_url}/health")).await {
        Ok(health) => compare_api_version(health.api_version, api::API_VERSION),
        Err(_) => VersionCheck::Unreachable,
    }
}

/// Loads the games list and a target game (a specific id if given, else the
/// most recent one), replacing whatever's currently shown. Shared by the
/// initial bootstrap and the reconnect-recovery loop so both end up in the
/// same state after a successful load.
#[allow(clippy::too_many_arguments)]
async fn load_summaries_and_game(
    server_url: &str,
    token: Option<&str>,
    preferred_game_id: Option<String>,
    mut game: Signal<Option<GameStateDto>>,
    mut game_summaries: Signal<Vec<api::GameSummaryDto>>,
    mut info_message: Signal<Option<String>>,
    mut error_message: Signal<Option<String>>,
    dragging_tile_id: Signal<Option<usize>>,
    selected_blank_letter: Signal<Option<String>>,
    staged_placements: Signal<Vec<StagedPlacementView>>,
    selected_cell: Signal<Option<usize>>,
    exchange_mode: Signal<bool>,
    exchange_selected: Signal<HashSet<usize>>,
    direction_override: Signal<Option<DirectionDto>>,
) {
    // The games list is per-account now (the server 401s without a
    // session), so there's nothing meaningful to fetch until the caller is
    // signed in — surface that as guidance rather than an "error".
    let Some(token) = token else {
        game_summaries.set(Vec::new());
        info_message.set(Some("Sign in to see your games.".to_string()));
        return;
    };

    match load_game_summaries(server_url, Some(token)).await {
        Ok(summaries) => {
            let target_id = preferred_game_id.or_else(|| summaries.first().map(|s| s.id.clone()));
            game_summaries.set(summaries);
            match target_id {
                Some(game_id) => match load_game_by_id(server_url, &game_id, Some(token)).await {
                    Ok(loaded) => {
                        info_message.set(None);
                        reset_composer_state(
                            dragging_tile_id,
                            selected_blank_letter,
                            staged_placements,
                            selected_cell,
                            exchange_mode,
                            exchange_selected,
                            direction_override,
                        );
                        game.set(Some(loaded));
                    }
                    Err(error) => error_message.set(Some(error)),
                },
                None => {
                    info_message.set(Some("No games yet. Create one to begin.".to_string()));
                }
            }
        }
        Err(error) => error_message.set(Some(error)),
    }
}


async fn create_custom_game(
    server_url: &str,
    token: Option<&str>,
    submission: &crate::components::games_panel::CustomGameSubmission,
) -> Result<GameStateDto, String> {
    let request = CreateGameRequest {
        seats: submission.seats.clone(),
        seed: None,
        variant: submission.variant.clone(),
        language: None,
        board_layout: None,
        move_time_limit_seconds: submission.move_time_limit_seconds,
    };
    post_json(&format!("{server_url}/games"), token, &request).await
}

/// Neither endpoint takes a request body (the invitation id in the path is
/// all the server needs), but `post_json` always serializes a payload — `()`
/// serializes to `null`, which the handlers simply never look at.
async fn accept_invitation(server_url: &str, invitation_id: &str, token: Option<&str>) -> Result<GameStateDto, String> {
    post_json(&format!("{server_url}/invitations/{invitation_id}/accept"), token, &()).await
}

async fn reject_invitation(server_url: &str, invitation_id: &str, token: Option<&str>) -> Result<serde_json::Value, String> {
    post_json(&format!("{server_url}/invitations/{invitation_id}/reject"), token, &()).await
}

/// Hides a finished game from the caller's own games list — see
/// `crate::components::games_panel::game_row`'s "Remove" button.
async fn remove_game(server_url: &str, game_id: &str, token: Option<&str>) -> Result<serde_json::Value, String> {
    post_json(&format!("{server_url}/games/{game_id}/remove"), token, &()).await
}

async fn start_game(server_url: &str, game_id: &str, token: Option<&str>) -> Result<GameStateDto, String> {
    post_json(
        &format!("{server_url}/games/{game_id}/start"),
        token,
        &StartGameRequest::default(),
    )
    .await
}

async fn swap_seats(
    server_url: &str,
    game_id: &str,
    seat_a: u8,
    seat_b: u8,
    token: Option<&str>,
) -> Result<GameStateDto, String> {
    post_json(
        &format!("{server_url}/games/{game_id}/reorder-seats"),
        token,
        &api::SwapSeatsRequest { seat_a, seat_b },
    )
    .await
}

async fn submit_pass(
    server_url: &str,
    game: &GameStateDto,
    token: Option<&str>,
) -> Result<GameStateDto, String> {
    let request = GameActionRequest {
        seat_number: game.current_seat,
        action: api::PlayerActionDto::Pass,
    };
    post_json(&format!("{server_url}/games/{}/actions", game.id), token, &request).await
}

/// Not routed through `submit_pass`/`submit_resign`'s `GameActionRequest`
/// shape — chat has its own endpoint, not gated by turn ownership (see the
/// matching note on the server's `post_chat_message` handler).
async fn submit_chat_message(
    server_url: &str,
    game: &GameStateDto,
    body: String,
    token: Option<&str>,
) -> Result<GameStateDto, String> {
    post_json(
        &format!("{server_url}/games/{}/chat", game.id),
        token,
        &api::PostChatMessageRequest { body },
    )
    .await
}

async fn submit_resign(
    server_url: &str,
    game: &GameStateDto,
    token: Option<&str>,
) -> Result<GameStateDto, String> {
    let request = GameActionRequest {
        seat_number: game.current_seat,
        action: api::PlayerActionDto::Resign,
    };
    post_json(&format!("{server_url}/games/{}/actions", game.id), token, &request).await
}

async fn submit_exchange(
    server_url: &str,
    game: &GameStateDto,
    tiles: Vec<TileDto>,
    token: Option<&str>,
) -> Result<GameStateDto, String> {
    let request = GameActionRequest {
        seat_number: game.current_seat,
        action: api::PlayerActionDto::Exchange { tiles },
    };
    post_json(&format!("{server_url}/games/{}/actions", game.id), token, &request).await
}

async fn submit_manual_move(
    server_url: &str,
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    direction: DirectionDto,
    token: Option<&str>,
) -> Result<GameStateDto, String> {
    let request = build_manual_move_request(game, staged, direction)?;
    post_json(&format!("{server_url}/games/{}/actions", game.id), token, &request).await
}

async fn post_json<T, R>(url: &str, token: Option<&str>, payload: &T) -> Result<R, String>
where
    T: serde::Serialize + ?Sized,
    R: serde::de::DeserializeOwned,
{
    post_json_impl(url, token, payload).await
}

/// For endpoints that respond `204 No Content` (nothing to deserialize) —
/// `post_json::<_, ()>` would fail trying to parse an empty body as JSON.
async fn post_no_content<T>(url: &str, token: Option<&str>, payload: &T) -> Result<(), String>
where
    T: serde::Serialize + ?Sized,
{
    post_no_content_impl(url, token, payload).await
}

/// Text shown for a request that never got a response at all — as opposed
/// to a response the server sent back rejecting it, which keeps its own
/// specific message. This is the one signal that distinguishes "the server
/// is down/unreachable" from "you made an illegal move."
const UNREACHABLE_MESSAGE: &str = "Can't reach the server.";

/// Marks the backend unreachable. Called only at the point where a request
/// never got a response — a genuine connection failure, not an HTTP error
/// status. Returns `UNREACHABLE_MESSAGE` for convenience at call sites.
fn mark_offline() -> String {
    *IS_ONLINE.write() = false;
    UNREACHABLE_MESSAGE.to_string()
}

/// Marks the backend reachable — called as soon as any HTTP response
/// arrives at all, even a rejection, since that still proves the server is
/// up and talking to us.
fn mark_online() {
    if !*IS_ONLINE.read() {
        *IS_ONLINE.write() = true;
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn get_json<R>(url: &str) -> Result<R, String>
where
    R: serde::de::DeserializeOwned,
{
    get_json_auth(url, None).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn get_json_auth<R>(url: &str, token: Option<&str>) -> Result<R, String>
where
    R: serde::de::DeserializeOwned,
{
    let mut request = reqwest::Client::new().get(url);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = request.send().await.map_err(|_| mark_offline())?;
    mark_online();
    if !response.status().is_success() {
        let msg = response
            .json::<api::ApiError>()
            .await
            .map(|e| e.message)
            .unwrap_or_else(|_| "Request failed".to_string());
        return Err(msg);
    }
    response.json::<R>().await.map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
async fn get_json<R>(url: &str) -> Result<R, String>
where
    R: serde::de::DeserializeOwned,
{
    get_json_auth(url, None).await
}

#[cfg(target_arch = "wasm32")]
async fn get_json_auth<R>(url: &str, token: Option<&str>) -> Result<R, String>
where
    R: serde::de::DeserializeOwned,
{
    let mut builder = Request::get(url);
    if let Some(token) = token {
        builder = builder.header("Authorization", &format!("Bearer {token}"));
    }
    let response = builder.send().await.map_err(|_| mark_offline())?;
    mark_online();
    if !response.ok() {
        let msg = response
            .json::<api::ApiError>()
            .await
            .map(|e| e.message)
            .unwrap_or_else(|_| format!("HTTP {} {}", response.status(), response.status_text()));
        return Err(msg);
    }
    response
        .json::<R>()
        .await
        .map_err(|error| error.to_string())
}

/// Plain-text GET — for the dictionary word list (`get_json` assumes a
/// JSON body, which this isn't). Only the wasm build calls this: the
/// native dictionary is compiled in, so `load_client_dictionary`'s native
/// path never needs to fetch anything.
#[cfg(target_arch = "wasm32")]
async fn get_text(url: &str) -> Result<String, String> {
    let response = Request::get(url)
        .send()
        .await
        .map_err(|_| mark_offline())?;
    mark_online();
    if !response.ok() {
        return Err(format!("HTTP {} {}", response.status(), response.status_text()));
    }
    response.text().await.map_err(|error| error.to_string())
}

/// The dictionary the live move preview validates against, for the given
/// `VariantRules.language` (e.g. "sowpods", "enable2k"). Native builds
/// (server and desktop) already have every known dictionary compiled in —
/// nothing to fetch. The wasm/web build deliberately doesn't embed any of
/// them (see `rules_shared::dictionary`'s doc comments), so this is a real
/// network round-trip there, resolving to `None` if it fails or the
/// language is unrecognized (the preview just stays absent, same as while
/// nothing's staged yet — never a hard error).
#[cfg(not(target_arch = "wasm32"))]
async fn load_client_dictionary(
    _server_url: &str,
    language: &str,
) -> Option<&'static rules_shared::WordListDictionary> {
    rules_shared::dictionary_by_name(language)
}

#[cfg(target_arch = "wasm32")]
async fn load_client_dictionary(
    server_url: &str,
    language: &str,
) -> Option<&'static rules_shared::WordListDictionary> {
    let text = get_text(&format!("{server_url}/dictionaries/{language}"))
        .await
        .ok()?;
    Some(Box::leak(Box::new(
        rules_shared::WordListDictionary::from_word_list(text),
    )))
}

#[cfg(not(target_arch = "wasm32"))]
async fn post_json_impl<T, R>(url: &str, token: Option<&str>, payload: &T) -> Result<R, String>
where
    T: serde::Serialize + ?Sized,
    R: serde::de::DeserializeOwned,
{
    let mut request = reqwest::Client::new().post(url).json(payload);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = request.send().await.map_err(|_| mark_offline())?;
    mark_online();
    if !response.status().is_success() {
        let msg = response
            .json::<api::ApiError>()
            .await
            .map(|e| e.message)
            .unwrap_or_else(|_| "Request failed".to_string());
        return Err(msg);
    }
    response.json::<R>().await.map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
async fn post_json_impl<T, R>(url: &str, token: Option<&str>, payload: &T) -> Result<R, String>
where
    T: serde::Serialize + ?Sized,
    R: serde::de::DeserializeOwned,
{
    let mut builder = Request::post(url);
    if let Some(token) = token {
        builder = builder.header("Authorization", &format!("Bearer {token}"));
    }
    let response = builder
        .json(payload)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|_| mark_offline())?;
    mark_online();
    if !response.ok() {
        let msg = response
            .json::<api::ApiError>()
            .await
            .map(|e| e.message)
            .unwrap_or_else(|_| format!("HTTP {} {}", response.status(), response.status_text()));
        return Err(msg);
    }
    response
        .json::<R>()
        .await
        .map_err(|error| error.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
async fn post_no_content_impl<T>(url: &str, token: Option<&str>, payload: &T) -> Result<(), String>
where
    T: serde::Serialize + ?Sized,
{
    let mut request = reqwest::Client::new().post(url).json(payload);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = request.send().await.map_err(|_| mark_offline())?;
    mark_online();
    if !response.status().is_success() {
        let msg = response
            .json::<api::ApiError>()
            .await
            .map(|e| e.message)
            .unwrap_or_else(|_| "Request failed".to_string());
        return Err(msg);
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
async fn post_no_content_impl<T>(url: &str, token: Option<&str>, payload: &T) -> Result<(), String>
where
    T: serde::Serialize + ?Sized,
{
    let mut builder = Request::post(url);
    if let Some(token) = token {
        builder = builder.header("Authorization", &format!("Bearer {token}"));
    }
    let response = builder
        .json(payload)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|_| mark_offline())?;
    mark_online();
    if !response.ok() {
        let msg = response
            .json::<api::ApiError>()
            .await
            .map(|e| e.message)
            .unwrap_or_else(|_| format!("HTTP {} {}", response.status(), response.status_text()));
        return Err(msg);
    }
    Ok(())
}

async fn subscribe_to_game_events(
    server_url: &str,
    game_id: &str,
    token: Option<&str>,
    game_signal: Signal<Option<GameStateDto>>,
    websocket_game_id: Signal<Option<String>>,
) -> Result<(), String> {
    subscribe_to_game_events_impl(server_url, game_id, token, game_signal, websocket_game_id).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn subscribe_to_game_events_impl(
    server_url: &str,
    game_id: &str,
    token: Option<&str>,
    mut game_signal: Signal<Option<GameStateDto>>,
    websocket_game_id: Signal<Option<String>>,
) -> Result<(), String> {
    let ws_url = websocket_url(server_url, game_id, token)?;
    let (stream, _) = connect_async(ws_url).await.map_err(|_| mark_offline())?;
    mark_online();
    let (_, mut read) = stream.split();

    while let Some(message) = read.next().await {
        // The player may have switched to a different game while this
        // connection was awaiting its next message. Stop applying updates
        // (and drop `read`, closing the socket) rather than clobbering the
        // now-selected game's state with this abandoned game's events.
        if websocket_game_id.peek().as_deref() != Some(game_id) {
            break;
        }
        let message = message.map_err(|_| mark_offline())?;
        let text = match message.to_text() {
            Ok(text) => text,
            Err(_) => continue,
        };
        let event =
            serde_json::from_str::<GameEventDto>(text).map_err(|error| error.to_string())?;
        let updated = match event {
            GameEventDto::StateUpdated { game }
            | GameEventDto::GameStarted { game }
            | GameEventDto::GameFinished { game } => game,
        };
        if updated.id == game_id {
            game_signal.set(Some(updated));
        }
    }

    Ok(())
}

#[cfg(target_arch = "wasm32")]
async fn subscribe_to_game_events_impl(
    server_url: &str,
    game_id: &str,
    token: Option<&str>,
    mut game_signal: Signal<Option<GameStateDto>>,
    websocket_game_id: Signal<Option<String>>,
) -> Result<(), String> {
    let ws_url = websocket_url(server_url, game_id, token)?;
    let mut read = WebSocket::open(&ws_url).map_err(|_| mark_offline())?;
    mark_online();

    while let Some(message) = read.next().await {
        // See the native impl's comment: without this check, a connection
        // left over from a game the player has since navigated away from
        // keeps delivering events that would otherwise overwrite the
        // currently-selected game's state.
        if websocket_game_id.peek().as_deref() != Some(game_id) {
            break;
        }
        let message = message.map_err(|_| mark_offline())?;
        let text = match message {
            WsMessage::Text(text) => text,
            WsMessage::Bytes(_) => continue,
        };
        let event =
            serde_json::from_str::<GameEventDto>(&text).map_err(|error| error.to_string())?;
        let updated = match event {
            GameEventDto::StateUpdated { game }
            | GameEventDto::GameStarted { game }
            | GameEventDto::GameFinished { game } => game,
        };
        if updated.id == game_id {
            game_signal.set(Some(updated));
        }
    }

    Ok(())
}

/// `token` travels as a query parameter, not the `Authorization` header
/// every other request uses — browsers' native `WebSocket` API can't set
/// custom headers on the handshake. Session tokens are plain UUIDs (hex
/// digits and hyphens only), so no percent-encoding is needed here.
fn websocket_url(server_url: &str, game_id: &str, token: Option<&str>) -> Result<String, String> {
    if let Some(url) = server_url.strip_prefix("http://") {
        return Ok(with_token_query(format!("ws://{url}/games/{game_id}/events"), token));
    }
    if let Some(url) = server_url.strip_prefix("https://") {
        return Ok(with_token_query(format!("wss://{url}/games/{game_id}/events"), token));
    }
    // An empty `server_url` means "same origin as the page" (see
    // `default_server_url` — used when a reverse proxy serves both the
    // static assets and the API from one host, e.g. the container
    // deployment). There's no explicit scheme/host to rewrite in that case,
    // so it's derived from the browser's own location instead.
    if server_url.is_empty() {
        return same_origin_websocket_url(game_id, token);
    }
    Err(format!("Unsupported server url: {server_url}"))
}

fn with_token_query(url: String, token: Option<&str>) -> String {
    match token {
        Some(token) => format!("{url}?token={token}"),
        None => url,
    }
}

/// The app has no router (see `crates/ui/src/views/reset_password.rs`'s doc
/// comment for why) — this is the one place a URL path/query is read to
/// decide what to render. Only meaningful on web: a password-reset link is
/// always clicked from an email client into a browser, never opened by the
/// desktop build, which has no URL bar to land a deep link on.
#[cfg(target_arch = "wasm32")]
fn reset_password_token_from_url() -> Option<String> {
    let location = web_sys::window()?.location();
    if location.pathname().ok()?.trim_end_matches('/') != "/reset-password" {
        return None;
    }
    let search = location.search().ok()?;
    // Tokens are plain UUIDs (hex digits and hyphens only), so there's
    // nothing here that could need percent-decoding — a plain split is
    // enough without pulling in a URL-encoding crate for this one call site.
    search.strip_prefix('?')?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token" && !value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn reset_password_token_from_url() -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
fn same_origin_websocket_url(game_id: &str, token: Option<&str>) -> Result<String, String> {
    let location = web_sys::window()
        .ok_or_else(|| "No browser window available".to_string())?
        .location();
    let protocol = location
        .protocol()
        .map_err(|_| "Could not read page protocol".to_string())?;
    let host = location
        .host()
        .map_err(|_| "Could not read page host".to_string())?;
    let ws_scheme = if protocol == "https:" { "wss" } else { "ws" };
    Ok(with_token_query(
        format!("{ws_scheme}://{host}/games/{game_id}/events"),
        token,
    ))
}

/// The desktop build never runs with a same-origin (empty) `server_url` — it
/// always talks to an explicit configured server — so this is unreachable
/// in practice; it exists only so `websocket_url` compiles for both targets.
#[cfg(not(target_arch = "wasm32"))]
fn same_origin_websocket_url(_game_id: &str, _token: Option<&str>) -> Result<String, String> {
    Err("Same-origin server URLs are only supported on the web build".to_string())
}

fn build_manual_move_request(
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    direction_hint: DirectionDto,
) -> Result<GameActionRequest, String> {
    if game.status != GameStatus::Active {
        return Err("Game is not active".to_string());
    }
    if staged.is_empty() {
        return Err("No staged placements to submit.".to_string());
    }

    let mut placements = staged.to_vec();
    placements.sort_by_key(|placement| placement.board_index);

    let positions: Vec<(u8, u8)> = placements
        .iter()
        .map(|p| {
            (
                (p.board_index % BOARD_WIDTH) as u8,
                (p.board_index / BOARD_WIDTH) as u8,
            )
        })
        .collect();

    let same_row = positions.iter().all(|(_, y)| *y == positions[0].1);
    let same_column = positions.iter().all(|(x, _)| *x == positions[0].0);

    let direction = if placements.len() == 1 {
        infer_single_tile_direction(game, placements[0].board_index, direction_hint)
    } else if same_row {
        DirectionDto::Horizontal
    } else if same_column {
        DirectionDto::Vertical
    } else {
        return Err("Staged placements must be in a single row or single column.".to_string());
    };

    let (start_x, start_y) = match direction {
        DirectionDto::Horizontal => {
            let min_x = positions.iter().map(|(x, _)| *x).min().unwrap_or(0);
            (min_x, positions[0].1)
        }
        DirectionDto::Vertical => {
            let min_y = positions.iter().map(|(_, y)| *y).min().unwrap_or(0);
            (positions[0].0, min_y)
        }
    };

    let mut tile_placements = placements
        .into_iter()
        .map(|p| {
            let x = (p.board_index % BOARD_WIDTH) as u8;
            let y = (p.board_index / BOARD_WIDTH) as u8;
            let offset = match direction {
                DirectionDto::Horizontal => x - start_x,
                DirectionDto::Vertical => y - start_y,
            };
            TilePlacementDto {
                offset,
                tile: p.tile,
            }
        })
        .collect::<Vec<_>>();
    tile_placements.sort_by_key(|p| p.offset);

    Ok(GameActionRequest {
        seat_number: game.current_seat,
        action: api::PlayerActionDto::Place {
            candidate: MoveCandidateDto {
                start: PositionDto {
                    x: start_x,
                    y: start_y,
                },
                direction,
                tiles: tile_placements,
            },
        },
    })
}

/// Runs the same validation/scoring the server would for this candidate —
/// entirely locally, via `rules-shared` (the crate the server's own move
/// handling is built on) — so the live preview is instant and needs no
/// network round-trip. The server still gets the final say when the move
/// is actually submitted (`submit_manual_move`); this is purely a
/// responsiveness optimization for the composer, not a trust boundary.
fn compute_client_preview(
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    direction_hint: DirectionDto,
    dictionary: &rules_shared::WordListDictionary,
) -> Option<MovePreviewView> {
    let request = match build_manual_move_request(game, staged, direction_hint) {
        Ok(r) => r,
        Err(detail) => {
            return Some(MovePreviewView {
                is_legal: false,
                headline: "Cannot preview this arrangement".to_string(),
                detail,
                score: None,
            });
        }
    };
    let candidate_dto = match request.action {
        api::PlayerActionDto::Place { candidate } => candidate,
        _ => return None,
    };

    // `language`/`board_layout` are vestigial display fields derived from
    // `variant` server-side (see `VariantRules` — an edition bundles all
    // three under one name), so `variant` alone determines which ruleset
    // this game actually uses. Falls back to no preview (rather than a
    // wrong one) for an edition this client doesn't recognize.
    let Some(rules) = rules_shared::VariantRules::by_name(&game.variant) else {
        return None;
    };
    let board_state = crate::client_rules::to_rules_board_state(&game.board, &rules.alphabet);
    let state = rules_shared::GameState::from_board(board_state, &rules, dictionary);
    let rack = game
        .racks
        .get(request.seat_number as usize)
        .map(crate::client_rules::to_rules_rack);
    let candidate = crate::client_rules::to_rules_candidate(&candidate_dto, &rules.alphabet);

    let engine = rules_shared::RulesEngine {
        rules: &rules,
        dictionary,
    };

    match engine.validate_game_move(&state, rack.as_ref(), &candidate) {
        Ok(validated) => Some(MovePreviewView {
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
        }),
        Err(error) => Some(MovePreviewView {
            is_legal: false,
            headline: rules_shared::format_move_error(&error),
            detail: String::new(),
            score: None,
        }),
    }
}

fn infer_single_tile_direction(
    game: &GameStateDto,
    board_index: usize,
    direction_hint: DirectionDto,
) -> DirectionDto {
    let x = board_index % BOARD_WIDTH;
    let y = board_index / BOARD_WIDTH;

    let has_horizontal_neighbor = (x > 0
        && game
            .board
            .get(board_index - 1)
            .is_some_and(board_cell_has_letter))
        || (x + 1 < BOARD_WIDTH
            && game
                .board
                .get(board_index + 1)
                .is_some_and(board_cell_has_letter));
    let has_vertical_neighbor = (y > 0
        && game
            .board
            .get(board_index - BOARD_WIDTH)
            .is_some_and(board_cell_has_letter))
        || (y + 1 < BOARD_HEIGHT
            && game
                .board
                .get(board_index + BOARD_WIDTH)
                .is_some_and(board_cell_has_letter));

    match (has_horizontal_neighbor, has_vertical_neighbor) {
        (true, false) => DirectionDto::Horizontal,
        (false, true) => DirectionDto::Vertical,
        _ => direction_hint,
    }
}

fn board_cell_has_letter(cell: &BoardCellDto) -> bool {
    cell.letter.is_some()
}

/// The direction this turn's staged placements would submit in — same
/// logic `build_manual_move_request` uses, exposed standalone so the
/// click/keyboard composer can auto-advance in the direction the move will
/// actually be read in, not always horizontally.
///
/// With exactly one staged tile the direction is inherently ambiguous, so
/// two extra (purely-current-state) signals get a say, in priority order:
/// `selected_cell`, when it's aligned with the staged tile on one axis, is
/// the strongest signal — it's the player explicitly clicking elsewhere to
/// point out which way the word should run. Failing that, `direction_override`
/// (set by the space-bar/button toggle) wins. Only once both are silent does
/// this fall back to the permanent-neighbor-based guess.
fn infer_typing_direction(
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    selected_cell: Option<usize>,
    direction_override: Option<DirectionDto>,
) -> DirectionDto {
    match staged.len() {
        0 => DirectionDto::Horizontal,
        1 => {
            let anchor = staged[0].board_index;
            if let Some(selected) = selected_cell {
                if let Some(direction) = aligned_direction(anchor, selected) {
                    return direction;
                }
            }
            infer_single_tile_direction(
                game,
                anchor,
                direction_override.unwrap_or(DirectionDto::Horizontal),
            )
        }
        _ => {
            let positions: Vec<(usize, usize)> = staged
                .iter()
                .map(|p| (p.board_index % BOARD_WIDTH, p.board_index / BOARD_WIDTH))
                .collect();
            let same_row = positions.iter().all(|(_, y)| *y == positions[0].1);
            if same_row {
                DirectionDto::Horizontal
            } else {
                DirectionDto::Vertical
            }
        }
    }
}

/// If `other` shares exactly one axis with `anchor` (same row, different
/// column, or same column, different row), returns the direction that
/// alignment implies. `None` if they're the same cell or share neither axis
/// (diagonal) — genuinely ambiguous, not this function's call to make.
fn aligned_direction(anchor: usize, other: usize) -> Option<DirectionDto> {
    if anchor == other {
        return None;
    }
    let (ax, ay) = (anchor % BOARD_WIDTH, anchor / BOARD_WIDTH);
    let (ox, oy) = (other % BOARD_WIDTH, other / BOARD_WIDTH);
    match (ax == ox, ay == oy) {
        (false, true) => Some(DirectionDto::Horizontal),
        (true, false) => Some(DirectionDto::Vertical),
        _ => None,
    }
}

/// Steps one cell from `index` in `direction`; `forward` picks which way
/// along that axis. Returns `None` at the board edge.
fn step_index(index: usize, direction: DirectionDto, forward: bool) -> Option<usize> {
    let x = index % BOARD_WIDTH;
    let y = index / BOARD_WIDTH;
    match (direction, forward) {
        (DirectionDto::Horizontal, true) => (x + 1 < BOARD_WIDTH).then(|| index + 1),
        (DirectionDto::Horizontal, false) => (x > 0).then(|| index - 1),
        (DirectionDto::Vertical, true) => (y + 1 < BOARD_HEIGHT).then(|| index + BOARD_WIDTH),
        (DirectionDto::Vertical, false) => (y > 0).then(|| index - BOARD_WIDTH),
    }
}

/// Walks from `from_index` in `direction`, skipping over cells that are
/// already occupied (a permanently-played letter, or a tile staged earlier
/// this turn), and returns the first free one. Used to auto-advance to the
/// next slot when typing a word — skipping past a tile just staged is
/// correct going forward, since typing shouldn't double back onto what it
/// just placed.
fn find_next_placeable_cell(
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    from_index: usize,
    direction: DirectionDto,
    forward: bool,
) -> Option<usize> {
    let mut current = from_index;
    loop {
        current = step_index(current, direction, forward)?;
        let is_permanent = game
            .board
            .get(current)
            .is_some_and(board_cell_has_letter);
        let is_staged = staged.iter().any(|p| p.board_index == current);
        if !is_permanent && !is_staged {
            return Some(current);
        }
    }
}

/// Like `step_index`, but wraps around to the other side of the row/column
/// instead of stopping at the board edge.
fn step_index_wrapping(index: usize, direction: DirectionDto, forward: bool) -> usize {
    let x = index % BOARD_WIDTH;
    let y = index / BOARD_WIDTH;
    match (direction, forward) {
        (DirectionDto::Horizontal, true) => y * BOARD_WIDTH + (x + 1) % BOARD_WIDTH,
        (DirectionDto::Horizontal, false) => y * BOARD_WIDTH + (x + BOARD_WIDTH - 1) % BOARD_WIDTH,
        (DirectionDto::Vertical, true) => ((y + 1) % BOARD_HEIGHT) * BOARD_WIDTH + x,
        (DirectionDto::Vertical, false) => ((y + BOARD_HEIGHT - 1) % BOARD_HEIGHT) * BOARD_WIDTH + x,
    }
}

/// Arrow-key navigation's counterpart to `find_next_placeable_cell`: wraps
/// around the edge of the row/column instead of stopping there, since
/// moving the cursor by hand should cycle rather than get stuck — unlike
/// advancing through a word being typed, which should stop at the edge
/// (see `find_next_placeable_cell`'s own doc comment). Still skips over
/// occupied cells the same way, and is bounded to one full lap of the
/// row/column, so a completely full line returns `None` (meaning: don't
/// move) instead of spinning forever.
fn find_next_placeable_cell_wrapping(
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    from_index: usize,
    direction: DirectionDto,
    forward: bool,
) -> Option<usize> {
    let line_length = match direction {
        DirectionDto::Horizontal => BOARD_WIDTH,
        DirectionDto::Vertical => BOARD_HEIGHT,
    };
    let mut current = from_index;
    for _ in 0..line_length.saturating_sub(1) {
        current = step_index_wrapping(current, direction, forward);
        let is_permanent = game.board.get(current).is_some_and(board_cell_has_letter);
        let is_staged = staged.iter().any(|p| p.board_index == current);
        if !is_permanent && !is_staged {
            return Some(current);
        }
    }
    None
}

/// Walks backward from `from_index`, skipping only permanently-played
/// letters (not staged ones — unlike `find_next_placeable_cell`), and
/// returns the first cell that isn't a permanent letter. That's the cell
/// backspace should act on: either a staged tile to remove, or — if nothing
/// has been typed there yet — the next empty slot to land the cursor on.
/// Landing *on* the previous staged tile (rather than skipping past it) is
/// what makes backspace delete exactly one tile per press.
fn find_previous_editable_cell(
    game: &GameStateDto,
    from_index: usize,
    direction: DirectionDto,
) -> Option<usize> {
    let mut current = from_index;
    loop {
        current = step_index(current, direction, false)?;
        let is_permanent = game
            .board
            .get(current)
            .is_some_and(board_cell_has_letter);
        if !is_permanent {
            return Some(current);
        }
    }
}

/// Moves `selected_cell` to the next placeable cell after `from_index`,
/// following the direction this turn's placements are currently reading
/// in. Falls back to reselecting `from_index` (the cell just played) at the
/// edge of the board, rather than clearing the selection or wrapping — so
/// typing a word that runs off the edge leaves the cursor on the last tile
/// placed instead of dropping it entirely.
fn advance_selection(
    game: &GameStateDto,
    staged_placements: Signal<Vec<StagedPlacementView>>,
    mut selected_cell: Signal<Option<usize>>,
    direction_override: Option<DirectionDto>,
    from_index: usize,
) {
    let staged = staged_placements();
    let direction = infer_typing_direction(game, &staged, Some(from_index), direction_override);
    let next = find_next_placeable_cell(game, &staged, from_index, direction, true);
    selected_cell.set(next.or(Some(from_index)));
}

/// Flips the effective typing direction (space bar / direction button).
/// Only meaningful with exactly one staged tile — with zero or two-plus,
/// direction isn't ambiguous, so this is a no-op. Moves `selected_cell` to
/// follow the new direction immediately, so the cursor lands where the next
/// letter would actually go rather than leaving it in the old direction's
/// slot.
fn toggle_direction_override(
    game: &GameStateDto,
    staged_placements: Signal<Vec<StagedPlacementView>>,
    mut direction_override: Signal<Option<DirectionDto>>,
    mut selected_cell: Signal<Option<usize>>,
) {
    let staged = staged_placements();
    if staged.len() != 1 {
        return;
    }
    let anchor = staged[0].board_index;
    let current = infer_typing_direction(game, &staged, selected_cell(), direction_override());
    let next = match current {
        DirectionDto::Horizontal => DirectionDto::Vertical,
        DirectionDto::Vertical => DirectionDto::Horizontal,
    };
    direction_override.set(Some(next));
    selected_cell.set(find_next_placeable_cell(game, &staged, anchor, next, true));
}

/// Builds the staged placement for dropping/clicking/typing `tile` onto
/// `board_index`. `resolved_letter` is `Some` only when a blank is being
/// auto-assigned a letter because the player typed it directly (keyboard
/// path) — the mouse path still leaves blanks unresolved for the
/// blank-letter picker, same as before.
fn stage_tile_at_cell(
    board_index: usize,
    tile: &RackTileView,
    resolved_letter: Option<String>,
) -> StagedPlacementView {
    let (tile_for_board, display_for_board) = match (&tile.tile, resolved_letter) {
        (TileDto::Blank { .. }, Some(letter)) => {
            let display = letter.to_lowercase();
            (TileDto::Blank { acting_as: Some(letter) }, display)
        }
        (TileDto::Blank { .. }, None) => (TileDto::Blank { acting_as: None }, "?".to_string()),
        (other, _) => (other.clone(), tile.display.clone()),
    };
    StagedPlacementView {
        board_index,
        rack_tile_id: tile.id,
        display: display_for_board,
        tile: tile_for_board,
    }
}

/// Resets everything about an in-progress move/exchange composition —
/// called whenever the game state moves on from under it (a new game
/// loaded, an action submitted, a turn started).
fn reset_composer_state(
    mut dragging_tile_id: Signal<Option<usize>>,
    mut selected_blank_letter: Signal<Option<String>>,
    mut staged_placements: Signal<Vec<StagedPlacementView>>,
    mut selected_cell: Signal<Option<usize>>,
    mut exchange_mode: Signal<bool>,
    mut exchange_selected: Signal<HashSet<usize>>,
    mut direction_override: Signal<Option<DirectionDto>>,
) {
    dragging_tile_id.set(None);
    selected_blank_letter.set(None);
    staged_placements.set(Vec::new());
    selected_cell.set(None);
    exchange_mode.set(false);
    exchange_selected.set(HashSet::new());
    direction_override.set(None);
}

/// True if this seat is either unclaimed (anonymous/open play — anyone may
/// view or act on it, unchanged from before) or claimed by exactly this
/// viewer. Mirrors the server's own ownership rule (`submit_action` et al)
/// client-side, so the UI doesn't show a seat as live only for the server
/// to reject the attempt after the fact.
fn seat_is_open_or_owned_by(participant: &ParticipantDto, viewer_player_id: Option<&str>) -> bool {
    match participant.player_id.as_deref() {
        None => true,
        Some(owner) => Some(owner) == viewer_player_id,
    }
}

/// The seat whose rack this viewer should see, if any: their own claimed
/// seat (regardless of whose turn it is), else the current seat if it's
/// unclaimed, else `None`.
fn viewer_rack_seat(game: &GameStateDto, viewer_player_id: Option<&str>) -> Option<usize> {
    if let Some(viewer_player_id) = viewer_player_id {
        if let Some(owned) = game
            .participants
            .iter()
            .find(|participant| participant.player_id.as_deref() == Some(viewer_player_id))
        {
            return Some(owned.seat_number as usize);
        }
    }
    let current = game.participants.get(game.current_seat as usize)?;
    if current.player_id.is_none() {
        Some(game.current_seat as usize)
    } else {
        None
    }
}

fn current_rack_tiles(game: &GameStateDto, staged: &[StagedPlacementView]) -> Vec<RackTileView> {
    rack_tiles_for_seat(game, Some(game.current_seat as usize), staged)
}

fn rack_tiles_for_seat(
    game: &GameStateDto,
    seat_index: Option<usize>,
    staged: &[StagedPlacementView],
) -> Vec<RackTileView> {
    let Some(seat_index) = seat_index else {
        return Vec::new();
    };
    let Some(rack) = game.racks.get(seat_index) else {
        return Vec::new();
    };

    let rules = rules_shared::VariantRules::by_name(&game.variant)
        .unwrap_or_else(rules_shared::VariantRules::official);
    let used_ids: std::collections::HashSet<usize> = staged
        .iter()
        .map(|placement| placement.rack_tile_id)
        .collect();
    let mut next_id = 0usize;
    let mut tiles = Vec::new();

    for (index, count) in rack.counts.iter().enumerate() {
        let Some(grapheme) = rules.alphabet.to_grapheme(rules_shared::Letter::from(index as u8))
        else {
            continue;
        };
        let letter_text = grapheme.to_string();
        for _ in 0..*count {
            tiles.push(RackTileView {
                id: next_id,
                display: letter_text.clone(),
                tile: TileDto::Letter {
                    letter: letter_text.clone(),
                },
                is_used: used_ids.contains(&next_id),
            });
            next_id += 1;
        }
    }

    for _ in 0..rack.blanks {
        tiles.push(RackTileView {
            id: next_id,
            display: "*".to_string(),
            tile: TileDto::Blank { acting_as: None },
            is_used: used_ids.contains(&next_id),
        });
        next_id += 1;
    }

    tiles
}

/// Reorders already-computed rack tiles for display — `rack_tiles_for_seat`
/// always returns them in a fixed alphabetical order, so the visual
/// shuffle lives entirely in which permutation gets applied on top of it.
/// Out-of-range indices (shouldn't happen; `order` is reset to identity
/// whenever the tile count changes) are silently dropped rather than
/// panicking.
fn apply_rack_order(tiles: &[RackTileView], order: &[usize]) -> Vec<RackTileView> {
    order.iter().filter_map(|&i| tiles.get(i).cloned()).collect()
}

/// Moves `dragged_id` to sit where `target_id` currently is (dragging a
/// rack tile onto another one to reorder the rack), shifting everything
/// between them over by one. A no-op (returns `order` unchanged) if either
/// id isn't present or they're the same tile.
fn reorder_rack_order(order: &[usize], dragged_id: usize, target_id: usize) -> Vec<usize> {
    if dragged_id == target_id {
        return order.to_vec();
    }
    let (Some(from), Some(to)) = (
        order.iter().position(|&id| id == dragged_id),
        order.iter().position(|&id| id == target_id),
    ) else {
        return order.to_vec();
    };
    let mut new_order = order.to_vec();
    new_order.remove(from);
    let insert_at = if from < to { to - 1 } else { to };
    new_order.insert(insert_at, dragged_id);
    new_order
}

#[cfg(target_arch = "wasm32")]
fn random_index_below(bound: usize) -> usize {
    (js_sys::Math::random() * bound as f64) as usize
}

#[cfg(not(target_arch = "wasm32"))]
fn random_index_below(bound: usize) -> usize {
    use rand::Rng;
    rand::thread_rng().gen_range(0..bound)
}

/// Fisher–Yates, in place.
fn shuffle_order(order: &mut [usize]) {
    for i in (1..order.len()).rev() {
        let j = random_index_below(i + 1);
        order.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tile(id: usize, letter: char) -> RackTileView {
        RackTileView {
            id,
            display: letter.to_string(),
            tile: TileDto::Letter {
                letter: letter.to_string(),
            },
            is_used: false,
        }
    }

    #[test]
    fn apply_rack_order_reorders_tiles_to_match() {
        let tiles = vec![sample_tile(0, 'A'), sample_tile(1, 'B'), sample_tile(2, 'C')];
        let reordered = apply_rack_order(&tiles, &[2, 0, 1]);
        let letters: Vec<String> = reordered.iter().map(|t| t.display.clone()).collect();
        assert_eq!(letters, vec!["C", "A", "B"]);
    }

    #[test]
    fn apply_rack_order_drops_out_of_range_indices_defensively() {
        let tiles = vec![sample_tile(0, 'A'), sample_tile(1, 'B')];
        let reordered = apply_rack_order(&tiles, &[1, 5, 0]);
        let letters: Vec<String> = reordered.iter().map(|t| t.display.clone()).collect();
        assert_eq!(letters, vec!["B", "A"]);
    }

    #[test]
    fn reorder_rack_order_moves_a_tile_forward_to_the_target() {
        // [A,B,C,D], drag A onto C -> A ends up right before C.
        assert_eq!(reorder_rack_order(&[0, 1, 2, 3], 0, 2), vec![1, 0, 2, 3]);
    }

    #[test]
    fn reorder_rack_order_moves_a_tile_backward_to_the_target() {
        // [A,B,C,D], drag D onto B -> D takes B's slot, B and C shift right.
        assert_eq!(reorder_rack_order(&[0, 1, 2, 3], 3, 1), vec![0, 3, 1, 2]);
    }

    #[test]
    fn reorder_rack_order_dropping_a_tile_on_itself_is_a_no_op() {
        assert_eq!(reorder_rack_order(&[0, 1, 2], 1, 1), vec![0, 1, 2]);
    }

    #[test]
    fn reorder_rack_order_ignores_an_unknown_id() {
        assert_eq!(reorder_rack_order(&[0, 1, 2], 5, 1), vec![0, 1, 2]);
        assert_eq!(reorder_rack_order(&[0, 1, 2], 0, 5), vec![0, 1, 2]);
    }

    #[test]
    fn shuffle_order_stays_a_permutation_of_the_same_indices() {
        let mut order: Vec<usize> = (0..7).collect();
        shuffle_order(&mut order);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..7).collect::<Vec<_>>());
    }

    #[test]
    fn shuffle_order_leaves_a_single_tile_in_place() {
        let mut order = vec![0];
        shuffle_order(&mut order);
        assert_eq!(order, vec![0]);
    }

    #[test]
    fn websocket_url_rewrites_http_and_https_schemes() {
        assert_eq!(
            websocket_url("http://example.com:3000", "game-1", None),
            Ok("ws://example.com:3000/games/game-1/events".to_string())
        );
        assert_eq!(
            websocket_url("https://example.com", "game-1", None),
            Ok("wss://example.com/games/game-1/events".to_string())
        );
    }

    #[test]
    fn websocket_url_appends_a_token_query_parameter_when_present() {
        assert_eq!(
            websocket_url("http://example.com:3000", "game-1", Some("tok-123")),
            Ok("ws://example.com:3000/games/game-1/events?token=tok-123".to_string())
        );
    }

    #[test]
    fn websocket_url_rejects_an_unrecognized_scheme() {
        assert!(websocket_url("ftp://example.com", "game-1", None).is_err());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn websocket_url_same_origin_is_only_supported_on_web() {
        // The desktop/native build never actually passes an empty
        // server_url (it always talks to an explicit configured server),
        // so this just confirms the fallback doesn't panic — same-origin
        // resolution itself is only meaningful in a browser.
        assert!(websocket_url("", "game-1", None).is_err());
    }

    #[test]
    fn matching_version_is_compatible() {
        let v = api::ApiVersion { major: 1, minor: 2 };
        assert_eq!(compare_api_version(v, v), VersionCheck::Compatible);
    }

    #[test]
    fn differing_major_is_a_hard_mismatch() {
        let server = api::ApiVersion { major: 2, minor: 0 };
        let client = api::ApiVersion { major: 1, minor: 0 };
        assert_eq!(
            compare_api_version(server, client),
            VersionCheck::MajorMismatch { server, client }
        );
    }

    #[test]
    fn differing_minor_with_matching_major_is_a_soft_mismatch() {
        let server = api::ApiVersion { major: 1, minor: 3 };
        let client = api::ApiVersion { major: 1, minor: 1 };
        assert_eq!(
            compare_api_version(server, client),
            VersionCheck::MinorMismatch { server, client }
        );
    }

    fn test_game(board: Vec<BoardCellDto>) -> GameStateDto {
        GameStateDto {
            board,
            ..empty_live_game()
        }
    }

    fn letter_placement(board_index: usize, rack_tile_id: usize, letter: char) -> StagedPlacementView {
        StagedPlacementView {
            board_index,
            rack_tile_id,
            display: letter.to_string(),
            tile: TileDto::Letter {
                letter: letter.to_string(),
            },
        }
    }

    #[test]
    fn step_index_stops_at_board_edges() {
        assert_eq!(step_index(0, DirectionDto::Horizontal, false), None);
        assert_eq!(step_index(0, DirectionDto::Vertical, false), None);
        assert_eq!(step_index(0, DirectionDto::Horizontal, true), Some(1));
        assert_eq!(step_index(0, DirectionDto::Vertical, true), Some(BOARD_WIDTH));

        let last = BOARD_WIDTH * BOARD_HEIGHT - 1;
        assert_eq!(step_index(last, DirectionDto::Horizontal, true), None);
        assert_eq!(step_index(last, DirectionDto::Vertical, true), None);
    }

    #[test]
    fn find_next_placeable_cell_skips_permanent_and_staged_tiles() {
        let mut board = empty_board();
        board[12].letter = Some("A".to_string());
        let game = test_game(board);
        let staged = vec![letter_placement(11, 0, 'B')];

        // From 10 going right: 11 is staged, 12 is permanently filled, 13 is
        // the first genuinely free cell.
        assert_eq!(
            find_next_placeable_cell(&game, &staged, 10, DirectionDto::Horizontal, true),
            Some(13)
        );
    }

    #[test]
    fn find_next_placeable_cell_returns_none_past_the_edge() {
        let game = test_game(empty_board());
        assert_eq!(
            find_next_placeable_cell(&game, &[], BOARD_WIDTH - 1, DirectionDto::Horizontal, true),
            None
        );
    }

    #[test]
    fn step_index_wrapping_cycles_to_the_other_side_of_the_row_or_column() {
        assert_eq!(
            step_index_wrapping(BOARD_WIDTH - 1, DirectionDto::Horizontal, true),
            0
        );
        assert_eq!(
            step_index_wrapping(0, DirectionDto::Horizontal, false),
            BOARD_WIDTH - 1
        );
        let last_row_start = (BOARD_HEIGHT - 1) * BOARD_WIDTH;
        assert_eq!(
            step_index_wrapping(last_row_start, DirectionDto::Vertical, true),
            0
        );
        assert_eq!(
            step_index_wrapping(0, DirectionDto::Vertical, false),
            last_row_start
        );
    }

    #[test]
    fn find_next_placeable_cell_wrapping_cycles_around_the_board_edge() {
        // Arrow-key navigation (unlike advancing through a typed word)
        // should wrap rather than stop at the edge.
        let game = test_game(empty_board());
        assert_eq!(
            find_next_placeable_cell_wrapping(
                &game,
                &[],
                BOARD_WIDTH - 1,
                DirectionDto::Horizontal,
                true
            ),
            Some(0)
        );
    }

    #[test]
    fn find_next_placeable_cell_wrapping_skips_occupied_cells_on_the_way_around() {
        let mut board = empty_board();
        // Fill the whole row except index 5.
        for x in 0..BOARD_WIDTH {
            if x != 5 {
                board[x].letter = Some("A".to_string());
            }
        }
        let game = test_game(board);
        assert_eq!(
            find_next_placeable_cell_wrapping(&game, &[], 4, DirectionDto::Horizontal, true),
            Some(5)
        );
    }

    #[test]
    fn find_next_placeable_cell_wrapping_returns_none_when_the_whole_line_is_full() {
        let mut board = empty_board();
        for x in 0..BOARD_WIDTH {
            board[x].letter = Some("A".to_string());
        }
        let game = test_game(board);
        assert_eq!(
            find_next_placeable_cell_wrapping(&game, &[], 4, DirectionDto::Horizontal, true),
            None
        );
    }

    #[test]
    fn find_previous_editable_cell_lands_on_the_immediately_preceding_staged_tile() {
        // Regression test: backspace used to skip straight past the last
        // staged tile (landing one cell further back than intended) because
        // it reused the forward-advance helper, which treats staged cells
        // as something to skip over rather than stop on.
        let game = test_game(empty_board());
        assert_eq!(
            find_previous_editable_cell(&game, 12, DirectionDto::Horizontal),
            Some(11)
        );
    }

    #[test]
    fn find_previous_editable_cell_skips_over_permanent_letters_only() {
        let mut board = empty_board();
        board[11].letter = Some("A".to_string());
        board[10].letter = Some("B".to_string());
        let game = test_game(board);

        // From 12 going left: 11 and 10 are permanent, so backspace should
        // land on 9 — the first cell that isn't a permanent letter.
        assert_eq!(
            find_previous_editable_cell(&game, 12, DirectionDto::Horizontal),
            Some(9)
        );
    }

    #[test]
    fn find_previous_editable_cell_returns_none_past_the_edge() {
        let game = test_game(empty_board());
        assert_eq!(
            find_previous_editable_cell(&game, 0, DirectionDto::Horizontal),
            None
        );
    }

    #[test]
    fn infer_typing_direction_defaults_to_horizontal_when_empty_or_isolated() {
        let game = test_game(empty_board());
        assert_eq!(
            infer_typing_direction(&game, &[], None, None),
            DirectionDto::Horizontal
        );

        let staged = vec![letter_placement(112, 0, 'A')];
        assert_eq!(
            infer_typing_direction(&game, &staged, None, None),
            DirectionDto::Horizontal
        );
    }

    #[test]
    fn infer_typing_direction_follows_the_existing_neighbor_for_a_single_tile() {
        let staged = vec![letter_placement(112, 0, 'A')];

        let mut board_with_left_neighbor = empty_board();
        board_with_left_neighbor[111].letter = Some("C".to_string());
        let game = test_game(board_with_left_neighbor);
        assert_eq!(
            infer_typing_direction(&game, &staged, None, None),
            DirectionDto::Horizontal
        );

        let mut board_with_top_neighbor = empty_board();
        board_with_top_neighbor[112 - BOARD_WIDTH].letter = Some("C".to_string());
        let game = test_game(board_with_top_neighbor);
        assert_eq!(
            infer_typing_direction(&game, &staged, None, None),
            DirectionDto::Vertical
        );
    }

    #[test]
    fn infer_typing_direction_follows_multi_tile_alignment() {
        let game = test_game(empty_board());

        let same_row = vec![letter_placement(100, 0, 'A'), letter_placement(102, 1, 'B')];
        assert_eq!(
            infer_typing_direction(&game, &same_row, None, None),
            DirectionDto::Horizontal
        );

        let same_column = vec![
            letter_placement(100, 0, 'A'),
            letter_placement(100 + BOARD_WIDTH * 2, 1, 'B'),
        ];
        assert_eq!(
            infer_typing_direction(&game, &same_column, None, None),
            DirectionDto::Vertical
        );
    }

    #[test]
    fn infer_typing_direction_follows_the_selected_cell_for_a_single_ambiguous_tile() {
        // No permanent neighbor on either axis, so this is genuinely
        // ambiguous — the currently selected cell (the player having clicked
        // elsewhere to imply a direction) should win over the plain default.
        let game = test_game(empty_board());
        let staged = vec![letter_placement(112, 0, 'A')];

        // Selecting the cell to the right implies horizontal.
        assert_eq!(
            infer_typing_direction(&game, &staged, Some(113), None),
            DirectionDto::Horizontal
        );
        // Selecting the cell below implies vertical.
        assert_eq!(
            infer_typing_direction(&game, &staged, Some(112 + BOARD_WIDTH), None),
            DirectionDto::Vertical
        );
        // Selecting the tile's own cell isn't a signal either way.
        assert_eq!(
            infer_typing_direction(&game, &staged, Some(112), None),
            DirectionDto::Horizontal
        );
    }

    #[test]
    fn infer_typing_direction_selected_cell_beats_a_stale_direction_override() {
        let game = test_game(empty_board());
        let staged = vec![letter_placement(112, 0, 'A')];

        // An override says Vertical, but the player has since clicked a cell
        // that unambiguously implies Horizontal — the explicit click wins.
        assert_eq!(
            infer_typing_direction(&game, &staged, Some(113), Some(DirectionDto::Vertical)),
            DirectionDto::Horizontal
        );
    }

    #[test]
    fn infer_typing_direction_falls_back_to_the_override_when_selection_is_ambiguous() {
        let game = test_game(empty_board());
        let staged = vec![letter_placement(112, 0, 'A')];

        // No selected cell at all falls back to the override.
        assert_eq!(
            infer_typing_direction(&game, &staged, None, Some(DirectionDto::Vertical)),
            DirectionDto::Vertical
        );
        // A diagonally-selected cell shares neither axis, so it's not a
        // signal either — falls back to the override too.
        assert_eq!(
            infer_typing_direction(
                &game,
                &staged,
                Some(112 + BOARD_WIDTH + 1),
                Some(DirectionDto::Vertical)
            ),
            DirectionDto::Vertical
        );
    }

    #[test]
    fn aligned_direction_reads_the_axis_two_cells_share() {
        assert_eq!(aligned_direction(112, 113), Some(DirectionDto::Horizontal));
        assert_eq!(
            aligned_direction(112, 112 + BOARD_WIDTH),
            Some(DirectionDto::Vertical)
        );
        assert_eq!(aligned_direction(112, 112), None);
        assert_eq!(aligned_direction(112, 112 + BOARD_WIDTH + 1), None);
    }

    #[test]
    fn stage_tile_at_cell_keeps_letter_tiles_unchanged() {
        let tile = RackTileView {
            id: 5,
            display: "Q".to_string(),
            tile: TileDto::Letter {
                letter: "Q".to_string(),
            },
            is_used: false,
        };
        let placement = stage_tile_at_cell(42, &tile, None);
        assert_eq!(placement.board_index, 42);
        assert_eq!(placement.rack_tile_id, 5);
        assert_eq!(placement.display, "Q");
        assert_eq!(
            placement.tile,
            TileDto::Letter {
                letter: "Q".to_string()
            }
        );
    }

    #[test]
    fn stage_tile_at_cell_resolves_a_typed_blank_and_lowercases_its_display() {
        let tile = RackTileView {
            id: 6,
            display: "*".to_string(),
            tile: TileDto::Blank { acting_as: None },
            is_used: false,
        };
        let placement = stage_tile_at_cell(7, &tile, Some("Z".to_string()));
        assert_eq!(placement.display, "z");
        assert_eq!(
            placement.tile,
            TileDto::Blank {
                acting_as: Some("Z".to_string())
            }
        );
    }

    /// A digraph tile's assigned blank (e.g. Spanish's RR) lowercases as
    /// a whole grapheme, not truncated to one character.
    #[test]
    fn stage_tile_at_cell_lowercases_a_digraph_blank_assignment() {
        let tile = RackTileView {
            id: 6,
            display: "*".to_string(),
            tile: TileDto::Blank { acting_as: None },
            is_used: false,
        };
        let placement = stage_tile_at_cell(7, &tile, Some("RR".to_string()));
        assert_eq!(placement.display, "rr");
        assert_eq!(
            placement.tile,
            TileDto::Blank {
                acting_as: Some("RR".to_string())
            }
        );
    }

    #[test]
    fn stage_tile_at_cell_leaves_an_unresolved_blank_for_the_mouse_path() {
        let tile = RackTileView {
            id: 6,
            display: "*".to_string(),
            tile: TileDto::Blank { acting_as: None },
            is_used: false,
        };
        let placement = stage_tile_at_cell(7, &tile, None);
        assert_eq!(placement.display, "?");
        assert_eq!(placement.tile, TileDto::Blank { acting_as: None });
    }

    fn participant(seat_number: u8, player_id: Option<&str>) -> ParticipantDto {
        ParticipantDto {
            seat_number,
            kind: SeatKind::Human,
            display_name: format!("Seat {seat_number}"),
            player_id: player_id.map(str::to_string),
            engine_id: None,
            score: 0,
        }
    }

    fn game_with_participants(participants: Vec<ParticipantDto>, current_seat: u8) -> GameStateDto {
        let racks = participants
            .iter()
            .map(|_| RackDto {
                counts: Vec::new(),
                blanks: 0,
            })
            .collect();
        GameStateDto {
            participants,
            racks,
            current_seat,
            ..test_game(empty_board())
        }
    }

    #[test]
    fn seat_is_open_or_owned_by_lets_anyone_use_an_unclaimed_seat() {
        let unclaimed = participant(0, None);
        assert!(seat_is_open_or_owned_by(&unclaimed, None));
        assert!(seat_is_open_or_owned_by(&unclaimed, Some("alice")));
        assert!(seat_is_open_or_owned_by(&unclaimed, Some("mallory")));
    }

    #[test]
    fn seat_is_open_or_owned_by_restricts_a_claimed_seat_to_its_owner() {
        let claimed = participant(0, Some("alice"));
        assert!(seat_is_open_or_owned_by(&claimed, Some("alice")));
        assert!(!seat_is_open_or_owned_by(&claimed, Some("mallory")));
        assert!(!seat_is_open_or_owned_by(&claimed, None));
    }

    #[test]
    fn viewer_rack_seat_shows_your_own_seat_even_when_its_not_your_turn() {
        // Alice owns seat 0, Bob owns seat 1, it's Bob's turn — Alice
        // should still see her own rack while she waits.
        let game = game_with_participants(
            vec![participant(0, Some("alice")), participant(1, Some("bob"))],
            1,
        );
        assert_eq!(viewer_rack_seat(&game, Some("alice")), Some(0));
        assert_eq!(viewer_rack_seat(&game, Some("bob")), Some(1));
    }

    #[test]
    fn viewer_rack_seat_hides_a_claimed_game_from_a_non_participant() {
        let game = game_with_participants(
            vec![participant(0, Some("alice")), participant(1, Some("bob"))],
            0,
        );
        assert_eq!(viewer_rack_seat(&game, Some("mallory")), None);
        assert_eq!(viewer_rack_seat(&game, None), None);
    }

    #[test]
    fn viewer_rack_seat_stays_open_for_an_anonymous_game() {
        // Neither seat is claimed — matches today's anonymous-play
        // behavior, unaffected by ownership gating.
        let game = game_with_participants(vec![participant(0, None), participant(1, None)], 0);
        assert_eq!(viewer_rack_seat(&game, None), Some(0));
        assert_eq!(viewer_rack_seat(&game, Some("anyone")), Some(0));
    }
}
