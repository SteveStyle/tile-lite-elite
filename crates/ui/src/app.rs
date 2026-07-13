use api::{
    BoardCellDto, CreateGameRequest, DirectionDto, GameActionRequest, GameEventDto, GameStateDto,
    GameStatus, MoveCandidateDto, ParticipantDto, PositionDto, PremiumDto, RackDto, SeatKind,
    StartGameRequest, TileDto, TilePlacementDto,
};
use dioxus::prelude::*;
use futures_util::StreamExt;
use std::collections::HashSet;

#[cfg(target_arch = "wasm32")]
use gloo_net::{
    http::Request,
    websocket::{Message as WsMessage, futures::WebSocket},
};

#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::connect_async;

use crate::components::auth_panel::AuthPanel;
use crate::components::games_panel::GamesPanel;
use crate::views::Home;

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
pub(crate) const BOARD_WIDTH: usize = 15;
const BOARD_HEIGHT: usize = 15;
/// How often the background reconnect loop pings `/health` while the
/// server is unreachable.
const RECONNECT_POLL_MS: u64 = 3000;
/// Delay between WebSocket reconnect attempts.
const WEBSOCKET_RETRY_MS: u64 = 3000;

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
    pub display: char,
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
    pub display: char,
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
    let server_url = option_env!("SCRABBLE_PX_API_BASE_URL")
        .map(str::to_string)
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());

    // Desktop has no browser origin to derive from, so it resolves from
    // `crate::config` instead: a compiled-in default environment, with a
    // `--server-url`/`--env` CLI override (see `main.rs` and `config.rs`).
    #[cfg(not(target_arch = "wasm32"))]
    let server_url = crate::config::server_url();
    let mut game = use_signal(|| None::<GameStateDto>);
    let mut game_summaries = use_signal(Vec::<api::GameSummaryDto>::new);
    let mut session = use_signal(|| None::<api::PlayerSessionDto>);
    let mut is_loading = use_signal(|| false);
    let mut info_message = use_signal(|| Some("Loading games from server...".to_string()));
    let mut error_message = use_signal(|| None::<String>);
    let mut bootstrapped = use_signal(|| false);
    let mut websocket_game_id = use_signal(|| None::<String>);
    let mut dragging_tile_id = use_signal(|| None::<usize>);
    let mut staged_placements = use_signal(Vec::<StagedPlacementView>::new);
    let mut selected_blank_letter = use_signal(|| None::<char>);
    let mut selected_cell = use_signal(|| None::<usize>);
    let mut exchange_mode = use_signal(|| false);
    let mut exchange_selected = use_signal(HashSet::<usize>::new);

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
            spawn(async move {
                // Keep retrying for as long as this is still the selected
                // game — a dropped connection (network blip, server
                // restart) shouldn't leave live updates dead for the rest
                // of the session. `subscribe_to_game_events` itself marks
                // `IS_ONLINE` on connect/disconnect (see `mark_online` /
                // `mark_offline`); this loop just keeps trying.
                while websocket_game_id().as_deref() == Some(game_id.as_str()) {
                    let _ = subscribe_to_game_events(&server_url, &game_id, game).await;
                    if websocket_game_id().as_deref() != Some(game_id.as_str()) {
                        break;
                    }
                    sleep_ms(WEBSOCKET_RETRY_MS).await;
                }
            });
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
    let rack_tiles = rack_tiles_for_seat(&game_for_view, viewer_rack_seat, &staged_placements());
    let can_submit_manual_action =
        can_submit_human_action && !exchange_mode() && !staged_placements().is_empty();

    let mut staged_preview: Signal<Option<MovePreviewView>> = use_signal(|| None);
    {
        let server_url_for_preview = server_url.clone();
        let game_for_direction = game_for_view.clone();
        use_effect(move || {
            let staged = staged_placements();
            let game_val = game();
            let direction = infer_typing_direction(&game_for_direction, &staged);
            let is_human_turn = can_submit_human_action;
            let server_url = server_url_for_preview.clone();
            let token = session().map(|current| current.session_token.clone());
            spawn(async move {
                if !is_human_turn || staged.is_empty() {
                    staged_preview.set(None);
                    return;
                }
                if let Some(game) = game_val {
                    let preview =
                        fetch_server_preview(&server_url, &game, &staged, direction, token.as_deref()).await;
                    staged_preview.set(preview);
                }
            });
        });
    }
    let staged_preview = staged_preview();
    let server_url_for_login = server_url.clone();
    let server_url_for_custom_create = server_url.clone();
    let server_url_for_accept = server_url.clone();
    let server_url_for_reject = server_url.clone();
    let server_url_for_refresh = server_url.clone();
    let server_url_for_select = server_url.clone();
    let server_url_for_start = server_url.clone();
    let server_url_for_exchange = server_url.clone();
    let server_url_for_pass = server_url.clone();
    let server_url_for_manual = server_url.clone();
    let game_for_home = game_for_view.clone();
    let game_for_drop = game_for_view.clone();
    let game_for_select = game_for_view.clone();
    let game_for_click = game_for_view.clone();
    let game_for_type = game_for_view.clone();
    let game_for_backspace = game_for_view.clone();

    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }

        div { class: "app-shell",
            header { class: "topbar",
                p { class: "topbar-kicker", "Scrabble PX" }
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
                    is_loading: is_loading(),
                    my_display_name: session().map(|current| current.display_name.clone()),
                    can_start,
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
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            match load_game_by_id(&server_url, &game_id).await {
                                Ok(loaded) => {
                                    info_message.set(None);
                                    reset_composer_state(
                                        dragging_tile_id,
                                        selected_blank_letter,
                                        staged_placements,
                                        selected_cell,
                                        exchange_mode,
                                        exchange_selected,
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
                            match create_custom_game(&server_url, token.as_deref(), &submission).await {
                                Ok(created) => {
                                    info_message.set(None);
                                    reset_composer_state(
                                        dragging_tile_id,
                                        selected_blank_letter,
                                        staged_placements,
                                        selected_cell,
                                        exchange_mode,
                                        exchange_selected,
                                    );
                                    websocket_game_id.set(None);
                                    game.set(Some(created));
                                    if let Ok(summaries) = load_game_summaries(&server_url, token.as_deref()).await {
                                        game_summaries.set(summaries);
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
                    can_view_rack,
                    staged_placements: staged_placements().clone(),
                    can_stage_moves: can_submit_human_action && !exchange_mode(),
                    selected_cell: selected_cell(),
                    on_drag_rack_tile: move |tile_id| {
                        dragging_tile_id.set(Some(tile_id));
                    },
                    on_drag_end_rack_tile: move |_| {
                        dragging_tile_id.set(None);
                    },
                    on_drop_board_cell: move |board_index| {
                        if !can_submit_human_action || exchange_mode() {
                            return;
                        }
                        if game_for_drop
                            .board
                            .get(board_index)
                            .is_some_and(|cell: &BoardCellDto| cell.letter.is_some())
                        {
                            return;
                        }
                        if staged_placements()
                            .iter()
                            .any(|p| p.board_index == board_index)
                        {
                            return;
                        }
                        let Some(tile_id) = dragging_tile_id() else {
                            return;
                        };
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
                        let placement = stage_tile_at_cell(cell_index, &tile, None);
                        staged_placements
                            .with_mut(|placements| placements.push(placement));
                        advance_selection(&game_for_click, staged_placements, selected_cell, cell_index);
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
                        let chosen = rack
                            .iter()
                            .find(|t| {
                                !t.is_used
                                    && matches!(&t.tile, TileDto::Letter { letter: l } if *l == letter)
                            })
                            .or_else(|| {
                                rack.iter()
                                    .find(|t| !t.is_used && matches!(t.tile, TileDto::Blank { .. }))
                            });
                        let Some(tile) = chosen else {
                            return;
                        };
                        let resolved = matches!(tile.tile, TileDto::Blank { .. }).then_some(letter);
                        let placement = stage_tile_at_cell(cell_index, tile, resolved);
                        staged_placements
                            .with_mut(|placements| placements.push(placement));
                        advance_selection(&game_for_type, staged_placements, selected_cell, cell_index);
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
                            let direction = infer_typing_direction(&game_for_backspace, &staged_placements());
                            find_previous_editable_cell(&game_for_backspace, cell_index, direction)
                        };
                        let Some(target) = target else {
                            return;
                        };
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
                        info_message.set(Some("Cleared staged placements.".to_string()));
                    },
                    on_remove_staged: move |board_index| {
                        staged_placements
                            .with_mut(|placements| {
                                placements.retain(|p| p.board_index != board_index);
                            });
                    },
                    on_set_blank_letter: move |letter| {
                        selected_blank_letter.set(Some(letter));
                        staged_placements
                            .with_mut(|placements| {
                                if let Some(placement) = placements
                                    .iter_mut()
                                    .find(|p| matches!(p.tile, TileDto::Blank { acting_as: None }))
                                {
                                    placement.tile = TileDto::Blank {
                                        acting_as: Some(letter),
                                    };
                                    placement.display = letter.to_ascii_lowercase();
                                }
                            });
                    },
                    selected_blank_letter: selected_blank_letter(),
                    staged_preview,
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
                                        info_message.set(Some("Submitted pass action.".to_string()));
                                        reset_composer_state(
                                            dragging_tile_id,
                                            selected_blank_letter,
                                            staged_placements,
                                            selected_cell,
                                            exchange_mode,
                                            exchange_selected,
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
                            let direction = infer_typing_direction(&current_game, &staged);
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
                                        info_message.set(Some("Played a move.".to_string()));
                                        reset_composer_state(
                                            dragging_tile_id,
                                            selected_blank_letter,
                                            staged_placements,
                                            selected_cell,
                                            exchange_mode,
                                            exchange_selected,
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
                                        info_message.set(Some("Exchanged tiles.".to_string()));
                                        reset_composer_state(
                                            dragging_tile_id,
                                            selected_blank_letter,
                                            staged_placements,
                                            selected_cell,
                                            exchange_mode,
                                            exchange_selected,
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
            counts: [0; 26],
            blanks: 0,
        }],
        moves: vec![],
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

async fn validate_session(server_url: &str, session_token: &str) -> Result<api::PlayerDto, String> {
    let request = api::ValidateSessionRequest {
        session_token: session_token.to_string(),
    };
    post_json(&format!("{server_url}/auth/validate"), None, &request).await
}

async fn load_game_by_id(server_url: &str, game_id: &str) -> Result<GameStateDto, String> {
    get_json::<GameStateDto>(&format!("{server_url}/games/{game_id}")).await
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
    selected_blank_letter: Signal<Option<char>>,
    staged_placements: Signal<Vec<StagedPlacementView>>,
    selected_cell: Signal<Option<usize>>,
    exchange_mode: Signal<bool>,
    exchange_selected: Signal<HashSet<usize>>,
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
                Some(game_id) => match load_game_by_id(server_url, &game_id).await {
                    Ok(loaded) => {
                        info_message.set(None);
                        reset_composer_state(
                            dragging_tile_id,
                            selected_blank_letter,
                            staged_placements,
                            selected_cell,
                            exchange_mode,
                            exchange_selected,
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
        variant: None,
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

async fn start_game(server_url: &str, game_id: &str, token: Option<&str>) -> Result<GameStateDto, String> {
    post_json(
        &format!("{server_url}/games/{game_id}/start"),
        token,
        &StartGameRequest::default(),
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
    game_signal: Signal<Option<GameStateDto>>,
) -> Result<(), String> {
    subscribe_to_game_events_impl(server_url, game_id, game_signal).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn subscribe_to_game_events_impl(
    server_url: &str,
    game_id: &str,
    mut game_signal: Signal<Option<GameStateDto>>,
) -> Result<(), String> {
    let ws_url = websocket_url(server_url, game_id)?;
    let (stream, _) = connect_async(ws_url).await.map_err(|_| mark_offline())?;
    mark_online();
    let (_, mut read) = stream.split();

    while let Some(message) = read.next().await {
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
    mut game_signal: Signal<Option<GameStateDto>>,
) -> Result<(), String> {
    let ws_url = websocket_url(server_url, game_id)?;
    let mut read = WebSocket::open(&ws_url).map_err(|_| mark_offline())?;
    mark_online();

    while let Some(message) = read.next().await {
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

fn websocket_url(server_url: &str, game_id: &str) -> Result<String, String> {
    if let Some(url) = server_url.strip_prefix("http://") {
        return Ok(format!("ws://{url}/games/{game_id}/events"));
    }
    if let Some(url) = server_url.strip_prefix("https://") {
        return Ok(format!("wss://{url}/games/{game_id}/events"));
    }
    // An empty `server_url` means "same origin as the page" (see
    // `default_server_url` — used when a reverse proxy serves both the
    // static assets and the API from one host, e.g. the container
    // deployment). There's no explicit scheme/host to rewrite in that case,
    // so it's derived from the browser's own location instead.
    if server_url.is_empty() {
        return same_origin_websocket_url(game_id);
    }
    Err(format!("Unsupported server url: {server_url}"))
}

#[cfg(target_arch = "wasm32")]
fn same_origin_websocket_url(game_id: &str) -> Result<String, String> {
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
    Ok(format!("{ws_scheme}://{host}/games/{game_id}/events"))
}

/// The desktop build never runs with a same-origin (empty) `server_url` — it
/// always talks to an explicit configured server — so this is unreachable
/// in practice; it exists only so `websocket_url` compiles for both targets.
#[cfg(not(target_arch = "wasm32"))]
fn same_origin_websocket_url(_game_id: &str) -> Result<String, String> {
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

async fn fetch_server_preview(
    server_url: &str,
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    direction_hint: DirectionDto,
    token: Option<&str>,
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
    let candidate = match request.action {
        api::PlayerActionDto::Place { candidate } => candidate,
        _ => return None,
    };
    let preview_request = api::PreviewMoveRequest {
        seat_number: request.seat_number,
        candidate,
    };
    match post_json::<_, api::PreviewMoveResponse>(
        &format!("{server_url}/games/{}/preview", game.id),
        token,
        &preview_request,
    )
    .await
    {
        Ok(response) => Some(MovePreviewView {
            is_legal: response.is_legal,
            headline: response.headline,
            detail: response.detail,
            score: response.score,
        }),
        Err(_) => None,
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
fn infer_typing_direction(game: &GameStateDto, staged: &[StagedPlacementView]) -> DirectionDto {
    match staged.len() {
        0 => DirectionDto::Horizontal,
        1 => infer_single_tile_direction(game, staged[0].board_index, DirectionDto::Horizontal),
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
/// in. Clears the selection at the edge of the board rather than wrapping.
fn advance_selection(
    game: &GameStateDto,
    staged_placements: Signal<Vec<StagedPlacementView>>,
    mut selected_cell: Signal<Option<usize>>,
    from_index: usize,
) {
    let staged = staged_placements();
    let direction = infer_typing_direction(game, &staged);
    selected_cell.set(find_next_placeable_cell(game, &staged, from_index, direction, true));
}

/// Builds the staged placement for dropping/clicking/typing `tile` onto
/// `board_index`. `resolved_letter` is `Some` only when a blank is being
/// auto-assigned a letter because the player typed it directly (keyboard
/// path) — the mouse path still leaves blanks unresolved for the
/// blank-letter picker, same as before.
fn stage_tile_at_cell(
    board_index: usize,
    tile: &RackTileView,
    resolved_letter: Option<char>,
) -> StagedPlacementView {
    let (tile_for_board, display_for_board) = match (&tile.tile, resolved_letter) {
        (TileDto::Blank { .. }, Some(letter)) => (
            TileDto::Blank {
                acting_as: Some(letter),
            },
            letter.to_ascii_lowercase(),
        ),
        (TileDto::Blank { .. }, None) => (TileDto::Blank { acting_as: None }, '?'),
        (other, _) => (other.clone(), tile.display),
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
    mut selected_blank_letter: Signal<Option<char>>,
    mut staged_placements: Signal<Vec<StagedPlacementView>>,
    mut selected_cell: Signal<Option<usize>>,
    mut exchange_mode: Signal<bool>,
    mut exchange_selected: Signal<HashSet<usize>>,
) {
    dragging_tile_id.set(None);
    selected_blank_letter.set(None);
    staged_placements.set(Vec::new());
    selected_cell.set(None);
    exchange_mode.set(false);
    exchange_selected.set(HashSet::new());
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

    let used_ids: std::collections::HashSet<usize> = staged
        .iter()
        .map(|placement| placement.rack_tile_id)
        .collect();
    let mut next_id = 0usize;
    let mut tiles = Vec::new();

    for (index, count) in rack.counts.iter().enumerate() {
        for _ in 0..*count {
            tiles.push(RackTileView {
                id: next_id,
                display: (b'A' + index as u8) as char,
                tile: TileDto::Letter {
                    letter: (b'A' + index as u8) as char,
                },
                is_used: used_ids.contains(&next_id),
            });
            next_id += 1;
        }
    }

    for _ in 0..rack.blanks {
        tiles.push(RackTileView {
            id: next_id,
            display: '*',
            tile: TileDto::Blank { acting_as: None },
            is_used: used_ids.contains(&next_id),
        });
        next_id += 1;
    }

    tiles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_url_rewrites_http_and_https_schemes() {
        assert_eq!(
            websocket_url("http://example.com:3000", "game-1"),
            Ok("ws://example.com:3000/games/game-1/events".to_string())
        );
        assert_eq!(
            websocket_url("https://example.com", "game-1"),
            Ok("wss://example.com/games/game-1/events".to_string())
        );
    }

    #[test]
    fn websocket_url_rejects_an_unrecognized_scheme() {
        assert!(websocket_url("ftp://example.com", "game-1").is_err());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn websocket_url_same_origin_is_only_supported_on_web() {
        // The desktop/native build never actually passes an empty
        // server_url (it always talks to an explicit configured server),
        // so this just confirms the fallback doesn't panic — same-origin
        // resolution itself is only meaningful in a browser.
        assert!(websocket_url("", "game-1").is_err());
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
            display: letter,
            tile: TileDto::Letter { letter },
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
        board[12].letter = Some('A');
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
        board[11].letter = Some('A');
        board[10].letter = Some('B');
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
        assert_eq!(infer_typing_direction(&game, &[]), DirectionDto::Horizontal);

        let staged = vec![letter_placement(112, 0, 'A')];
        assert_eq!(infer_typing_direction(&game, &staged), DirectionDto::Horizontal);
    }

    #[test]
    fn infer_typing_direction_follows_the_existing_neighbor_for_a_single_tile() {
        let staged = vec![letter_placement(112, 0, 'A')];

        let mut board_with_left_neighbor = empty_board();
        board_with_left_neighbor[111].letter = Some('C');
        let game = test_game(board_with_left_neighbor);
        assert_eq!(infer_typing_direction(&game, &staged), DirectionDto::Horizontal);

        let mut board_with_top_neighbor = empty_board();
        board_with_top_neighbor[112 - BOARD_WIDTH].letter = Some('C');
        let game = test_game(board_with_top_neighbor);
        assert_eq!(infer_typing_direction(&game, &staged), DirectionDto::Vertical);
    }

    #[test]
    fn infer_typing_direction_follows_multi_tile_alignment() {
        let game = test_game(empty_board());

        let same_row = vec![letter_placement(100, 0, 'A'), letter_placement(102, 1, 'B')];
        assert_eq!(infer_typing_direction(&game, &same_row), DirectionDto::Horizontal);

        let same_column = vec![
            letter_placement(100, 0, 'A'),
            letter_placement(100 + BOARD_WIDTH * 2, 1, 'B'),
        ];
        assert_eq!(infer_typing_direction(&game, &same_column), DirectionDto::Vertical);
    }

    #[test]
    fn stage_tile_at_cell_keeps_letter_tiles_unchanged() {
        let tile = RackTileView {
            id: 5,
            display: 'Q',
            tile: TileDto::Letter { letter: 'Q' },
            is_used: false,
        };
        let placement = stage_tile_at_cell(42, &tile, None);
        assert_eq!(placement.board_index, 42);
        assert_eq!(placement.rack_tile_id, 5);
        assert_eq!(placement.display, 'Q');
        assert_eq!(placement.tile, TileDto::Letter { letter: 'Q' });
    }

    #[test]
    fn stage_tile_at_cell_resolves_a_typed_blank_and_lowercases_its_display() {
        let tile = RackTileView {
            id: 6,
            display: '*',
            tile: TileDto::Blank { acting_as: None },
            is_used: false,
        };
        let placement = stage_tile_at_cell(7, &tile, Some('Z'));
        assert_eq!(placement.display, 'z');
        assert_eq!(placement.tile, TileDto::Blank { acting_as: Some('Z') });
    }

    #[test]
    fn stage_tile_at_cell_leaves_an_unresolved_blank_for_the_mouse_path() {
        let tile = RackTileView {
            id: 6,
            display: '*',
            tile: TileDto::Blank { acting_as: None },
            is_used: false,
        };
        let placement = stage_tile_at_cell(7, &tile, None);
        assert_eq!(placement.display, '?');
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
        let racks = participants.iter().map(|_| RackDto { counts: [0; 26], blanks: 0 }).collect();
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
