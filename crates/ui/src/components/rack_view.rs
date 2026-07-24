use crate::app::RackTileView;
use dioxus::prelude::*;
use std::collections::HashSet;

#[component]
#[allow(clippy::too_many_arguments)]
pub fn RackView(
    tiles: Vec<RackTileView>,
    can_stage_moves: bool,
    exchange_mode: bool,
    exchange_selected: HashSet<usize>,
    /// The active game's letter values/alphabet, for tile-face point
    /// values — see `BoardView`'s identical props.
    letter_values: [u8; rules_shared::MAX_ALPHABET_SIZE],
    alphabet: rules_shared::Alphabet,
    on_drag_start: EventHandler<usize>,
    on_drag_end: EventHandler<()>,
    on_drop_tile: EventHandler<usize>,
    on_click_tile: EventHandler<usize>,
    on_toggle_exchange_tile: EventHandler<usize>,
) -> Element {
    let tile_els = tiles.into_iter().map(|tile| {
        let is_marked_for_exchange = exchange_selected.contains(&tile.id);
        let mut class_name = if tile.is_used {
            "rack-tile rack-tile-used".to_string()
        } else {
            "rack-tile".to_string()
        };
        if is_marked_for_exchange {
            class_name.push_str(" rack-tile-selected");
        }
        let draggable = can_stage_moves && !exchange_mode && !tile.is_used;
        // In exchange mode only unused tiles are selectable. When staging,
        // both are clickable: an unused tile places onto the selected cell, a
        // used (already-placed) tile's greyed slot returns it to the rack —
        // `on_click_tile` in app.rs decides which by looking at the placements.
        let clickable = if exchange_mode {
            !tile.is_used
        } else {
            can_stage_moves
        };

        rsx! {
            div {
                key: "{tile.id}",
                class: "{class_name}",
                draggable: "{draggable}",
                ondragstart: move |_| {
                    if draggable {
                        on_drag_start.call(tile.id);
                    }
                },
                ondragend: move |_| on_drag_end.call(()),
                // Accepts drops so tiles dragged within the rack can reorder
                // it (see on_drop_tile in app.rs) — same "always call
                // prevent_default" reasoning as the board cells, otherwise
                // the browser never treats this as a valid drop target at
                // all and `ondrop` would never fire here.
                ondragover: move |event| {
                    event.prevent_default();
                },
                ondrop: move |event| {
                    event.prevent_default();
                    on_drop_tile.call(tile.id);
                },
                onclick: move |_| {
                    if !clickable {
                        return;
                    }
                    if exchange_mode {
                        on_toggle_exchange_tile.call(tile.id);
                    } else {
                        on_click_tile.call(tile.id);
                    }
                },
                // Mirrors `BoardView`'s `.tile-face` wrapper exactly — the
                // point value is positioned absolutely within it, and its
                // containing block needs to be this inner, padding-respecting
                // box (not `.rack-tile` itself) for the offset to land in the
                // same place relative to the rounded corner as it does on the
                // board. See `.rack-tile .tile-face` in main.css for why a
                // direct child wouldn't have worked.
                div { class: "tile-face",
                    span { class: "tile-letter", "{tile.display}" }
                    span { class: "tile-value", "{crate::tile_value::tile_point_value(&tile.tile, &letter_values, &alphabet)}" }
                }
            }
        }
    });

    rsx! {
        div { class: "rack-strip", {tile_els} }
    }
}
