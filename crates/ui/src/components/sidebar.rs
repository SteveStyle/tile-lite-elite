use api::{GameStatus, MoveRecordDto, ParticipantDto};
use dioxus::prelude::*;

#[component]
pub fn Sidebar(
    participants: Vec<ParticipantDto>,
    moves: Vec<MoveRecordDto>,
    current_seat: u8,
    status: GameStatus,
) -> Element {
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
                    span { "Seat {participant.seat_number}" }
                    span { "Score {participant.score}" }
                }
            }
        }
    });

    let history_items = moves.iter().map(|record| {
        rsx! {
            div { key: "{record.move_number}", class: "history-item",
                div { class: "history-row",
                    span { class: "history-badge", "#{record.move_number}" }
                    span { class: "history-type", "{record.move_type}" }
                }
                p { class: "history-copy", "{record.description}" }
            }
        }
    });

    rsx! {
        aside { class: "workspace-sidebar",
            section { class: "sidebar-card",
                p { class: "sidebar-kicker", "Match State" }
                h2 { "Seats" }
                div { class: "seat-list", {seat_cards} }
            }

            section { class: "sidebar-card",
                p { class: "sidebar-kicker", "Server Decisions" }
                h2 { "Move History" }
                if moves.is_empty() {
                    p { class: "empty-copy",
                        "No moves yet. Create a game, assign seats, then start from the authoritative server."
                    }
                } else {
                    div { class: "history-list", {history_items} }
                }
            }

            section { class: "sidebar-card sidebar-card-accent",
                p { class: "sidebar-kicker", "Contract" }
                h2 { "Client Role" }
                p { class: "empty-copy",
                    "Status: {status_label(&status)}. This UI renders API-shaped state and leaves legality, scoring, and persistence to the server."
                }
            }
        }
    }
}

fn seat_kind_label(kind: &api::SeatKind) -> &'static str {
    match kind {
        api::SeatKind::Human => "Human",
        api::SeatKind::Engine => "Engine",
    }
}

fn status_label(status: &GameStatus) -> &'static str {
    match status {
        GameStatus::Waiting => "Waiting",
        GameStatus::Active => "Active",
        GameStatus::Finished => "Finished",
    }
}
