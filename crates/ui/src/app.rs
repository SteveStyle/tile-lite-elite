use api::{
    BoardCellDto, CreateGameRequest, CreateSeatRequest, DirectionDto, GameActionRequest,
    GameEventDto, GameStateDto, GameStatus, MoveCandidateDto, ParticipantDto, PositionDto,
    PremiumDto, RackDto, SeatKind, StartGameRequest, TileDto, TilePlacementDto,
};
use dioxus::prelude::*;
use futures_util::StreamExt;
use rules_shared::{
    BoardCell, BoardState, Direction, EmptyCell, FilledCell, GameState, Letter, MoveGenerator,
    Rack, RulesEngine, SOWPODS, Tile, VariantRules,
};

#[cfg(target_arch = "wasm32")]
use gloo_net::{
    http::Request,
    websocket::{Message as WsMessage, futures::WebSocket},
};

#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::connect_async;

use crate::views::Home;

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const DEFAULT_ENGINE_ID: &str = "greedy-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RackTileView {
    pub id: usize,
    pub display: char,
    pub tile: TileDto,
    pub is_used: bool,
}

impl RackTileView {
    pub fn is_blank(&self) -> bool {
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
}

#[component]
pub fn RootApp() -> Element {
    let server_url = option_env!("SCRABBLE_PX_API_BASE_URL")
        .unwrap_or("http://127.0.0.1:3000")
        .to_string();
    let mut game = use_signal(|| None::<GameStateDto>);
    let mut is_loading = use_signal(|| false);
    let mut info_message = use_signal(|| Some("Loading latest game from server...".to_string()));
    let mut error_message = use_signal(|| None::<String>);
    let mut bootstrapped = use_signal(|| false);
    let mut websocket_game_id = use_signal(|| None::<String>);
    let mut selected_rack_tile_id = use_signal(|| None::<usize>);
    let mut staged_placements = use_signal(Vec::<StagedPlacementView>::new);
    let mut placement_direction = use_signal(|| DirectionDto::Horizontal);
    let mut selected_blank_letter = use_signal(|| None::<char>);

    if !bootstrapped() {
        bootstrapped.set(true);
        let server_url = server_url.clone();
        spawn(async move {
            is_loading.set(true);
            error_message.set(None);
            match load_latest_game(&server_url).await {
                Ok(Some(loaded)) => {
                    info_message.set(Some(format!("Loaded game {}", loaded.id)));
                    selected_rack_tile_id.set(None);
                    selected_blank_letter.set(None);
                    staged_placements.set(Vec::new());
                    game.set(Some(loaded));
                }
                Ok(None) => {
                    info_message.set(Some(
                        "No server game loaded yet. Create one to begin.".to_string(),
                    ));
                }
                Err(error) => {
                    error_message.set(Some(error));
                }
            }
            is_loading.set(false);
        });
    }

    if let Some(current_game) = game() {
        if websocket_game_id().as_deref() != Some(current_game.id.as_str()) {
            let game_id = current_game.id.clone();
            websocket_game_id.set(Some(game_id.clone()));
            let server_url = server_url.clone();
            spawn(async move {
                if let Err(error) = subscribe_to_game_events(&server_url, &game_id, game).await {
                    error_message.set(Some(error));
                }
            });
        }
    }

    let game_for_view = game().clone().unwrap_or_else(empty_live_game);
    let can_start = game()
        .as_ref()
        .is_some_and(|current| current.status == GameStatus::Waiting);
    let can_submit_human_action = game().as_ref().is_some_and(|current| {
        current.status == GameStatus::Active
            && current
                .participants
                .get(current.current_seat as usize)
                .is_some_and(|participant| participant.kind == SeatKind::Human)
    });
    let rack_tiles = current_rack_tiles(&game_for_view, &staged_placements());
    let selected_rack_tile = selected_rack_tile_id()
        .and_then(|id| rack_tiles.iter().find(|tile| tile.id == id).cloned());
    let can_submit_manual_action = can_submit_human_action && !staged_placements().is_empty();
    let staged_preview = preview_staged_move(
        &game_for_view,
        &staged_placements(),
        placement_direction(),
        can_submit_human_action,
    );
    let server_url_for_create = server_url.clone();
    let server_url_for_reload = server_url.clone();
    let server_url_for_start = server_url.clone();
    let server_url_for_suggested = server_url.clone();
    let server_url_for_pass = server_url.clone();
    let server_url_for_manual = server_url.clone();
    let server_url_for_view = server_url.clone();
    let game_for_home = game_for_view.clone();
    let game_for_inferred_direction = game_for_view.clone();
    let game_for_board_click = game_for_view.clone();
    let game_for_rack_click = game_for_view.clone();

    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }

        div { class: "app-shell",
            header { class: "topbar",
                div {
                    p { class: "topbar-kicker", "Scrabble PX" }
                    h1 { class: "topbar-title", "Thin client surface for the authoritative server" }
                }
                div { class: "topbar-actions",
                    button {
                        class: "toggle-button",
                        disabled: is_loading(),
                        onclick: move |_| {
                            let server_url = server_url_for_create.clone();
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match create_default_game(&server_url).await {
                                    Ok(created) => {
                                        info_message.set(Some(format!("Created game {}", created.id)));
                                        selected_rack_tile_id.set(None);
                                        selected_blank_letter.set(None);
                                        staged_placements.set(Vec::new());
                                        websocket_game_id.set(None);
                                        game.set(Some(created));
                                    }
                                    Err(error) => error_message.set(Some(error)),
                                }
                                is_loading.set(false);
                            });
                        },
                        "New Human vs Engine"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        disabled: is_loading(),
                        onclick: move |_| {
                            let server_url = server_url_for_reload.clone();
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match load_latest_game(&server_url).await {
                                    Ok(Some(loaded)) => {
                                        info_message.set(Some(format!("Reloaded game {}", loaded.id)));
                                        selected_rack_tile_id.set(None);
                                        selected_blank_letter.set(None);
                                        staged_placements.set(Vec::new());
                                        websocket_game_id.set(None);
                                        game.set(Some(loaded));
                                    }
                                    Ok(None) => {
                                        info_message.set(Some("No games found on the server.".to_string()));
                                        game.set(None);
                                    }
                                    Err(error) => error_message.set(Some(error)),
                                }
                                is_loading.set(false);
                            });
                        },
                        "Reload"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        disabled: is_loading() || !can_start,
                        onclick: move |_| {
                            let server_url = server_url_for_start.clone();
                            let current_game = game().clone();
                            if let Some(current_game) = current_game {
                                spawn(async move {
                                    is_loading.set(true);
                                    error_message.set(None);
                                    match start_game(&server_url, &current_game.id).await {
                                        Ok(updated) => {
                                            info_message.set(Some(format!("Started game {}", updated.id)));
                                            selected_rack_tile_id.set(None);
                                            selected_blank_letter.set(None);
                                            staged_placements.set(Vec::new());
                                            game.set(Some(updated));
                                        }
                                        Err(error) => error_message.set(Some(error)),
                                    }
                                    is_loading.set(false);
                                });
                            }
                        },
                        "Start"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        disabled: is_loading() || !can_submit_human_action,
                        onclick: move |_| {
                            let server_url = server_url_for_suggested.clone();
                            let current_game = game().clone();
                            if let Some(current_game) = current_game {
                                spawn(async move {
                                    is_loading.set(true);
                                    error_message.set(None);
                                    match submit_suggested_move(&server_url, &current_game).await {
                                        Ok(updated) => {
                                            info_message.set(Some("Submitted suggested move.".to_string()));
                                            selected_rack_tile_id.set(None);
                                            selected_blank_letter.set(None);
                                            staged_placements.set(Vec::new());
                                            game.set(Some(updated));
                                        }
                                        Err(error) => error_message.set(Some(error)),
                                    }
                                    is_loading.set(false);
                                });
                            }
                        },
                        "Play Suggested Move"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        disabled: is_loading() || !can_submit_human_action,
                        onclick: move |_| {
                            let server_url = server_url_for_pass.clone();
                            let current_game = game().clone();
                            if let Some(current_game) = current_game {
                                spawn(async move {
                                    is_loading.set(true);
                                    error_message.set(None);
                                    match submit_pass(&server_url, &current_game).await {
                                        Ok(updated) => {
                                            info_message.set(Some("Submitted pass action.".to_string()));
                                            selected_rack_tile_id.set(None);
                                            selected_blank_letter.set(None);
                                            staged_placements.set(Vec::new());
                                            game.set(Some(updated));
                                        }
                                        Err(error) => error_message.set(Some(error)),
                                    }
                                    is_loading.set(false);
                                });
                            }
                        },
                        "Pass"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        disabled: is_loading() || !can_submit_manual_action,
                        onclick: move |_| {
                            let server_url = server_url_for_manual.clone();
                            let current_game = game().clone();
                            let staged = staged_placements().clone();
                            let direction = placement_direction();
                            if let Some(current_game) = current_game {
                                spawn(async move {
                                    is_loading.set(true);
                                    error_message.set(None);
                                    match submit_manual_move(&server_url, &current_game, &staged, direction)
                                        .await
                                    {
                                        Ok(updated) => {
                                            info_message.set(Some("Submitted staged move.".to_string()));
                                            selected_rack_tile_id.set(None);
                                            selected_blank_letter.set(None);
                                            staged_placements.set(Vec::new());
                                            game.set(Some(updated));
                                        }
                                        Err(error) => error_message.set(Some(error)),
                                    }
                                    is_loading.set(false);
                                });
                            }
                        },
                        "Submit Staged Move"
                    }
                }
            }

            Home {
                game: game_for_home,
                server_url: server_url_for_view,
                is_live: game().is_some(),
                is_loading: is_loading(),
                info_message: info_message().clone(),
                error_message: error_message().clone(),
                rack_tiles,
                selected_rack_tile_id: selected_rack_tile_id(),
                staged_placements: staged_placements().clone(),
                can_stage_moves: can_submit_human_action,
                placement_direction: placement_direction(),
                inferred_direction: infer_current_direction(
                    &game_for_inferred_direction,
                    &staged_placements(),
                    placement_direction(),
                ),
                on_board_cell_click: move |board_index| {
                    if !can_submit_human_action {
                        error_message.set(Some("It is not a human-controlled turn.".to_string()));
                        return;
                    }

                    if game_for_board_click
                        .board
                        .get(board_index)
                        .is_some_and(|cell: &BoardCellDto| cell.letter.is_some())
                    {
                        return;
                    }

                    if staged_placements()
                        .iter()
                        .any(|placement| placement.board_index == board_index)
                    {
                        staged_placements
                            .with_mut(|placements| {
                                placements.retain(|placement| placement.board_index != board_index);
                            });
                        return;
                    }
                    let Some(selected_id) = selected_rack_tile_id() else {
                        info_message
                            .set(
                                Some(
                                    "Select a rack tile before placing it on the board.".to_string(),
                                ),
                            );
                        return;
                    };
                    let Some(tile) = current_rack_tiles(&game_for_board_click, &staged_placements())
                        .into_iter()
                        .find(|tile| tile.id == selected_id) else {
                        error_message
                            .set(Some("Selected rack tile is no longer available.".to_string()));
                        selected_rack_tile_id.set(None);
                        return;
                    };
                    let (tile_for_board, display_for_board) = match tile.tile.clone() {
                        TileDto::Blank { .. } => {
                            let Some(blank_letter) = selected_blank_letter() else {
                                info_message
                                    .set(
                                        Some(
                                            "Choose a letter for the selected blank tile before placing it."
                                                .to_string(),
                                        ),
                                    );

                                return;
                            };
                            (
                                TileDto::Blank {
                                    acting_as: Some(blank_letter),
                                },
                                blank_letter.to_ascii_lowercase(),
                            )
                        }
                        other => (other, tile.display),
                    };
                    staged_placements
                        .with_mut(|placements| {
                            placements
                                .push(StagedPlacementView {
                                    board_index,
                                    rack_tile_id: tile.id,
                                    display: display_for_board,
                                    tile: tile_for_board,
                                });
                        });
                    selected_rack_tile_id.set(None);
                    selected_blank_letter.set(None);
                },
                on_rack_tile_click: move |tile_id| {
                    if !can_submit_human_action {
                        return;
                    }

                    let Some(tile) = current_rack_tiles(&game_for_rack_click, &staged_placements())
                        .into_iter()
                        .find(|tile| tile.id == tile_id)
                        else {
                        return;
                    };

                    if tile.is_used {
                        return;
                    }

                    if selected_rack_tile_id() == Some(tile_id) {
                        selected_rack_tile_id.set(None);
                        selected_blank_letter.set(None);
                    } else {
                        selected_rack_tile_id.set(Some(tile_id));
                        if tile.is_blank() {
                            selected_blank_letter.set(None);
                            info_message
                                .set(
                                    Some(
                                        "Selected blank tile. Choose its letter, then place it on the board."
                                            .to_string(),
                                    ),
                                );
                        } else {
                            selected_blank_letter.set(None);
                            info_message.set(Some(format!("Selected tile {}", tile.display)));
                        }
                    }
                },
                on_clear_staged: move |_| {
                    selected_rack_tile_id.set(None);
                    selected_blank_letter.set(None);
                    staged_placements.set(Vec::new());
                    info_message.set(Some("Cleared staged placements.".to_string()));
                },
                on_remove_staged: move |board_index| {
                    staged_placements
                        .with_mut(|placements| {
                            placements.retain(|placement| placement.board_index != board_index);
                        });
                },
                on_set_horizontal: move |_| placement_direction.set(DirectionDto::Horizontal),
                on_set_vertical: move |_| placement_direction.set(DirectionDto::Vertical),
                on_set_blank_letter: move |letter| {
                    selected_blank_letter.set(Some(letter));
                    info_message.set(Some(format!("Blank tile will act as {}.", letter)));
                },
                selected_blank_letter: selected_blank_letter(),
                selected_rack_tile_is_blank: selected_rack_tile
                                                                                                                                                                                                                                                    .as_ref()
                                                                                                                                                                                                                                                    .is_some_and(RackTileView::is_blank),
                staged_preview,
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
        bag_count: 100,
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

async fn load_latest_game(server_url: &str) -> Result<Option<GameStateDto>, String> {
    let ids = get_json::<Vec<String>>(&format!("{server_url}/games")).await?;

    let Some(game_id) = ids.first() else {
        return Ok(None);
    };

    let game = get_json::<GameStateDto>(&format!("{server_url}/games/{game_id}")).await?;

    Ok(Some(game))
}

async fn create_default_game(server_url: &str) -> Result<GameStateDto, String> {
    let request = CreateGameRequest {
        seats: vec![
            CreateSeatRequest {
                kind: SeatKind::Human,
                display_name: "Alice".to_string(),
                engine_id: None,
                email: None,
                recovery_secret: None,
            },
            CreateSeatRequest {
                kind: SeatKind::Engine,
                display_name: "Greedy".to_string(),
                engine_id: Some(DEFAULT_ENGINE_ID.to_string()),
                email: None,
                recovery_secret: None,
            },
        ],
        seed: None,
        variant: None,
        language: None,
        board_layout: None,
    };

    post_json(&format!("{server_url}/games"), &request).await
}

async fn start_game(server_url: &str, game_id: &str) -> Result<GameStateDto, String> {
    post_json(
        &format!("{server_url}/games/{game_id}/start"),
        &StartGameRequest::default(),
    )
    .await
}

async fn submit_pass(server_url: &str, game: &GameStateDto) -> Result<GameStateDto, String> {
    let request = GameActionRequest {
        seat_number: game.current_seat,
        action: api::PlayerActionDto::Pass,
    };
    post_json(&format!("{server_url}/games/{}/actions", game.id), &request).await
}

async fn submit_suggested_move(
    server_url: &str,
    game: &GameStateDto,
) -> Result<GameStateDto, String> {
    let request = build_suggested_move_request(game)?;
    post_json(&format!("{server_url}/games/{}/actions", game.id), &request).await
}

async fn submit_manual_move(
    server_url: &str,
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    direction: DirectionDto,
) -> Result<GameStateDto, String> {
    let request = build_manual_move_request(game, staged, direction)?;
    post_json(&format!("{server_url}/games/{}/actions", game.id), &request).await
}

async fn post_json<T, R>(url: &str, payload: &T) -> Result<R, String>
where
    T: serde::Serialize + ?Sized,
    R: serde::de::DeserializeOwned,
{
    post_json_impl(url, payload).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn get_json<R>(url: &str) -> Result<R, String>
where
    R: serde::de::DeserializeOwned,
{
    reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json::<R>()
        .await
        .map_err(|error| error.to_string())
}

#[cfg(target_arch = "wasm32")]
async fn get_json<R>(url: &str) -> Result<R, String>
where
    R: serde::de::DeserializeOwned,
{
    let response = Request::get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "HTTP {} {}",
            response.status(),
            response.status_text()
        ));
    }
    response
        .json::<R>()
        .await
        .map_err(|error| error.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
async fn post_json_impl<T, R>(url: &str, payload: &T) -> Result<R, String>
where
    T: serde::Serialize + ?Sized,
    R: serde::de::DeserializeOwned,
{
    reqwest::Client::new()
        .post(url)
        .json(payload)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json::<R>()
        .await
        .map_err(|error| error.to_string())
}

#[cfg(target_arch = "wasm32")]
async fn post_json_impl<T, R>(url: &str, payload: &T) -> Result<R, String>
where
    T: serde::Serialize + ?Sized,
    R: serde::de::DeserializeOwned,
{
    let response = Request::post(url)
        .json(payload)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "HTTP {} {}",
            response.status(),
            response.status_text()
        ));
    }
    response
        .json::<R>()
        .await
        .map_err(|error| error.to_string())
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
    let (stream, _) = connect_async(ws_url)
        .await
        .map_err(|error| error.to_string())?;
    let (_, mut read) = stream.split();

    while let Some(message) = read.next().await {
        let message = message.map_err(|error| error.to_string())?;
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
    let mut read = WebSocket::open(&ws_url).map_err(|error| error.to_string())?;

    while let Some(message) = read.next().await {
        let message = message.map_err(|error| error.to_string())?;
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
    Err(format!("Unsupported server url: {server_url}"))
}

fn build_suggested_move_request(game: &GameStateDto) -> Result<GameActionRequest, String> {
    if game.status != GameStatus::Active {
        return Err("Game is not active".to_string());
    }

    let participant = game
        .participants
        .get(game.current_seat as usize)
        .ok_or_else(|| "Current seat is missing".to_string())?;
    if participant.kind != SeatKind::Human {
        return Err("Current seat is not human-controlled".to_string());
    }

    let rack = game
        .racks
        .get(game.current_seat as usize)
        .ok_or_else(|| "Current rack is missing".to_string())?;
    let rules = VariantRules::official();
    let board = board_from_dto(&game.board)?;
    let state = GameState::from_board(board, &rules, &*SOWPODS);
    let rack = rack_from_dto(rack);
    let engine = RulesEngine {
        rules: &rules,
        dictionary: &*SOWPODS,
    };

    let mut best_candidate = None;
    let mut best_score = i16::MIN;
    for candidate in engine.enumerate_legal_moves(&state, &rack) {
        if let Ok(validated) = engine.validate_game_move(&state, Some(&rack), &candidate) {
            if validated.score.total > best_score {
                best_score = validated.score.total;
                best_candidate = Some(candidate);
            }
        }
    }

    let action = match best_candidate {
        Some(candidate) => api::PlayerActionDto::Place {
            candidate: move_candidate_to_dto(candidate),
        },
        None => api::PlayerActionDto::Pass,
    };

    Ok(GameActionRequest {
        seat_number: game.current_seat,
        action,
    })
}

fn preview_staged_move(
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    direction_hint: DirectionDto,
    can_stage_moves: bool,
) -> Option<MovePreviewView> {
    if !can_stage_moves || staged.is_empty() {
        return None;
    }

    match build_manual_move_candidate(game, staged, direction_hint) {
        Ok(candidate) => {
            let rules = VariantRules::official();
            let board = match board_from_dto(&game.board) {
                Ok(board) => board,
                Err(error) => {
                    return Some(MovePreviewView {
                        is_legal: false,
                        headline: "Preview unavailable".to_string(),
                        detail: error,
                    });
                }
            };
            let rack = match game.racks.get(game.current_seat as usize) {
                Some(rack) => rack_from_dto(rack),
                None => {
                    return Some(MovePreviewView {
                        is_legal: false,
                        headline: "Preview unavailable".to_string(),
                        detail: "Current rack is missing.".to_string(),
                    });
                }
            };
            let state = GameState::from_board(board, &rules, &*SOWPODS);
            let engine = RulesEngine {
                rules: &rules,
                dictionary: &*SOWPODS,
            };

            match engine.validate_game_move(&state, Some(&rack), &candidate) {
                Ok(validated) => Some(MovePreviewView {
                    is_legal: true,
                    headline: format!(
                        "Legal preview: {} for {} points",
                        validated.preview.main_word, validated.score.total
                    ),
                    detail: if validated.preview.cross_words.is_empty() {
                        "No cross words created.".to_string()
                    } else {
                        format!(
                            "Cross words: {}",
                            validated
                                .preview
                                .cross_words
                                .iter()
                                .map(|word| word.word.clone())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    },
                }),
                Err(error) => Some(MovePreviewView {
                    is_legal: false,
                    headline: "Move is not currently legal".to_string(),
                    detail: format_move_error(&error),
                }),
            }
        }
        Err(error) => Some(MovePreviewView {
            is_legal: false,
            headline: "Move is not currently legal".to_string(),
            detail: error,
        }),
    }
}

fn infer_current_direction(
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    direction_hint: DirectionDto,
) -> DirectionDto {
    if staged.len() != 1 {
        return direction_hint;
    }

    infer_single_tile_direction(game, staged[0].board_index, direction_hint)
}

fn build_manual_move_request(
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    direction_hint: DirectionDto,
) -> Result<GameActionRequest, String> {
    let candidate = build_manual_move_candidate(game, staged, direction_hint)?;

    Ok(GameActionRequest {
        seat_number: game.current_seat,
        action: api::PlayerActionDto::Place {
            candidate: move_candidate_to_dto(candidate),
        },
    })
}

fn build_manual_move_candidate(
    game: &GameStateDto,
    staged: &[StagedPlacementView],
    direction_hint: DirectionDto,
) -> Result<rules_shared::MoveCandidate, String> {
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
        .map(|placement| {
            (
                (placement.board_index % BoardState::WIDTH) as u8,
                (placement.board_index / BoardState::WIDTH) as u8,
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

    let mut tiles = placements
        .into_iter()
        .map(|placement| {
            let x = (placement.board_index % BoardState::WIDTH) as u8;
            let y = (placement.board_index / BoardState::WIDTH) as u8;
            let offset = match direction {
                DirectionDto::Horizontal => x - start_x,
                DirectionDto::Vertical => y - start_y,
            };
            rules_shared::TilePlacement {
                offset,
                tile: tile_from_dto(&placement.tile),
            }
        })
        .collect::<Vec<_>>();
    tiles.sort_by_key(|placement| placement.offset);

    Ok(rules_shared::MoveCandidate {
        start: rules_shared::Position::new(start_x, start_y),
        direction: match direction {
            DirectionDto::Horizontal => Direction::Horizontal,
            DirectionDto::Vertical => Direction::Vertical,
        },
        tiles,
    })
}

fn infer_single_tile_direction(
    game: &GameStateDto,
    board_index: usize,
    direction_hint: DirectionDto,
) -> DirectionDto {
    let x = board_index % BoardState::WIDTH;
    let y = board_index / BoardState::WIDTH;

    let has_horizontal_neighbor = (x > 0
        && game
            .board
            .get(board_index - 1)
            .is_some_and(board_cell_has_letter))
        || (x + 1 < BoardState::WIDTH
            && game
                .board
                .get(board_index + 1)
                .is_some_and(board_cell_has_letter));
    let has_vertical_neighbor = (y > 0
        && game
            .board
            .get(board_index - BoardState::WIDTH)
            .is_some_and(board_cell_has_letter))
        || (y + 1 < BoardState::HEIGHT
            && game
                .board
                .get(board_index + BoardState::WIDTH)
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

fn current_rack_tiles(game: &GameStateDto, staged: &[StagedPlacementView]) -> Vec<RackTileView> {
    let Some(rack) = game.racks.get(game.current_seat as usize) else {
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

fn board_from_dto(cells: &[BoardCellDto]) -> Result<BoardState, String> {
    if cells.len() != BoardState::WIDTH * BoardState::HEIGHT {
        return Err(format!(
            "Expected {} board cells, got {}",
            BoardState::WIDTH * BoardState::HEIGHT,
            cells.len()
        ));
    }

    let mut board = BoardState::default();
    for (index, cell) in cells.iter().enumerate() {
        let x = (index % BoardState::WIDTH) as u8;
        let y = (index / BoardState::WIDTH) as u8;
        let pos = rules_shared::Position::new(x, y);
        let board_cell = match cell.letter {
            Some(letter) => BoardCell::Filled(FilledCell {
                letter: Letter::from(letter),
                is_blank: cell.is_blank,
            }),
            None => BoardCell::Empty(EmptyCell {
                premium: premium_from_dto(&cell.premium),
            }),
        };
        board.set(pos, board_cell);
    }

    Ok(board)
}

fn premium_from_dto(premium: &PremiumDto) -> rules_shared::Premium {
    match premium {
        PremiumDto::Blank => rules_shared::Premium::Blank,
        PremiumDto::DoubleLetter => rules_shared::Premium::DoubleLetter,
        PremiumDto::TripleLetter => rules_shared::Premium::TripleLetter,
        PremiumDto::DoubleWord => rules_shared::Premium::DoubleWord,
        PremiumDto::TripleWord => rules_shared::Premium::TripleWord,
    }
}

fn rack_from_dto(rack: &RackDto) -> Rack {
    Rack {
        counts: rack.counts,
        blanks: rack.blanks,
    }
}

fn move_candidate_to_dto(candidate: rules_shared::MoveCandidate) -> MoveCandidateDto {
    MoveCandidateDto {
        start: PositionDto {
            x: candidate.start.x,
            y: candidate.start.y,
        },
        direction: match candidate.direction {
            Direction::Horizontal => DirectionDto::Horizontal,
            Direction::Vertical => DirectionDto::Vertical,
        },
        tiles: candidate
            .tiles
            .into_iter()
            .map(|tile| TilePlacementDto {
                offset: tile.offset,
                tile: tile_to_dto(tile.tile),
            })
            .collect(),
    }
}

fn tile_from_dto(tile: &TileDto) -> Tile {
    match tile {
        TileDto::Letter { letter } => Tile::Letter(Letter::from(*letter)),
        TileDto::Blank { acting_as } => Tile::Blank {
            acting_as: acting_as.map(Letter::from),
        },
    }
}

fn format_move_error(error: &rules_shared::MoveError) -> String {
    match error {
        rules_shared::MoveError::InvalidMove => "Invalid move shape.".to_string(),
        rules_shared::MoveError::InvalidWord(word) => format!("Invalid word: {word}"),
        rules_shared::MoveError::InvalidPosition => "Tile placement is off the board.".to_string(),
        rules_shared::MoveError::InvalidDirection => "Invalid move direction.".to_string(),
        rules_shared::MoveError::TilesDoNotFit => {
            "The selected tiles do not fit the current rack or span.".to_string()
        }
        rules_shared::MoveError::TilesDoNotConnect => {
            "The move does not connect to the board correctly.".to_string()
        }
        rules_shared::MoveError::LetterNotAllowedInPosition => {
            "A staged tile is not allowed at one of the chosen squares.".to_string()
        }
    }
}

fn tile_to_dto(tile: Tile) -> TileDto {
    match tile {
        Tile::Letter(letter) => TileDto::Letter {
            letter: letter.as_char(),
        },
        Tile::Blank { acting_as } => TileDto::Blank {
            acting_as: acting_as.map(|letter| letter.as_char()),
        },
    }
}
