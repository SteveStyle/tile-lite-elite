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
pub fn format_relative_time(epoch_seconds: i64) -> String {
    let diff = now_epoch_seconds().saturating_sub(epoch_seconds.max(0) as u64);

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

/// How long is left on the current turn before the seat gets auto-retired
/// (see `GameSession::apply_move_timeout` on the server), given when the
/// turn started and the game's move-time-limit — both in the same "seconds
/// since the Unix epoch" string format as `format_relative_time`. Shown as
/// combined days+hours while more than an hour remains, then switches to
/// minutes-only for the final hour (rounded up, so any time still left
/// reads as at least "1m left" rather than a misleading "0m left").
pub fn format_time_remaining(turn_started_at: i64, move_time_limit_seconds: u64) -> String {
    let deadline = turn_started_at.max(0) as u64 + move_time_limit_seconds;
    let now = now_epoch_seconds();
    if now >= deadline {
        return "overdue".to_string();
    }
    let remaining = deadline - now;

    if remaining <= 3_600 {
        let minutes = remaining.div_ceil(60);
        format!("{minutes}m left")
    } else {
        let days = remaining / 86_400;
        let hours = (remaining % 86_400) / 3_600;
        if days > 0 {
            format!("{days}d {hours}h left")
        } else {
            format!("{hours}h left")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started_seconds_ago(seconds_ago: u64) -> i64 {
        (now_epoch_seconds() - seconds_ago) as i64
    }

    #[test]
    fn shows_days_and_hours_above_one_hour_remaining() {
        // 72h limit, 20h elapsed -> 52h (2d 4h) remaining.
        let started = started_seconds_ago(20 * 3_600);
        assert_eq!(format_time_remaining(started, 72 * 3_600), "2d 4h left");
    }

    #[test]
    fn shows_hours_only_when_under_a_day_remains() {
        // 72h limit, 68h elapsed -> 4h remaining.
        let started = started_seconds_ago(68 * 3_600);
        assert_eq!(format_time_remaining(started, 72 * 3_600), "4h left");
    }

    #[test]
    fn switches_to_minutes_at_exactly_one_hour_remaining() {
        let started = started_seconds_ago(71 * 3_600);
        assert_eq!(format_time_remaining(started, 72 * 3_600), "60m left");
    }

    #[test]
    fn shows_minutes_under_one_hour_remaining() {
        let started = started_seconds_ago(72 * 3_600 - 30 * 60);
        assert_eq!(format_time_remaining(started, 72 * 3_600), "30m left");
    }

    #[test]
    fn rounds_up_so_any_remaining_time_shows_at_least_one_minute() {
        let started = started_seconds_ago(72 * 3_600 - 10);
        assert_eq!(format_time_remaining(started, 72 * 3_600), "1m left");
    }

    #[test]
    fn reports_overdue_once_the_deadline_has_passed() {
        let started = started_seconds_ago(73 * 3_600);
        assert_eq!(format_time_remaining(started, 72 * 3_600), "overdue");
    }
}
