use api::{
    BoardCellDto, CreateGameRequest, CreateSeatRequest, DirectionDto, GameActionRequest,
    GameEventDto, GameStateDto, GameStatus, MoveCandidateDto, ParticipantDto, PositionDto,
    PremiumDto, RackDto, SeatKind, StartGameRequest, TileDto, TilePlacementDto,
};
use dioxus::prelude::*;
use futures_util::StreamExt;

#[cfg(target_arch = "wasm32")]
use gloo_net::{
    http::Request,
    websocket::{Message as WsMessage, futures::WebSocket},
};

#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::connect_async;

use crate::components::auth_panel::AuthPanel;
use crate::components::games_panel::GamesPanel;
use crate::components::sidebar::Sidebar;
use crate::views::Home;

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const DEFAULT_ENGINE_ID: &str = "greedy-v1";
const BOARD_WIDTH: usize = 15;
const BOARD_HEIGHT: usize = 15;

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
}

#[component]
pub fn RootApp() -> Element {
    let server_url = option_env!("SCRABBLE_PX_API_BASE_URL")
        .unwrap_or("http://127.0.0.1:3000")
        .to_string();
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

    if !bootstrapped() {
        bootstrapped.set(true);
        let server_url = server_url.clone();
        spawn(async move {
            let stored = crate::local_storage::load();
            if let Some(token) = stored.session_token.clone() {
                match validate_session(&server_url, &token).await {
                    Ok(player) => {
                        session.set(Some(api::PlayerSessionDto {
                            player_id: player.id,
                            session_token: token,
                            display_name: player.display_name,
                            email: player.email,
                        }));
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
            match load_game_summaries(&server_url).await {
                Ok(summaries) => {
                    let most_recent_id = summaries.first().map(|summary| summary.id.clone());
                    game_summaries.set(summaries);
                    match most_recent_id {
                        Some(game_id) => match load_game_by_id(&server_url, &game_id).await {
                            Ok(loaded) => {
                                info_message.set(None);
                                dragging_tile_id.set(None);
                                selected_blank_letter.set(None);
                                staged_placements.set(Vec::new());
                                game.set(Some(loaded));
                            }
                            Err(error) => error_message.set(Some(error)),
                        },
                        None => {
                            info_message.set(Some(
                                "No games yet. Create one to begin.".to_string(),
                            ));
                        }
                    }
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
    let can_submit_manual_action = can_submit_human_action && !staged_placements().is_empty();

    let mut staged_preview: Signal<Option<MovePreviewView>> = use_signal(|| None);
    {
        let server_url_for_preview = server_url.clone();
        use_effect(move || {
            let staged = staged_placements();
            let game_val = game();
            let direction = DirectionDto::Horizontal;
            let is_human_turn = can_submit_human_action;
            let server_url = server_url_for_preview.clone();
            spawn(async move {
                if !is_human_turn || staged.is_empty() {
                    staged_preview.set(None);
                    return;
                }
                if let Some(game) = game_val {
                    let preview =
                        fetch_server_preview(&server_url, &game, &staged, direction).await;
                    staged_preview.set(preview);
                }
            });
        });
    }
    let staged_preview = staged_preview();
    let server_url_for_create = server_url.clone();
    let server_url_for_refresh = server_url.clone();
    let server_url_for_select = server_url.clone();
    let server_url_for_start = server_url.clone();
    let server_url_for_suggested = server_url.clone();
    let server_url_for_pass = server_url.clone();
    let server_url_for_manual = server_url.clone();
    let game_for_home = game_for_view.clone();
    let game_for_board_drop = game_for_view.clone();

    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }

        div { class: "app-shell",
            header { class: "topbar",
                p { class: "topbar-kicker", "Scrabble PX" }
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
                        info_message.set(Some(format!("Logged in as {}", new_session.display_name)));
                        session.set(Some(new_session));
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
                }
            }

            div { class: "workspace-shell",
                GamesPanel {
                    summaries: game_summaries().clone(),
                    selected_id: game().as_ref().map(|current| current.id.clone()),
                    is_loading: is_loading(),
                    on_select: move |game_id: String| {
                        let server_url = server_url_for_select.clone();
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            match load_game_by_id(&server_url, &game_id).await {
                                Ok(loaded) => {
                                    info_message.set(None);
                                    dragging_tile_id.set(None);
                                    selected_blank_letter.set(None);
                                    staged_placements.set(Vec::new());
                                    websocket_game_id.set(None);
                                    game.set(Some(loaded));
                                }
                                Err(error) => error_message.set(Some(error)),
                            }
                            is_loading.set(false);
                        });
                    },
                    on_new_game: move |_| {
                        let server_url = server_url_for_create.clone();
                        let token = session().map(|current| current.session_token.clone());
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            match create_default_game(&server_url, token.as_deref()).await {
                                Ok(created) => {
                                    info_message.set(None);
                                    dragging_tile_id.set(None);
                                    selected_blank_letter.set(None);
                                    staged_placements.set(Vec::new());
                                    websocket_game_id.set(None);
                                    game.set(Some(created));
                                    if let Ok(summaries) = load_game_summaries(&server_url).await {
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
                        spawn(async move {
                            is_loading.set(true);
                            error_message.set(None);
                            match load_game_summaries(&server_url).await {
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
                    staged_placements: staged_placements().clone(),
                    can_stage_moves: can_submit_human_action,
                    on_drag_rack_tile: move |tile_id| {
                        dragging_tile_id.set(Some(tile_id));
                    },
                    on_drag_end_rack_tile: move |_| {
                        dragging_tile_id.set(None);
                    },
                    on_drop_board_cell: move |board_index| {
                        if !can_submit_human_action {
                            return;
                        }
                        if game_for_board_drop
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
                        let Some(tile) = current_rack_tiles(&game_for_board_drop, &staged_placements())
                            .into_iter()
                            .find(|t| t.id == tile_id)
                            else {
                            dragging_tile_id.set(None);
                            return;
                        };
                        let (tile_for_board, display_for_board) = match tile.tile.clone() {
                            TileDto::Blank { .. } => (TileDto::Blank { acting_as: None }, '?'),
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
                        dragging_tile_id.set(None);
                    },
                    on_clear_staged: move |_| {
                        dragging_tile_id.set(None);
                        selected_blank_letter.set(None);
                        staged_placements.set(Vec::new());
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
                                        dragging_tile_id.set(None);
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
                    can_submit_suggested: can_submit_human_action,
                    on_submit_suggested: move |_| {
                        let server_url = server_url_for_suggested.clone();
                        let current_game = game().clone();
                        let token = session().map(|current| current.session_token.clone());
                        if let Some(current_game) = current_game {
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match submit_suggested_move(&server_url, &current_game, token.as_deref()).await {
                                    Ok(updated) => {
                                        info_message.set(Some("Submitted suggested move.".to_string()));
                                        dragging_tile_id.set(None);
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
                    can_pass: can_submit_human_action,
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
                                        dragging_tile_id.set(None);
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
                    can_submit_manual: can_submit_manual_action,
                    on_submit_manual: move |_| {
                        let server_url = server_url_for_manual.clone();
                        let current_game = game().clone();
                        let staged = staged_placements().clone();
                        let token = session().map(|current| current.session_token.clone());
                        if let Some(current_game) = current_game {
                            spawn(async move {
                                is_loading.set(true);
                                error_message.set(None);
                                match submit_manual_move(
                                        &server_url,
                                        &current_game,
                                        &staged,
                                        DirectionDto::Horizontal,
                                        token.as_deref(),
                                    )
                                    .await
                                {
                                    Ok(updated) => {
                                        info_message.set(Some("Submitted staged move.".to_string()));
                                        dragging_tile_id.set(None);
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
                }

                Sidebar {
                    participants: game_for_view.participants.clone(),
                    moves: game_for_view.moves.clone(),
                    current_seat: game_for_view.current_seat,
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

async fn load_game_summaries(server_url: &str) -> Result<Vec<api::GameSummaryDto>, String> {
    get_json::<Vec<api::GameSummaryDto>>(&format!("{server_url}/games")).await
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

async fn validate_session(server_url: &str, session_token: &str) -> Result<api::PlayerDto, String> {
    let request = api::ValidateSessionRequest {
        session_token: session_token.to_string(),
    };
    post_json(&format!("{server_url}/auth/validate"), None, &request).await
}

async fn load_game_by_id(server_url: &str, game_id: &str) -> Result<GameStateDto, String> {
    get_json::<GameStateDto>(&format!("{server_url}/games/{game_id}")).await
}

async fn create_default_game(server_url: &str, token: Option<&str>) -> Result<GameStateDto, String> {
    let request = CreateGameRequest {
        seats: vec![
            CreateSeatRequest {
                kind: SeatKind::Human,
                display_name: "Alice".to_string(),
                engine_id: None,
            },
            CreateSeatRequest {
                kind: SeatKind::Engine,
                display_name: "Greedy".to_string(),
                engine_id: Some(DEFAULT_ENGINE_ID.to_string()),
            },
        ],
        seed: None,
        variant: None,
        language: None,
        board_layout: None,
    };

    post_json(&format!("{server_url}/games"), token, &request).await
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

async fn submit_suggested_move(
    server_url: &str,
    game: &GameStateDto,
    token: Option<&str>,
) -> Result<GameStateDto, String> {
    post_json::<(), _>(&format!("{server_url}/games/{}/suggest", game.id), token, &()).await
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

#[cfg(not(target_arch = "wasm32"))]
async fn get_json<R>(url: &str) -> Result<R, String>
where
    R: serde::de::DeserializeOwned,
{
    let response = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
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
    let response = Request::get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
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
    let response = request.send().await.map_err(|error| error.to_string())?;
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
        .map_err(|error| error.to_string())?;
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
) -> Option<MovePreviewView> {
    let request = match build_manual_move_request(game, staged, direction_hint) {
        Ok(r) => r,
        Err(detail) => {
            return Some(MovePreviewView {
                is_legal: false,
                headline: "Cannot preview this arrangement".to_string(),
                detail,
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
        None,
        &preview_request,
    )
    .await
    {
        Ok(response) => Some(MovePreviewView {
            is_legal: response.is_legal,
            headline: response.headline,
            detail: response.detail,
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
