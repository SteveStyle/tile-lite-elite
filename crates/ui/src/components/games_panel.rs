use crate::time_format::format_relative_time;
use api::{GameStatus, GameSummaryDto};
use dioxus::prelude::*;

#[component]
pub fn GamesPanel(
    summaries: Vec<GameSummaryDto>,
    selected_id: Option<String>,
    is_loading: bool,
    on_select: EventHandler<String>,
    on_new_game: EventHandler<()>,
    on_refresh: EventHandler<()>,
) -> Element {
    let rows = summaries.iter().cloned().map(|summary| {
        let is_selected = selected_id.as_deref() == Some(summary.id.as_str());
        let row_class = if is_selected {
            "game-row game-row-active"
        } else {
            "game-row"
        };
        let badge_class = format!("game-status-badge game-status-{}", status_slug(&summary.status));
        let participants_label = if summary.participants.is_empty() {
            "No seats yet".to_string()
        } else {
            summary
                .participants
                .iter()
                .map(|participant| participant.display_name.clone())
                .collect::<Vec<_>>()
                .join(" vs ")
        };
        let relative_time = format_relative_time(&summary.last_activity_at);
        let game_id = summary.id.clone();

        rsx! {
            button {
                key: "{summary.id}",
                class: "{row_class}",
                onclick: move |_| on_select.call(game_id.clone()),
                div { class: "game-row-top",
                    span { class: "{badge_class}", "{status_label(&summary.status)}" }
                    span { class: "game-row-time", "{relative_time}" }
                }
                p { class: "game-row-participants", "{participants_label}" }
            }
        }
    });

    rsx! {
        aside { class: "games-panel",
            div { class: "games-panel-header",
                h2 { "Games" }
                div { class: "games-panel-actions",
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| on_new_game.call(()),
                        "New"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        disabled: is_loading,
                        onclick: move |_| on_refresh.call(()),
                        "Refresh"
                    }
                }
            }
            if summaries.is_empty() {
                p { class: "empty-copy", "No games yet. Create one to begin." }
            } else {
                div { class: "games-list", {rows} }
            }
        }
    }
}

fn status_label(status: &GameStatus) -> &'static str {
    match status {
        GameStatus::Waiting => "Waiting",
        GameStatus::Active => "Active",
        GameStatus::Finished => "Finished",
    }
}

fn status_slug(status: &GameStatus) -> &'static str {
    match status {
        GameStatus::Waiting => "waiting",
        GameStatus::Active => "active",
        GameStatus::Finished => "finished",
    }
}
