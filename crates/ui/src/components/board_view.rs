use crate::app::StagedPlacementView;
use api::{BoardCellDto, PremiumDto};
use dioxus::prelude::*;

#[component]
pub fn BoardView(
    board: Vec<BoardCellDto>,
    staged_placements: Vec<StagedPlacementView>,
    can_stage_moves: bool,
    on_cell_click: EventHandler<usize>,
) -> Element {
    let cells = board.iter().enumerate().map(|(index, cell)| {
        let staged = staged_placements
            .iter()
            .find(|placement| placement.board_index == index);
        let class_name = if cell.letter.is_some() {
            format!(
                "board-cell {} board-cell-filled",
                premium_class(&cell.premium)
            )
        } else if staged.is_some() {
            format!(
                "board-cell {} board-cell-staged",
                premium_class(&cell.premium)
            )
        } else {
            format!("board-cell {}", premium_class(&cell.premium))
        };
        let is_clickable = can_stage_moves && cell.letter.is_none();
        let button_class = if is_clickable {
            format!("{class_name} board-cell-clickable")
        } else {
            class_name
        };

        rsx! {
            button {
                key: "{index}",
                class: "{button_class}",
                disabled: !is_clickable,
                onclick: move |_| on_cell_click.call(index),
                if let Some(letter) = cell.letter {
                    div { class: "tile-face", "{letter}" }
                } else if let Some(staged) = staged {
                    div { class: "tile-face", "{staged.display}" }
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
