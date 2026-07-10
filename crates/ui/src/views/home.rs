use crate::{
    app::{MovePreviewView, RackTileView, StagedPlacementView},
    components::{board_view::BoardView, rack_view::RackView, sidebar::Sidebar},
};
use api::{GameStateDto, TileDto};
use dioxus::prelude::*;

#[component]
pub fn Home(
    game: GameStateDto,
    server_url: String,
    is_live: bool,
    is_loading: bool,
    info_message: Option<String>,
    error_message: Option<String>,
    rack_tiles: Vec<RackTileView>,
    staged_placements: Vec<StagedPlacementView>,
    can_stage_moves: bool,
    on_drag_rack_tile: EventHandler<usize>,
    on_drag_end_rack_tile: EventHandler<()>,
    on_drop_board_cell: EventHandler<usize>,
    on_clear_staged: EventHandler<()>,
    on_remove_staged: EventHandler<usize>,
    on_set_blank_letter: EventHandler<char>,
    selected_blank_letter: Option<char>,
    staged_preview: Option<MovePreviewView>,
) -> Element {
    let first_rack = game.racks.first().cloned();

    // Show blank picker when there is a staged blank tile still needing a letter.
    let has_unresolved_blank = staged_placements
        .iter()
        .any(|p| matches!(p.tile, TileDto::Blank { acting_as: None }));

    let selected_blank_text = selected_blank_letter
        .map(|l| l.to_string())
        .unwrap_or_else(|| "choose a letter".to_string());

    let blank_letter_buttons = ('A'..='Z').map(|letter| {
        let class_name = if selected_blank_letter == Some(letter) {
            "blank-letter-button blank-letter-button-active"
        } else {
            "blank-letter-button"
        };
        rsx! {
            button {
                key: "{letter}",
                class: "{class_name}",
                onclick: move |_| on_set_blank_letter.call(letter),
                "{letter}"
            }
        }
    });

    rsx! {
        div { class: "workspace-shell",
            section { class: "workspace-main",
                div { class: "hero-panel",
                    p { class: "eyebrow", "Web Client" }
                    h1 { "Scrabble PX" }
                    p { class: "hero-copy",
                        "Server-authoritative play, engine-ready seats, and shared rules previews sit behind one API boundary."
                    }
                    div { class: "hero-metadata",
                        span { class: "meta-chip", "Status: {format_status(&game)}" }
                        span { class: "meta-chip", "Seat: {game.current_seat}" }
                        span { class: "meta-chip", "Bag: {game.bag_count}" }
                        span { class: "meta-chip",
                            if is_live {
                                "Live server state"
                            } else {
                                "No live game loaded"
                            }
                        }
                        if is_loading {
                            span { class: "meta-chip", "Working..." }
                        }
                    }
                    if let Some(info_message) = info_message {
                        p { class: "status-banner", "{info_message}" }
                    }
                    if let Some(error_message) = error_message {
                        p { class: "error-banner", "{error_message}" }
                    }
                }

                div { class: "board-panel",
                    div { class: "panel-header",
                        div {
                            h2 { "Board" }
                            p {
                                if is_live {
                                    "Live layout typed against the shared API DTOs."
                                } else {
                                    "Create or load a server game to replace this placeholder board."
                                }
                            }
                        }
                        div { class: "panel-tag", "{server_url}" }
                    }
                    BoardView {
                        board: game.board.clone(),
                        staged_placements: staged_placements.clone(),
                        can_stage_moves,
                        on_drop_tile: on_drop_board_cell,
                        on_remove_staged,
                    }
                }

                if let Some(rack) = first_rack {
                    div { class: "rack-panel",
                        div { class: "panel-header",
                            div {
                                h2 { "Rack" }
                                p {
                                    if can_stage_moves {
                                        "Drag a tile onto the board. Right-click a staged tile to remove it."
                                    } else {
                                        "Manual placement is enabled only for an active human turn."
                                    }
                                }
                            }
                            if !staged_placements.is_empty() {
                                button {
                                    class: "direction-button direction-button-muted",
                                    onclick: move |_| on_clear_staged.call(()),
                                    "Clear"
                                }
                            }
                        }
                        if has_unresolved_blank {
                            div { class: "blank-picker",
                                p { class: "composer-copy",
                                    "Blank tile — choose a letter: {selected_blank_text}"
                                }
                                div { class: "blank-picker-grid", {blank_letter_buttons} }
                            }
                        }
                        if let Some(preview) = staged_preview {
                            div { class: if preview.is_legal { "preview-banner" } else { "preview-banner preview-banner-error" },
                                h3 { class: "preview-title", "{preview.headline}" }
                                p { class: "composer-copy", "{preview.detail}" }
                            }
                        }
                        RackView {
                            tiles: rack_tiles,
                            can_stage_moves,
                            on_drag_start: on_drag_rack_tile,
                            on_drag_end: on_drag_end_rack_tile,
                        }
                    }
                }
            }

            Sidebar {
                participants: game.participants.clone(),
                moves: game.moves.clone(),
                current_seat: game.current_seat,
                status: game.status.clone(),
            }
        }
    }
}

fn format_status(game: &GameStateDto) -> &'static str {
    match game.status {
        api::GameStatus::Waiting => "Waiting",
        api::GameStatus::Active => "Active",
        api::GameStatus::Finished => "Finished",
    }
}
