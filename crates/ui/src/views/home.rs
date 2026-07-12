use crate::{
    app::{MovePreviewView, RackTileView, StagedPlacementView},
    components::{board_view::BoardView, rack_view::RackView},
};
use api::{GameStateDto, GameStatus, TileDto};
use dioxus::prelude::*;
use std::collections::HashSet;
use std::rc::Rc;

#[component]
pub fn Home(
    game: GameStateDto,
    is_live: bool,
    is_loading: bool,
    info_message: Option<String>,
    error_message: Option<String>,
    rack_tiles: Vec<RackTileView>,
    can_view_rack: bool,
    staged_placements: Vec<StagedPlacementView>,
    can_stage_moves: bool,
    selected_cell: Option<usize>,
    on_drag_rack_tile: EventHandler<usize>,
    on_drag_end_rack_tile: EventHandler<()>,
    on_drop_board_cell: EventHandler<usize>,
    on_select_cell: EventHandler<usize>,
    on_click_rack_tile: EventHandler<usize>,
    on_type_letter: EventHandler<char>,
    on_backspace: EventHandler<()>,
    on_delete: EventHandler<()>,
    on_clear_staged: EventHandler<()>,
    on_remove_staged: EventHandler<usize>,
    on_set_blank_letter: EventHandler<char>,
    selected_blank_letter: Option<char>,
    staged_preview: Option<MovePreviewView>,
    can_start: bool,
    on_start: EventHandler<()>,
    can_pass: bool,
    on_pass: EventHandler<()>,
    can_submit_manual: bool,
    on_submit_manual: EventHandler<()>,
    exchange_mode: bool,
    exchange_selected: HashSet<usize>,
    can_toggle_exchange: bool,
    on_toggle_exchange_mode: EventHandler<()>,
    on_toggle_exchange_tile: EventHandler<usize>,
    can_confirm_exchange: bool,
    on_confirm_exchange: EventHandler<()>,
    on_cancel_exchange: EventHandler<()>,
) -> Element {
    let has_rack = can_view_rack;
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

    // Keyboard typing (letter placement, backspace) only works while this
    // element has DOM focus. Clicking a board cell reclaims focus here
    // explicitly (see `on_select_cell` below) since a plain, non-form
    // element losing focus to e.g. a turn-action button is otherwise a dead
    // end — nothing else would naturally hand focus back.
    let mut keyboard_focus: Signal<Option<Rc<MountedData>>> = use_signal(|| None);

    rsx! {
        section {
            class: "workspace-main",
            tabindex: "0",
            onmounted: move |event| {
                keyboard_focus.set(Some(event.data()));
            },
            onkeydown: move |event| {
                if selected_cell.is_none() {
                    return;
                }
                match event.key() {
                    Key::Character(text) if text.chars().count() == 1 => {
                        if let Some(letter) = text.chars().next().filter(|c| c.is_ascii_alphabetic()) {
                            event.prevent_default();
                            on_type_letter.call(letter.to_ascii_uppercase());
                        }
                    }
                    Key::Backspace => {
                        event.prevent_default();
                        on_backspace.call(());
                    }
                    Key::Delete => {
                        event.prevent_default();
                        on_delete.call(());
                    }
                    _ => {}
                }
            },
            div { class: "status-strip",
                span { class: "meta-chip", "{format_status(&game)}" }
                if is_active {
                    span { class: "meta-chip", "Turn: {current_turn_name(&game)}" }
                    span { class: "meta-chip", "{crate::time_format::format_time_remaining(&game.turn_started_at, game.move_time_limit_seconds)}" }
                }
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
                    selected_cell,
                    on_drop_tile: on_drop_board_cell,
                    on_remove_staged,
                    on_select_cell: move |index| {
                        if let Some(handle) = keyboard_focus() {
                            spawn(async move {
                                let _ = handle.set_focus(true).await;
                            });
                        }
                        on_select_cell.call(index);
                    },
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
                    div { class: "preview-slot",
                        if let Some(preview) = staged_preview {
                            div { class: if preview.is_legal { "preview-banner" } else { "preview-banner preview-banner-error" },
                                h3 { class: "preview-title", "{preview.headline}" }
                                if preview.is_legal && !preview.detail.is_empty() {
                                    p { class: "composer-copy", "{preview.detail}" }
                                }
                            }
                        }
                    }
                    RackView {
                        tiles: rack_tiles,
                        can_stage_moves,
                        exchange_mode,
                        exchange_selected: exchange_selected.clone(),
                        on_drag_start: on_drag_rack_tile,
                        on_drag_end: on_drag_end_rack_tile,
                        on_click_tile: on_click_rack_tile,
                        on_toggle_exchange_tile,
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
                        if is_active && exchange_mode {
                            button {
                                class: "toggle-button toggle-button-muted",
                                disabled: is_loading,
                                onclick: move |_| on_cancel_exchange.call(()),
                                "Cancel"
                            }
                            button {
                                class: "toggle-button",
                                disabled: is_loading || !can_confirm_exchange,
                                onclick: move |_| on_confirm_exchange.call(()),
                                "Confirm Exchange ({exchange_selected.len()})"
                            }
                        }
                        if is_active && !exchange_mode {
                            button {
                                class: "toggle-button toggle-button-muted",
                                disabled: is_loading || !can_pass,
                                onclick: move |_| on_pass.call(()),
                                "Pass"
                            }
                            button {
                                class: "toggle-button toggle-button-muted",
                                disabled: is_loading || !can_toggle_exchange,
                                onclick: move |_| on_toggle_exchange_mode.call(()),
                                "Exchange"
                            }
                            button {
                                class: "toggle-button",
                                disabled: is_loading || !can_submit_manual,
                                onclick: move |_| on_submit_manual.call(()),
                                "Play"
                            }
                        }
                    }
                }
            }
        }
    }
}

fn current_turn_name(game: &GameStateDto) -> &str {
    game.participants
        .iter()
        .find(|participant| participant.seat_number == game.current_seat)
        .map(|participant| participant.display_name.as_str())
        .unwrap_or("Unknown")
}

fn format_status(game: &GameStateDto) -> &'static str {
    match game.status {
        api::GameStatus::Waiting => "Waiting",
        api::GameStatus::Active => "Active",
        api::GameStatus::Finished => "Finished",
    }
}
