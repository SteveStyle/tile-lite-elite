use crate::app::StagedPlacementView;
use api::{BoardCellDto, PremiumDto};
use dioxus::prelude::*;
use std::collections::HashSet;

#[component]
pub fn BoardView(
    board: Vec<BoardCellDto>,
    staged_placements: Vec<StagedPlacementView>,
    last_move_cells: HashSet<usize>,
    can_stage_moves: bool,
    selected_cell: Option<usize>,
    on_drop_tile: EventHandler<usize>,
    on_remove_staged: EventHandler<usize>,
    on_drag_staged_tile: EventHandler<usize>,
    on_drag_end_staged_tile: EventHandler<usize>,
    on_select_cell: EventHandler<usize>,
) -> Element {
    let cells = board.iter().enumerate().map(|(index, cell)| {
        let staged = staged_placements
            .iter()
            .find(|placement| placement.board_index == index);
        let has_letter = cell.letter.is_some();
        let is_staged = staged.is_some();
        let can_drop = can_stage_moves && !has_letter && !is_staged;
        let is_selectable = can_stage_moves && !has_letter;
        let is_selected = selected_cell == Some(index);
        let is_last_move = has_letter && last_move_cells.contains(&index);
        // A staged tile can be picked back up and moved to another cell,
        // or dragged off the board entirely to return it to the rack (see
        // on_drag_end_staged_tile) — same turn-taking gate as a fresh drag
        // from the rack.
        let staged_draggable = is_staged && can_stage_moves;

        let mut class_name = if has_letter {
            format!(
                "board-cell {} board-cell-filled",
                premium_class(&cell.premium)
            )
        } else if is_staged {
            format!(
                "board-cell {} board-cell-staged",
                premium_class(&cell.premium)
            )
        } else if can_drop {
            format!(
                "board-cell {} board-cell-droptarget",
                premium_class(&cell.premium)
            )
        } else {
            format!("board-cell {}", premium_class(&cell.premium))
        };
        if is_selectable {
            class_name.push_str(" board-cell-clickable");
        }
        if is_selected {
            class_name.push_str(" board-cell-selected");
        }
        if is_last_move {
            class_name.push_str(" board-cell-last-move");
        }

        rsx! {
            div {
                key: "{index}",
                class: "{class_name}",
                draggable: "{staged_draggable}",
                ondragover: move |event| {
                    if can_drop {
                        event.prevent_default();
                    }
                },
                ondrop: move |event| {
                    event.prevent_default();
                    if can_drop {
                        on_drop_tile.call(index);
                    }
                },
                ondragstart: move |_| {
                    if staged_draggable {
                        on_drag_staged_tile.call(index);
                    }
                },
                ondragend: move |_| {
                    if staged_draggable {
                        on_drag_end_staged_tile.call(index);
                    }
                },
                onclick: move |_| {
                    if is_selectable {
                        on_select_cell.call(index);
                    }
                },
                oncontextmenu: move |event| {
                    if is_staged {
                        event.prevent_default();
                        on_remove_staged.call(index);
                    }
                },
                if let Some(letter) = cell.letter {
                    div { class: "tile-face",
                        span { class: "tile-letter", "{letter}" }
                        span { class: "tile-value", "{crate::tile_value::board_cell_point_value(letter, cell.is_blank)}" }
                    }
                } else if let Some(staged) = staged {
                    div { class: "tile-face tile-face-staged",
                        span { class: "tile-letter", "{staged.display}" }
                        span { class: "tile-value", "{crate::tile_value::tile_point_value(&staged.tile)}" }
                    }
                } else {
                    div { class: "premium-label", "{premium_label(&cell.premium)}" }
                }
            }
        }
    });

    rsx! {
        div { class: "board-grid", {cells} }
    }
}

fn premium_class(premium: &PremiumDto) -> &'static str {
    match premium {
        PremiumDto::Blank => "premium-blank",
        PremiumDto::DoubleLetter => "premium-double-letter",
        PremiumDto::TripleLetter => "premium-triple-letter",
        PremiumDto::DoubleWord => "premium-double-word",
        PremiumDto::TripleWord => "premium-triple-word",
    }
}

fn premium_label(premium: &PremiumDto) -> &'static str {
    match premium {
        PremiumDto::Blank => "",
        PremiumDto::DoubleLetter => "DL",
        PremiumDto::TripleLetter => "TL",
        PremiumDto::DoubleWord => "DW",
        PremiumDto::TripleWord => "TW",
    }
}
