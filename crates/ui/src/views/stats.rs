use dioxus::prelude::*;

use crate::components::rating_chart::RatingChart;

/// Rendered inside `RootApp`'s `modal-backdrop`/`modal-card` overlay (same
/// pattern as the invite-confirmation modal) — not a standalone page, so
/// unlike `ResetPassword` it doesn't bring its own `document::Link`.
#[component]
pub fn StatsView(
    server_url: String,
    player_id: Option<String>,
    token: Option<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut stats = use_signal(|| None::<api::PlayerStatsDto>);
    let mut history = use_signal(Vec::<api::RatingPointDto>::new);
    let mut error_message = use_signal(|| None::<String>);
    let mut is_loading = use_signal(|| false);
    let mut requested = use_signal(|| false);

    if let Some(player_id) = player_id.clone()
        && !requested()
    {
        requested.set(true);
        is_loading.set(true);
        let server_url = server_url.clone();
        let token = token.clone();
        spawn(async move {
            match crate::app::fetch_player_stats(&server_url, &player_id, token.as_deref()).await {
                Ok(fetched) => stats.set(Some(fetched)),
                Err(error) => error_message.set(Some(error)),
            }
            match crate::app::fetch_player_rating_history(&server_url, &player_id, token.as_deref())
                .await
            {
                Ok(fetched) => history.set(fetched),
                Err(error) => error_message.set(Some(error)),
            }
            is_loading.set(false);
        });
    }

    rsx! {
        div { class: "stats-view",
            div { class: "stats-view-header",
                h2 { class: "modal-title", "Your stats" }
                button {
                    class: "toggle-button toggle-button-muted",
                    onclick: move |_| on_close.call(()),
                    "Close"
                }
            }
            if let Some(error) = error_message() {
                p { class: "error-banner", "{error}" }
            } else if is_loading() && stats().is_none() {
                p { class: "empty-copy", "Loading stats…" }
            } else if let Some(stats) = stats() {
                div { class: "stats-grid",
                    StatsTile { label: "Rating", value: format!("{:.0}", stats.rating) }
                    StatsTile { label: "Games rated", value: stats.games_rated.to_string() }
                    StatsTile { label: "Wins", value: stats.wins.to_string() }
                    StatsTile { label: "Losses", value: stats.losses.to_string() }
                    StatsTile { label: "Ties", value: stats.ties.to_string() }
                    StatsTile { label: "Timeouts", value: stats.timeouts.to_string() }
                    StatsTile { label: "Resignations", value: stats.resignations.to_string() }
                    StatsTile { label: "Bingos", value: stats.bingo_count.to_string() }
                }
                RatingChart { points: history() }
            }
        }
    }
}

#[component]
fn StatsTile(label: String, value: String) -> Element {
    rsx! {
        div { class: "stats-tile",
            span { class: "stats-tile-label", "{label}" }
            span { class: "stats-tile-value", "{value}" }
        }
    }
}
