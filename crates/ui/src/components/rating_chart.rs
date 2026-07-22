use dioxus::prelude::*;

const VIEW_WIDTH: f64 = 400.0;
const VIEW_HEIGHT: f64 = 160.0;
const PADDING: f64 = 16.0;

/// A rating-over-time line chart, hand-rolled as inline SVG — this wasm
/// build has no charting library today and this is simple enough not to
/// need one. Plots points evenly spaced by index (not by actual elapsed
/// time between games), which is enough to show trend/shape without
/// needing a date axis.
#[component]
pub fn RatingChart(points: Vec<api::RatingPointDto>) -> Element {
    if points.len() < 2 {
        return rsx! {
            p { class: "stats-chart-empty", "Play a few more rated games to see your rating graph." }
        };
    }

    let ratings: Vec<f64> = points.iter().map(|p| p.rating_after).collect();
    let min_rating = ratings.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_rating = ratings.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    // A flat series (every game exactly cancelled out, e.g. an all-draws
    // streak) would divide by zero below without this floor.
    let range = (max_rating - min_rating).max(1.0);

    let plot_width = VIEW_WIDTH - PADDING * 2.0;
    let plot_height = VIEW_HEIGHT - PADDING * 2.0;
    let last_index = (points.len() - 1) as f64;

    let coords: Vec<(f64, f64)> = ratings
        .iter()
        .enumerate()
        .map(|(i, &rating)| {
            let x = PADDING + (i as f64 / last_index) * plot_width;
            let y = PADDING + (1.0 - (rating - min_rating) / range) * plot_height;
            (x, y)
        })
        .collect();

    let polyline_points = coords
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect::<Vec<_>>()
        .join(" ");

    let first_rating = ratings[0];
    let last_rating = ratings[ratings.len() - 1];

    rsx! {
        div { class: "rating-chart",
            svg {
                view_box: "0 0 {VIEW_WIDTH} {VIEW_HEIGHT}",
                preserve_aspect_ratio: "none",
                polyline {
                    points: "{polyline_points}",
                    fill: "none",
                    stroke: "var(--clay)",
                    "stroke-width": "2.5",
                }
                for (x , y) in coords.iter() {
                    circle { cx: "{x:.1}", cy: "{y:.1}", r: "3.5", fill: "var(--oak)" }
                }
            }
            div { class: "rating-chart-range",
                span { "{first_rating:.0}" }
                span { "{last_rating:.0}" }
            }
        }
    }
}
