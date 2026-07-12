use crate::time_format::format_relative_time;
use api::{CreateSeatRequest, GameRelationship, GameStatus, GameSummaryDto, SeatClaim, SeatKind};
use dioxus::prelude::*;

const DEFAULT_ENGINE_ID: &str = "greedy-v1";
const DEFAULT_TIME_LIMIT_HOURS: u32 = 72;

/// The seat shape for a newly created game. Every variant is exactly two
/// seats today; see `crate::app::build_new_game_seats` for how each maps to
/// `CreateSeatRequest`s. These stay as one-click shortcuts alongside the
/// general seat-builder form below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewGameKind {
    VsEngine,
    VsHuman,
    EngineVsEngine,
}

/// What the seat-builder form emits on submit — already fully resolved into
/// `CreateSeatRequest`s (the "Me" seat's display name is filled in here,
/// since only this component knows the draft rows), so the caller just
/// POSTs it as-is.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomGameSubmission {
    pub seats: Vec<CreateSeatRequest>,
    pub move_time_limit_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdditionalSeatKind {
    Named,
    Open,
    Engine,
}

#[derive(Debug, Clone, PartialEq)]
struct AdditionalSeatDraft {
    kind: AdditionalSeatKind,
    /// The invitee's display name for `Named` rows; ignored otherwise.
    name: String,
}

#[component]
pub fn GamesPanel(
    summaries: Vec<GameSummaryDto>,
    selected_id: Option<String>,
    is_loading: bool,
    my_display_name: Option<String>,
    on_select: EventHandler<String>,
    on_new_game: EventHandler<NewGameKind>,
    on_custom_new_game: EventHandler<CustomGameSubmission>,
    on_accept_invitation: EventHandler<String>,
    on_reject_invitation: EventHandler<String>,
    on_refresh: EventHandler<()>,
) -> Element {
    let mut show_builder = use_signal(|| false);
    let mut additional_seats = use_signal(Vec::<AdditionalSeatDraft>::new);
    let mut time_limit_hours = use_signal(|| DEFAULT_TIME_LIMIT_HOURS);

    let can_create = my_display_name.is_some();

    let (your_turn, rest): (Vec<_>, Vec<_>) = summaries
        .iter()
        .cloned()
        .partition(|s| s.relationship == GameRelationship::YourTurn);
    let (participant, rest): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|s| s.relationship == GameRelationship::Participant);
    let (invited_named, invited_open): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|s| s.relationship == GameRelationship::InvitedByName);

    let section = |title: &'static str, rows: Vec<GameSummaryDto>, show_invite_actions: bool| {
        if rows.is_empty() {
            return rsx! {};
        }
        let row_elements = rows.into_iter().map(|summary| {
            game_row(&summary, selected_id.as_deref(), show_invite_actions, on_select, on_accept_invitation, on_reject_invitation)
        });
        rsx! {
            div { class: "games-list-section",
                h3 { class: "games-list-section-title", "{title}" }
                div { class: "games-list", {row_elements} }
            }
        }
    };

    let seat_rows = additional_seats().into_iter().enumerate().map(|(index, draft)| {
        let removable_index = index;
        rsx! {
            div { key: "{index}", class: "seat-draft-row",
                span { class: "seat-draft-kind", "{seat_kind_label(draft.kind)}" }
                if draft.kind == AdditionalSeatKind::Named {
                    input {
                        class: "seat-draft-name-input",
                        placeholder: "Display name to invite",
                        value: "{draft.name}",
                        oninput: move |event| {
                            additional_seats.with_mut(|seats| {
                                if let Some(seat) = seats.get_mut(index) {
                                    seat.name = event.value();
                                }
                            });
                        },
                    }
                }
                button {
                    class: "toggle-button toggle-button-muted seat-draft-remove",
                    onclick: move |_| {
                        additional_seats.with_mut(|seats| {
                            if removable_index < seats.len() {
                                seats.remove(removable_index);
                            }
                        });
                    },
                    "Remove"
                }
            }
        }
    });

    let engine_count = additional_seats()
        .iter()
        .filter(|seat| seat.kind == AdditionalSeatKind::Engine)
        .count();
    let can_submit_builder = my_display_name.is_some()
        && additional_seats()
            .iter()
            .all(|seat| seat.kind != AdditionalSeatKind::Named || !seat.name.trim().is_empty());

    rsx! {
        aside { class: "games-panel",
            div { class: "games-panel-header",
                h2 { "Games" }
                button {
                    class: "toggle-button toggle-button-muted",
                    disabled: is_loading,
                    onclick: move |_| on_refresh.call(()),
                    "Refresh"
                }
            }
            if can_create {
                div { class: "games-panel-new-game",
                    span { class: "games-panel-new-game-label", "New game:" }
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| on_new_game.call(NewGameKind::VsEngine),
                        "vs Engine"
                    }
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| on_new_game.call(NewGameKind::VsHuman),
                        "vs Human"
                    }
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| on_new_game.call(NewGameKind::EngineVsEngine),
                        "Engine vs Engine"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        disabled: is_loading,
                        onclick: move |_| {
                            let opening = !show_builder();
                            show_builder.set(opening);
                            if opening {
                                additional_seats.set(vec![AdditionalSeatDraft { kind: AdditionalSeatKind::Open, name: String::new() }]);
                                time_limit_hours.set(DEFAULT_TIME_LIMIT_HOURS);
                            }
                        },
                        if show_builder() { "Cancel" } else { "Custom game..." }
                    }
                }

                if show_builder() {
                    div { class: "game-builder",
                        p { class: "composer-copy", "You (seat 1, creator)" }
                        div { class: "seat-draft-list", {seat_rows} }
                        div { class: "game-builder-add-row",
                            button {
                                class: "toggle-button toggle-button-muted",
                                onclick: move |_| {
                                    additional_seats.with_mut(|seats| seats.push(AdditionalSeatDraft { kind: AdditionalSeatKind::Named, name: String::new() }));
                                },
                                "+ Invite by name"
                            }
                            button {
                                class: "toggle-button toggle-button-muted",
                                onclick: move |_| {
                                    additional_seats.with_mut(|seats| seats.push(AdditionalSeatDraft { kind: AdditionalSeatKind::Open, name: String::new() }));
                                },
                                "+ Open seat"
                            }
                            button {
                                class: "toggle-button toggle-button-muted",
                                onclick: move |_| {
                                    additional_seats.with_mut(|seats| seats.push(AdditionalSeatDraft { kind: AdditionalSeatKind::Engine, name: String::new() }));
                                },
                                "+ Engine"
                            }
                        }
                        div { class: "game-builder-time-limit",
                            label { "Time per move (hours): " }
                            input {
                                r#type: "number",
                                min: "1",
                                class: "seat-draft-name-input",
                                value: "{time_limit_hours()}",
                                oninput: move |event| {
                                    if let Ok(hours) = event.value().parse::<u32>() {
                                        if hours > 0 {
                                            time_limit_hours.set(hours);
                                        }
                                    }
                                },
                            }
                        }
                        button {
                            class: "toggle-button",
                            disabled: is_loading || !can_submit_builder,
                            onclick: move |_| {
                                let Some(my_name) = my_display_name.clone() else { return };
                                let mut seats = vec![CreateSeatRequest {
                                    kind: SeatKind::Human,
                                    display_name: my_name,
                                    engine_id: None,
                                    claim: Some(SeatClaim::Creator),
                                }];
                                let mut engine_index = 0;
                                for draft in additional_seats() {
                                    seats.push(match draft.kind {
                                        AdditionalSeatKind::Named => CreateSeatRequest {
                                            kind: SeatKind::Human,
                                            display_name: draft.name.trim().to_string(),
                                            engine_id: None,
                                            claim: Some(SeatClaim::Named { display_name: draft.name.trim().to_string() }),
                                        },
                                        AdditionalSeatKind::Open => CreateSeatRequest {
                                            kind: SeatKind::Human,
                                            display_name: "Open seat".to_string(),
                                            engine_id: None,
                                            claim: Some(SeatClaim::Open),
                                        },
                                        AdditionalSeatKind::Engine => {
                                            engine_index += 1;
                                            let label = if engine_count > 1 { format!("Greedy {engine_index}") } else { "Greedy".to_string() };
                                            CreateSeatRequest {
                                                kind: SeatKind::Engine,
                                                display_name: label,
                                                engine_id: Some(DEFAULT_ENGINE_ID.to_string()),
                                                claim: None,
                                            }
                                        }
                                    });
                                }
                                on_custom_new_game.call(CustomGameSubmission {
                                    seats,
                                    move_time_limit_seconds: Some(time_limit_hours() as u64 * 3600),
                                });
                                show_builder.set(false);
                            },
                            "Create game"
                        }
                    }
                }
            } else {
                p { class: "empty-copy", "Sign in to create or join games." }
            }

            if summaries.is_empty() {
                p { class: "empty-copy", "No games yet." }
            } else {
                {section("Your turn", your_turn, false)}
                {section("Your games", participant, false)}
                {section("Invited by name", invited_named, true)}
                {section("Open invitations", invited_open, true)}
            }
        }
    }
}

fn game_row(
    summary: &GameSummaryDto,
    selected_id: Option<&str>,
    show_invite_actions: bool,
    on_select: EventHandler<String>,
    on_accept_invitation: EventHandler<String>,
    on_reject_invitation: EventHandler<String>,
) -> Element {
    let is_selected = selected_id == Some(summary.id.as_str());
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
    let select_id = game_id.clone();
    let invitation_id = summary.invitation_id.clone();

    rsx! {
        div { key: "{summary.id}", class: "game-row-wrapper",
            button {
                class: "{row_class}",
                onclick: move |_| on_select.call(select_id.clone()),
                div { class: "game-row-top",
                    span { class: "{badge_class}", "{status_label(&summary.status)}" }
                    span { class: "game-row-time", "{relative_time}" }
                }
                p { class: "game-row-participants", "{participants_label}" }
            }
            if show_invite_actions {
                if let Some(invitation_id) = invitation_id {
                    div { class: "game-row-invite-actions",
                        button {
                            class: "toggle-button",
                            onclick: {
                                let invitation_id = invitation_id.clone();
                                move |_| on_accept_invitation.call(invitation_id.clone())
                            },
                            "Accept"
                        }
                        if summary.relationship == GameRelationship::InvitedByName {
                            button {
                                class: "toggle-button toggle-button-muted",
                                onclick: {
                                    let invitation_id = invitation_id.clone();
                                    move |_| on_reject_invitation.call(invitation_id.clone())
                                },
                                "Reject"
                            }
                        }
                    }
                }
            }
        }
    }
}

fn seat_kind_label(kind: AdditionalSeatKind) -> &'static str {
    match kind {
        AdditionalSeatKind::Named => "Invite by name:",
        AdditionalSeatKind::Open => "Open seat (any player may claim)",
        AdditionalSeatKind::Engine => "Engine",
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
