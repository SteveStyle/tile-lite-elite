//! Formats the server's `last_activity_at` timestamps (seconds since the
//! Unix epoch, as a string — see `server-game::persistence::now_iso`) as a
//! short relative string like "3m ago" for the games list.

#[cfg(target_arch = "wasm32")]
fn now_epoch_seconds() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

#[cfg(not(target_arch = "wasm32"))]
fn now_epoch_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Falls back to the raw string if it isn't parseable, so an unexpected
/// server format degrades gracefully instead of panicking the UI.
pub fn format_relative_time(epoch_seconds_str: &str) -> String {
    let Ok(then) = epoch_seconds_str.parse::<u64>() else {
        return epoch_seconds_str.to_string();
    };
    let diff = now_epoch_seconds().saturating_sub(then);

    if diff < 10 {
        "just now".to_string()
    } else if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3_600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3_600)
    } else if diff < 604_800 {
        format!("{}d ago", diff / 86_400)
    } else {
        format!("{}w ago", diff / 604_800)
    }
}
