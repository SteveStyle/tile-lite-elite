use crate::{
    app::{MovePreviewView, RackTileView, StagedPlacementView},
    components::{board_view::BoardView, rack_view::RackView},
    edition_label::edition_label,
};
use api::{DirectionDto, GameStateDto, GameStatus, TileDto};
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
    on_shuffle_rack: EventHandler<()>,
    can_view_rack: bool,
    staged_placements: Vec<StagedPlacementView>,
    can_stage_moves: bool,
    selected_cell: Option<usize>,
    can_toggle_direction: bool,
    current_typing_direction: DirectionDto,
    on_toggle_direction: EventHandler<()>,
    on_drag_rack_tile: EventHandler<usize>,
    on_drag_end_rack_tile: EventHandler<()>,
    on_drop_rack_tile: EventHandler<usize>,
    on_drag_staged_tile: EventHandler<usize>,
    on_drag_end_staged_tile: EventHandler<usize>,
    on_drop_board_cell: EventHandler<usize>,
    on_select_cell: EventHandler<usize>,
    on_move_selection: EventHandler<(DirectionDto, bool)>,
    on_click_rack_tile: EventHandler<usize>,
    on_type_letter: EventHandler<char>,
    on_backspace: EventHandler<()>,
    on_delete: EventHandler<()>,
    on_clear_staged: EventHandler<()>,
    on_remove_staged: EventHandler<usize>,
    on_set_blank_letter: EventHandler<String>,
    selected_blank_letter: Option<String>,
    staged_preview: Option<MovePreviewView>,
    is_your_turn: bool,
    can_pass: bool,
    on_pass: EventHandler<()>,
    can_resign: bool,
    on_resign: EventHandler<()>,
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
    let has_rack_tiles = !rack_tiles.is_empty();
    let is_active = game.status == GameStatus::Active;

    // Resolved once and reused everywhere this component needs the active
    // game's actual alphabet/letter values, rather than assuming the
    // standard Latin 26 — different editions (Wordfeud, German, ...)
    // genuinely differ here. Falls back to `official()` only for an
    // edition name this client build doesn't recognize, which shouldn't
    // happen for a real loaded game.
    let rules = rules_shared::VariantRules::by_name(&game.variant)
        .unwrap_or_else(rules_shared::VariantRules::official);

    // Show blank picker when there is a staged blank tile still needing a letter.
    let has_unresolved_blank = staged_placements
        .iter()
        .any(|p| matches!(p.tile, TileDto::Blank { acting_as: None }));

    let selected_blank_text = selected_blank_letter
        .clone()
        .unwrap_or_else(|| "choose a letter".to_string());

    let blank_letter_buttons = rules
        .letters()
        .map(|letter| rules.letter_grapheme(letter).to_string())
        .map(|letter| {
            let class_name = if selected_blank_letter.as_deref() == Some(letter.as_str()) {
                "blank-letter-button blank-letter-button-active"
            } else {
                "blank-letter-button"
            };
            let letter_for_click = letter.clone();
            rsx! {
                button {
                    key: "{letter}",
                    class: "{class_name}",
                    onclick: move |_| on_set_blank_letter.call(letter_for_click.clone()),
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
    // Resigning ends the game outright, so it's gated behind an explicit
    // confirmation rather than firing straight off the button click.
    let mut confirming_resign = use_signal(|| false);
    // Cloned for the `move` keydown closure below — `rules` itself is
    // still needed afterward (passed into `BoardView`/`RackView`).
    let rules_for_keydown = rules.clone();

    rsx! {
        section {
            class: "workspace-main",
            tabindex: "0",
            onmounted: move |event| {
                keyboard_focus.set(Some(event.data()));
            },
            onkeydown: move |event| {
                if event.key() == Key::Enter {
                    // Works regardless of cursor position — once tiles are
                    // staged, Enter submits them the same as clicking Play.
                    if can_submit_manual {
                        event.prevent_default();
                        on_submit_manual.call(());
                    }
                    return;
                }
                if selected_cell.is_none() {
                    return;
                }
                match event.key() {
                    Key::ArrowLeft => {
                        event.prevent_default();
                        on_move_selection.call((DirectionDto::Horizontal, false));
                    }
                    Key::ArrowRight => {
                        event.prevent_default();
                        on_move_selection.call((DirectionDto::Horizontal, true));
                    }
                    Key::ArrowUp => {
                        event.prevent_default();
                        on_move_selection.call((DirectionDto::Vertical, false));
                    }
                    Key::ArrowDown => {
                        event.prevent_default();
                        on_move_selection.call((DirectionDto::Vertical, true));
                    }
                    Key::Character(text) if text == " " => {
                        if can_toggle_direction {
                            event.prevent_default();
                            on_toggle_direction.call(());
                        }
                    }
                    Key::Character(text) if text.chars().count() == 1 => {
                        if let Some(ch) = text.chars().next() {
                            let upper = ch.to_uppercase().next().unwrap_or(ch);
                            if rules_for_keydown
                                .alphabet
                                .to_letter(&upper.to_string())
                                .is_some()
                            {
                                event.prevent_default();
                                on_type_letter.call(upper);
                            }
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
                if is_live {
                    span { class: "meta-chip", "{edition_label(&game.variant)}" }
                    span { class: "meta-chip", "{format_status(&game)}" }
                }
                if is_active {
                    span { class: "meta-chip", "Turn: {current_turn_name(&game)}" }
                    span { class: "meta-chip", "{crate::time_format::format_time_remaining(&game.turn_started_at, game.move_time_limit_seconds)}" }
                }
                if is_loading {
                    span { class: "meta-chip", "Working..." }
                }
            }
            if let Some(summary) = finished_game_summary(&game) {
                p { class: "game-over-banner", "{summary}" }
            }
            if !has_rack {
                if let Some(error_message) = error_message.clone() {
                    p { class: "error-banner", "{error_message}" }
                } else if let Some(info_message) = info_message.clone() {
                    p { class: "status-banner", "{info_message}" }
                }
            }

            div { class: "board-panel",
                BoardView {
                    board: game.board.clone(),
                    staged_placements: staged_placements.clone(),
                    last_move_cells: last_move_board_indices(&game.moves),
                    can_stage_moves,
                    selected_cell,
                    letter_values: rules.letter_values,
                    alphabet: rules.alphabet.clone(),
                    on_drop_tile: on_drop_board_cell,
                    on_remove_staged,
                    on_drag_staged_tile,
                    on_drag_end_staged_tile,
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
                    if has_unresolved_blank {
                        div { class: "blank-picker",
                            p { class: "composer-copy",
                                "Blank tile — choose a letter: {selected_blank_text}"
                            }
                            div { class: "blank-picker-grid", {blank_letter_buttons} }
                        }
                    }
                    // The one message slot for this composer — a fixed
                    // size regardless of which of these is showing, so it
                    // never shifts the tiles below it. Priority: the live
                    // preview of what's currently staged, else a submit
                    // error, else a plain status line (whose turn it is).
                    div { class: "preview-slot",
                        if let Some(preview) = staged_preview {
                            div { class: if preview.is_legal { "preview-banner" } else { "preview-banner preview-banner-error" },
                                div { class: "preview-banner-top",
                                    h3 { class: "preview-title", "{preview.headline}" }
                                    if let Some(score) = preview.score {
                                        span { class: "preview-score", "+{score}" }
                                    }
                                }
                                if preview.is_legal && !preview.detail.is_empty() {
                                    p { class: "composer-copy", "{preview.detail}" }
                                }
                            }
                        } else if let Some(error_message) = error_message.clone() {
                            div { class: "preview-banner preview-banner-error",
                                p { class: "composer-copy", "{error_message}" }
                            }
                        } else if is_active {
                            div { class: "preview-banner",
                                p { class: "composer-copy",
                                    if is_your_turn { "Your turn" } else { "Waiting for {current_turn_name(&game)}" }
                                }
                            }
                        }
                    }
                    div { class: "rack-row",
                        RackView {
                            tiles: rack_tiles,
                            can_stage_moves,
                            exchange_mode,
                            exchange_selected: exchange_selected.clone(),
                            letter_values: rules.letter_values,
                            alphabet: rules.alphabet.clone(),
                            on_drag_start: on_drag_rack_tile,
                            on_drag_end: on_drag_end_rack_tile,
                            on_drop_tile: on_drop_rack_tile,
                            on_click_tile: on_click_rack_tile,
                            on_toggle_exchange_tile,
                        }
                        span { class: "meta-chip rack-row-bag", "Bag {game.bag_count}" }
                    }

                    div { class: "turn-actions",
                        div { class: "turn-actions-left",
                            if has_rack_tiles {
                                button {
                                    class: "direction-button direction-button-muted",
                                    onclick: move |_| on_shuffle_rack.call(()),
                                    "Shuffle"
                                }
                            }
                            if !staged_placements.is_empty() {
                                button {
                                    class: "direction-button direction-button-muted",
                                    onclick: move |_| on_clear_staged.call(()),
                                    "Clear"
                                }
                            }
                            if can_toggle_direction {
                                button {
                                    class: "direction-button direction-button-muted",
                                    title: "Change which way this word reads — same as pressing space bar",
                                    onclick: move |_| on_toggle_direction.call(()),
                                    {
                                        match current_typing_direction {
                                            DirectionDto::Horizontal => "⇄ Switch to Down",
                                            DirectionDto::Vertical => "⇄ Switch to Across",
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "turn-actions-buttons",
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
                        if is_active && !exchange_mode {
                            div { class: "turn-actions-resign",
                                button {
                                    class: "toggle-button toggle-button-muted resign-button",
                                    disabled: is_loading || !can_resign,
                                    onclick: move |_| confirming_resign.set(true),
                                    "Resign"
                                }
                            }
                        }
                    }
                }
            }

            if confirming_resign() {
                div { class: "modal-backdrop",
                    div { class: "modal-card",
                        h2 { class: "modal-title", "Resign this game?" }
                        p { class: "modal-copy", "This ends the game immediately — there's no undoing it." }
                        div { class: "modal-actions",
                            button {
                                class: "toggle-button toggle-button-muted",
                                onclick: move |_| confirming_resign.set(false),
                                "Cancel"
                            }
                            button {
                                class: "toggle-button",
                                onclick: move |_| {
                                    confirming_resign.set(false);
                                    on_resign.call(());
                                },
                                "Yes, resign"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Board squares to highlight as "the last move" — whatever `moves.last()`
/// placed, which is empty for a pass/exchange/resign/timeout (nothing on
/// the board changed) rather than falling back to an earlier placement.
fn last_move_board_indices(moves: &[api::MoveRecordDto]) -> HashSet<usize> {
    moves
        .last()
        .map(|record| {
            record
                .positions
                .iter()
                .map(|p| p.y as usize * crate::app::BOARD_WIDTH + p.x as usize)
                .collect()
        })
        .unwrap_or_default()
}

/// The persistent "who won, and why" banner shown once a game finishes —
/// previously the only way to tell was to notice the status badge in the
/// games list and work the scores out by hand. `None` while the game is
/// still in progress.
fn finished_game_summary(game: &GameStateDto) -> Option<String> {
    if game.status != GameStatus::Finished {
        return None;
    }

    let seat_name = |seat: u8| -> &str {
        game.participants
            .iter()
            .find(|p| p.seat_number == seat)
            .map(|p| p.display_name.as_str())
            .unwrap_or("Someone")
    };

    let outcome = match game.winner_seat {
        Some(seat) => format!("Game over — {} won!", seat_name(seat)),
        None => "Game over — it's a tie!".to_string(),
    };

    match (game.final_bonus_seat, game.final_bonus_points) {
        (Some(seat), Some(points)) if points > 0 => Some(format!(
            "{outcome} {} went out and picked up a {points}-point bonus from the other players' racks.",
            seat_name(seat)
        )),
        _ => Some(outcome),
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
        api::GameStatus::Active => "Playing",
        api::GameStatus::Finished => "Finished",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place_record(positions: Vec<api::PositionDto>) -> api::MoveRecordDto {
        api::MoveRecordDto {
            move_number: 1,
            seat_number: 0,
            move_type: "place".to_string(),
            main_word: Some("CAT".to_string()),
            score_delta: 10,
            positions,
            description: String::new(),
        }
    }

    fn pass_record() -> api::MoveRecordDto {
        api::MoveRecordDto {
            move_number: 2,
            seat_number: 1,
            move_type: "pass".to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: String::new(),
        }
    }

    #[test]
    fn highlights_the_last_placed_move_s_squares() {
        let moves = vec![place_record(vec![
            api::PositionDto { x: 7, y: 7 },
            api::PositionDto { x: 8, y: 7 },
        ])];
        let indices = last_move_board_indices(&moves);
        assert_eq!(indices, HashSet::from([7 * 15 + 7, 7 * 15 + 8]));
    }

    #[test]
    fn a_trailing_pass_has_nothing_to_highlight_even_after_an_earlier_placement() {
        let moves = vec![
            place_record(vec![api::PositionDto { x: 7, y: 7 }]),
            pass_record(),
        ];
        assert!(last_move_board_indices(&moves).is_empty());
    }

    #[test]
    fn no_moves_yet_highlights_nothing() {
        assert!(last_move_board_indices(&[]).is_empty());
    }

    fn participant(seat_number: u8, display_name: &str, score: i32) -> api::ParticipantDto {
        api::ParticipantDto {
            seat_number,
            kind: api::SeatKind::Human,
            display_name: display_name.to_string(),
            player_id: None,
            engine_id: None,
            score,
        }
    }

    fn finished_game(
        winner_seat: Option<u8>,
        final_bonus_seat: Option<u8>,
        final_bonus_points: Option<i32>,
        participants: Vec<api::ParticipantDto>,
    ) -> GameStateDto {
        GameStateDto {
            id: "game-1".to_string(),
            status: GameStatus::Finished,
            variant: "official".to_string(),
            language: "sowpods".to_string(),
            board_layout: "official".to_string(),
            turn_number: 5,
            current_seat: 0,
            winner_seat,
            final_bonus_seat,
            final_bonus_points,
            bag_count: 0,
            move_time_limit_seconds: 0,
            turn_started_at: "0".to_string(),
            participants,
            board: Vec::new(),
            racks: Vec::new(),
            moves: Vec::new(),
            messages: Vec::new(),
        }
    }

    #[test]
    fn in_progress_game_has_no_summary() {
        let mut game = finished_game(Some(0), None, None, vec![participant(0, "Alice", 10)]);
        game.status = GameStatus::Active;
        assert_eq!(finished_game_summary(&game), None);
    }

    #[test]
    fn names_the_winner_with_no_rack_bonus() {
        let game = finished_game(
            Some(1),
            None,
            None,
            vec![participant(0, "Alice", 5), participant(1, "Bob", 20)],
        );
        assert_eq!(
            finished_game_summary(&game),
            Some("Game over — Bob won!".to_string())
        );
    }

    #[test]
    fn a_tie_names_no_one() {
        let game = finished_game(
            None,
            None,
            None,
            vec![participant(0, "Alice", 10), participant(1, "Bob", 10)],
        );
        assert_eq!(
            finished_game_summary(&game),
            Some("Game over — it's a tie!".to_string())
        );
    }

    #[test]
    fn going_out_names_the_winner_and_the_rack_bonus() {
        let game = finished_game(
            Some(0),
            Some(0),
            Some(10),
            vec![participant(0, "Alice", 30), participant(1, "Bob", -10)],
        );
        assert_eq!(
            finished_game_summary(&game),
            Some(
                "Game over — Alice won! Alice went out and picked up a 10-point bonus from the other players' racks."
                    .to_string()
            )
        );
    }

    #[test]
    fn a_zero_point_bonus_is_not_worth_mentioning() {
        let game = finished_game(
            Some(0),
            Some(0),
            Some(0),
            vec![participant(0, "Alice", 30), participant(1, "Bob", 0)],
        );
        assert_eq!(
            finished_game_summary(&game),
            Some("Game over — Alice won!".to_string())
        );
    }
}
