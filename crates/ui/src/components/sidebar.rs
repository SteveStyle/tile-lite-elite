use api::{MoveRecordDto, ParticipantDto};
use dioxus::prelude::*;

/// Only the most recent moves are shown — this is a live-play glance, not
/// a full game log.
const RECENT_MOVES_LIMIT: usize = 8;

#[component]
pub fn Sidebar(participants: Vec<ParticipantDto>, moves: Vec<MoveRecordDto>, current_seat: u8) -> Element {
    let seat_cards = participants.iter().map(|participant| {
        let class_name = if participant.seat_number == current_seat {
            "seat-card seat-card-active"
        } else {
            "seat-card"
        };

        rsx! {
            div { key: "{participant.seat_number}", class: "{class_name}",
                div { class: "seat-heading",
                    span { class: "seat-name", "{participant.display_name}" }
                    span { class: "seat-kind", "{seat_kind_label(&participant.kind)}" }
                }
                div { class: "seat-meta",
                    span { "Score {participant.score}" }
                }
            }
        }
    });

    let recent_moves = moves.iter().rev().take(RECENT_MOVES_LIMIT).map(|record| {
        let player_name = participant_name(&participants, record.seat_number);
        let is_scoring_play = record.move_type == "place";
        let row_class = if is_scoring_play {
            "recent-move-row"
        } else {
            "recent-move-row recent-move-row-muted"
        };

        let score_cell = if is_scoring_play {
            rsx! {
                span { class: "recent-move-score", "{record.score_delta}" }
            }
        } else {
            rsx! {
                span { class: "recent-move-score" }
            }
        };

        let detail_cell = if is_scoring_play {
            let word = record.main_word.clone().unwrap_or_default();
            let url = format!(
                "https://www.collinsdictionary.com/dictionary/english/{}",
                word.to_lowercase()
            );
            rsx! {
                a {
                    class: "recent-move-word",
                    href: "{url}",
                    target: "_blank",
                    rel: "noopener noreferrer",
                    "{word}"
                }
            }
        } else {
            let note = action_note(record);
            rsx! {
                span { class: "recent-move-note", "{note}" }
            }
        };

        rsx! {
            div { key: "{record.move_number}", class: "{row_class}",
                span { class: "recent-move-player", "{player_name}" }
                {score_cell}
                {detail_cell}
            }
        }
    });

    rsx! {
        aside { class: "workspace-sidebar",
            section { class: "sidebar-card",
                div { class: "seat-list", {seat_cards} }
            }

            section { class: "sidebar-card",
                h2 { "Recent Moves" }
                if moves.is_empty() {
                    p { class: "empty-copy", "No moves yet." }
                } else {
                    div { class: "recent-move-list", {recent_moves} }
                }
            }
        }
    }
}

fn participant_name(participants: &[ParticipantDto], seat_number: u8) -> String {
    participants
        .iter()
        .find(|participant| participant.seat_number == seat_number)
        .map(|participant| participant.display_name.clone())
        .unwrap_or_else(|| format!("Seat {seat_number}"))
}

/// Pass/exchange/resign rows have no word or score, so this builds the
/// short note shown in that slot instead. Exchange's tile count is parsed
/// out of the existing `description` text (e.g. "Alice exchanged 3 tiles")
/// rather than adding a new field, since the server already formats it.
fn action_note(record: &MoveRecordDto) -> String {
    match record.move_type.as_str() {
        "pass" => "passed".to_string(),
        "resign" => "resigned".to_string(),
        "exchange" => {
            let count = record
                .description
                .split_whitespace()
                .find_map(|token| token.parse::<u32>().ok())
                .unwrap_or(0);
            format!("exchanged {count} letter{}", if count == 1 { "" } else { "s" })
        }
        other => other.to_string(),
    }
}

fn seat_kind_label(kind: &api::SeatKind) -> &'static str {
    match kind {
        api::SeatKind::Human => "Human",
        api::SeatKind::Engine => "Engine",
    }
}
