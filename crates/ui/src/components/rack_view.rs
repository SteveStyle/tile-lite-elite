use crate::app::RackTileView;
use dioxus::prelude::*;

#[component]
pub fn RackView(
    tiles: Vec<RackTileView>,
    can_stage_moves: bool,
    on_drag_start: EventHandler<usize>,
    on_drag_end: EventHandler<()>,
) -> Element {
    let tile_els = tiles.into_iter().map(|tile| {
        let class_name = if tile.is_used {
            "rack-tile rack-tile-used"
        } else {
            "rack-tile"
        };
        let draggable = can_stage_moves && !tile.is_used;

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
                "{tile.display}"
            }
        }
    });

    rsx! {
        div { class: "rack-strip", {tile_els} }
    }
}
