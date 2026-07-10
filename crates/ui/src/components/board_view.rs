use crate::app::StagedPlacementView;
use api::{BoardCellDto, PremiumDto};
use dioxus::prelude::*;

#[component]
pub fn BoardView(
    board: Vec<BoardCellDto>,
    staged_placements: Vec<StagedPlacementView>,
    can_stage_moves: bool,
    on_drop_tile: EventHandler<usize>,
    on_remove_staged: EventHandler<usize>,
) -> Element {
    let cells = board.iter().enumerate().map(|(index, cell)| {
        let staged = staged_placements
            .iter()
            .find(|placement| placement.board_index == index);
        let has_letter = cell.letter.is_some();
        let is_staged = staged.is_some();
        let can_drop = can_stage_moves && !has_letter && !is_staged;

        let class_name = if has_letter {
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

        rsx! {
            div {
                key: "{index}",
                class: "{class_name}",
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
                oncontextmenu: move |event| {
                    if is_staged {
                        event.prevent_default();
                        on_remove_staged.call(index);
                    }
                },
                if let Some(letter) = cell.letter {
                    div { class: "tile-face", "{letter}" }
                } else if let Some(staged) = staged {
                    div { class: "tile-face tile-face-staged", "{staged.display}" }
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
