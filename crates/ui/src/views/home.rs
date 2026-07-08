use crate::{
    app::{MovePreviewView, RackTileView, StagedPlacementView},
    components::{board_view::BoardView, rack_view::RackView, sidebar::Sidebar},
};
use api::GameStateDto;
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
    selected_rack_tile_id: Option<usize>,
    staged_placements: Vec<StagedPlacementView>,
    can_stage_moves: bool,
    placement_direction: api::DirectionDto,
    inferred_direction: api::DirectionDto,
    on_board_cell_click: EventHandler<usize>,
    on_rack_tile_click: EventHandler<usize>,
    on_clear_staged: EventHandler<()>,
    on_remove_staged: EventHandler<usize>,
    on_set_horizontal: EventHandler<()>,
    on_set_vertical: EventHandler<()>,
    on_set_blank_letter: EventHandler<char>,
    selected_blank_letter: Option<char>,
    selected_rack_tile_is_blank: bool,
    staged_preview: Option<MovePreviewView>,
) -> Element {
    let first_rack = game.racks.first().cloned();
    let direction_label = match placement_direction {
        api::DirectionDto::Horizontal => "Horizontal",
        api::DirectionDto::Vertical => "Vertical",
    };
    let inferred_direction_label = match inferred_direction {
        api::DirectionDto::Horizontal => "Horizontal",
        api::DirectionDto::Vertical => "Vertical",
    };
    let selected_tile_label = selected_rack_tile_id.and_then(|selected_id| {
        rack_tiles
            .iter()
            .find(|tile| tile.id == selected_id)
            .map(|tile| tile.display)
    });
    let selected_tile_text = selected_tile_label
        .map(|tile| tile.to_string())
        .unwrap_or_else(|| "none".to_string());
    let selected_blank_text = selected_blank_letter
        .map(|letter| letter.to_string())
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
    let ordered_staged = {
        let mut ordered = staged_placements.clone();
        ordered.sort_by_key(|placement| placement.board_index);
        ordered
    };
    let staged_rows = ordered_staged.clone().into_iter().map(|placement| {
        let row = placement.board_index / 15 + 1;
        let col = (placement.board_index % 15) as u8;
        let label = format!("{}{}", (b'A' + col) as char, row);

        rsx! {
            div { key: "{placement.board_index}", class: "staged-item",
                div {
                    span { class: "staged-badge", "{label}" }
                    span { class: "staged-letter", "{placement.display}" }
                }
                button {
                    class: "staged-remove-button",
                    onclick: move |_| on_remove_staged.call(placement.board_index),
                    "Remove"
                }
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
                        on_cell_click: on_board_cell_click,
                    }
                }

                if let Some(rack) = first_rack {
                    div { class: "rack-panel",
                        div { class: "panel-header",
                            div {
                                h2 { "Rack Preview" }
                                p {
                                    "The local client can show previews without owning canonical state."
                                }
                            }
                        }
                        div { class: "composer-toolbar",
                            div { class: "composer-group",
                                button {
                                    class: if placement_direction == api::DirectionDto::Horizontal { "direction-button direction-button-active" } else { "direction-button" },
                                    disabled: !can_stage_moves,
                                    onclick: move |_| on_set_horizontal.call(()),
                                    "Horizontal"
                                }
                                button {
                                    class: if placement_direction == api::DirectionDto::Vertical { "direction-button direction-button-active" } else { "direction-button" },
                                    disabled: !can_stage_moves,
                                    onclick: move |_| on_set_vertical.call(()),
                                    "Vertical"
                                }
                            }
                            button {
                                class: "direction-button direction-button-muted",
                                disabled: staged_placements.is_empty(),
                                onclick: move |_| on_clear_staged.call(()),
                                "Clear"
                            }
                        }
                        p { class: "composer-copy",
                            "Direction: {direction_label}. Selected tile: {selected_tile_text}."
                        }
                        p { class: "composer-copy",
                            "Current move orientation: {inferred_direction_label}. Single-tile plays infer from nearby board tiles when possible."
                        }
                        p { class: "composer-copy",
                            if can_stage_moves {
                                "Click a rack tile, then click empty board squares to stage a move. Click a staged square again to remove it."
                            } else {
                                "Manual placement is enabled only for an active human turn."
                            }
                        }
                        if selected_rack_tile_is_blank {
                            div { class: "blank-picker",
                                p { class: "composer-copy", "Blank assignment: {selected_blank_text}" }
                                div { class: "blank-picker-grid", {blank_letter_buttons} }
                            }
                        }
                        if let Some(preview) = staged_preview {
                            div { class: if preview.is_legal { "preview-banner" } else { "preview-banner preview-banner-error" },
                                h3 { class: "preview-title", "{preview.headline}" }
                                p { class: "composer-copy", "{preview.detail}" }
                            }
                        }
                        if !ordered_staged.is_empty() {
                            div { class: "staged-list",
                                h3 { class: "preview-title", "Staged Placements" }
                                {staged_rows}
                            }
                        }
                        RackView {
                            rack,
                            tiles: rack_tiles,
                            selected_rack_tile_id,
                            on_tile_click: on_rack_tile_click,
                            can_stage_moves,
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
