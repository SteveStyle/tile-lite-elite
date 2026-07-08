use crate::app::RackTileView;
use api::RackDto;
use dioxus::prelude::*;

#[component]
pub fn RackView(
    rack: RackDto,
    tiles: Vec<RackTileView>,
    selected_rack_tile_id: Option<usize>,
    on_tile_click: EventHandler<usize>,
    can_stage_moves: bool,
) -> Element {
    let _ = rack;
    let tile_buttons = tiles.into_iter().map(|tile| {
        let class_name = if selected_rack_tile_id == Some(tile.id) {
            "rack-tile rack-tile-selected"
        } else if tile.is_used {
            "rack-tile rack-tile-used"
        } else {
            "rack-tile"
        };

        rsx! {
            button {
                key: "{tile.id}",
                class: "{class_name}",
                disabled: !can_stage_moves || tile.is_used,
                onclick: move |_| on_tile_click.call(tile.id),
                "{tile.display}"
            }
        }
    });

    rsx! {
        div { class: "rack-strip", {tile_buttons} }
    }
}
