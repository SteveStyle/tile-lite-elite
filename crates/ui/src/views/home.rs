use crate::{
    app::{MovePreviewView, RackTileView, StagedPlacementView},
    components::{board_view::BoardView, rack_view::RackView},
};
use api::{GameStateDto, GameStatus, TileDto};
use dioxus::prelude::*;

#[component]
pub fn Home(
    game: GameStateDto,
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
    can_start: bool,
    on_start: EventHandler<()>,
    can_submit_suggested: bool,
    on_submit_suggested: EventHandler<()>,
    can_pass: bool,
    on_pass: EventHandler<()>,
    can_submit_manual: bool,
    on_submit_manual: EventHandler<()>,
) -> Element {
    let has_rack = !game.racks.is_empty();
    let is_waiting = game.status == GameStatus::Waiting;
    let is_active = game.status == GameStatus::Active;

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
        section { class: "workspace-main",
            div { class: "status-strip",
                span { class: "meta-chip", "{format_status(&game)}" }
                span { class: "meta-chip", "Seat {game.current_seat}" }
                span { class: "meta-chip", "Bag {game.bag_count}" }
                if !is_live {
                    span { class: "meta-chip", "No game selected" }
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

            div { class: "board-panel",
                BoardView {
                    board: game.board.clone(),
                    staged_placements: staged_placements.clone(),
                    can_stage_moves,
                    on_drop_tile: on_drop_board_cell,
                    on_remove_staged,
                }
            }

            if has_rack {
                div { class: "rack-panel",
                    div { class: "panel-header",
                        h2 { "Rack" }
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

                    div { class: "turn-actions",
                        if is_waiting {
                            button {
                                class: "toggle-button",
                                disabled: is_loading || !can_start,
                                onclick: move |_| on_start.call(()),
                                "Start"
                            }
                        }
                        if is_active {
                            button {
                                class: "toggle-button toggle-button-muted",
                                disabled: is_loading || !can_submit_suggested,
                                onclick: move |_| on_submit_suggested.call(()),
                                "Play Suggested Move"
                            }
                            button {
                                class: "toggle-button toggle-button-muted",
                                disabled: is_loading || !can_pass,
                                onclick: move |_| on_pass.call(()),
                                "Pass"
                            }
                            button {
                                class: "toggle-button",
                                disabled: is_loading || !can_submit_manual,
                                onclick: move |_| on_submit_manual.call(()),
                                "Submit Staged Move"
                            }
                        }
                    }
                }
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
