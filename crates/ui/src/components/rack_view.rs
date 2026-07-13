use crate::app::RackTileView;
use dioxus::prelude::*;
use std::collections::HashSet;

#[component]
pub fn RackView(
    tiles: Vec<RackTileView>,
    can_stage_moves: bool,
    exchange_mode: bool,
    exchange_selected: HashSet<usize>,
    on_drag_start: EventHandler<usize>,
    on_drag_end: EventHandler<()>,
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
        let clickable = !tile.is_used && (exchange_mode || can_stage_moves);

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
                span { class: "tile-letter", "{tile.display}" }
                span { class: "tile-value", "{crate::tile_value::tile_point_value(&tile.tile)}" }
            }
        }
    });

    rsx! {
        div { class: "rack-strip", {tile_els} }
    }
}
