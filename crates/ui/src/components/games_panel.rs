use crate::edition_label::edition_label;
use crate::time_format::format_relative_time;
use api::{
    ChatMessageDto, CreateSeatRequest, GameRelationship, GameStateDto, GameStatus, GameSummaryDto,
    MoveRecordDto, ParticipantDto, SeatClaim, SeatInvitationStatus, SeatKind,
};
use dioxus::prelude::*;
use std::collections::HashMap;

const DEFAULT_ENGINE_ID: &str = "greedy-v1";
const DEFAULT_TIME_LIMIT_HOURS: u32 = 72;

/// One-click starting shapes for the draft table below — each seeds
/// `include_creator`/`additional_seats` with a starting roster that's still
/// fully editable before you click Invite (see `preset_draft`), including
/// the edition picker — every preset just prepopulates a starting point,
/// it doesn't restrict what the draft can become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewGameKind {
    VsEngine,
    VsFriend,
    VsHuman,
    EngineVsEngine,
}

/// What the seat-builder table emits on submit — already fully resolved
/// into `CreateSeatRequest`s (the "you" seat's display name is filled in
/// here, since only this component knows the draft rows), so the caller
/// just POSTs it as-is.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomGameSubmission {
    pub seats: Vec<CreateSeatRequest>,
    pub move_time_limit_seconds: Option<u64>,
    /// The edition to create the game under (e.g. "official", "wordfeud",
    /// "north_american") — `None` lets the server fall back to its own
    /// default ("official").
    pub variant: Option<String>,
    /// True when every seat is already resolved (no invitation left to wait
    /// on) — the roster the "Start" label (as opposed to "Invite") promised.
    /// The caller should immediately call the start endpoint too, rather
    /// than leaving the game sitting in `Waiting` behind a second,
    /// redundant "Start" click.
    pub start_immediately: bool,
}

/// Adding a seat to an already-created `Waiting` game — the post-creation
/// counterpart to `CustomGameSubmission`, one seat at a time instead of a
/// whole roster, and never sends an invitation itself (see `add_seat_row`).
#[derive(Debug, Clone, PartialEq)]
pub struct AddSeatSubmission {
    pub game_id: String,
    pub kind: SeatKind,
    pub display_name: String,
    pub engine_id: Option<String>,
    pub claim: Option<SeatClaim>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdditionalSeatKind {
    Named,
    Open,
    Email,
    Engine,
}

#[derive(Debug, Clone, PartialEq)]
struct AdditionalSeatDraft {
    kind: AdditionalSeatKind,
    /// The invitee's display name for `Named` rows, or the join-link
    /// address for `Email` rows; ignored otherwise.
    name: String,
}

/// The starting roster for each quick-preset button: whether "you" are
/// seated, plus the other seats. Pure and separate from `build_seats` so
/// both halves (seeding the draft, resolving it to a request) are testable
/// without a running component.
fn preset_draft(kind: NewGameKind) -> (bool, Vec<AdditionalSeatDraft>) {
    match kind {
        NewGameKind::VsEngine => (
            true,
            vec![AdditionalSeatDraft {
                kind: AdditionalSeatKind::Engine,
                name: String::new(),
            }],
        ),
        NewGameKind::VsFriend => (
            true,
            vec![AdditionalSeatDraft {
                kind: AdditionalSeatKind::Named,
                name: String::new(),
            }],
        ),
        NewGameKind::VsHuman => (
            true,
            vec![AdditionalSeatDraft {
                kind: AdditionalSeatKind::Open,
                name: String::new(),
            }],
        ),
        NewGameKind::EngineVsEngine => (
            false,
            vec![
                AdditionalSeatDraft {
                    kind: AdditionalSeatKind::Engine,
                    name: String::new(),
                },
                AdditionalSeatDraft {
                    kind: AdditionalSeatKind::Engine,
                    name: String::new(),
                },
            ],
        ),
    }
}

/// One row of the draft roster as actually displayed — "you" (the creator)
/// interleaved with the other seats at whatever position `creator_position`
/// puts them, rather than always pinned first. `Seat` carries its index
/// into `additional_seats` so editing/removing can address it directly.
#[derive(Debug, Clone, PartialEq)]
enum DraftRow {
    You,
    Seat(usize, AdditionalSeatDraft),
}

fn draft_rows(
    include_creator: bool,
    creator_position: usize,
    additional: &[AdditionalSeatDraft],
) -> Vec<DraftRow> {
    let mut rows: Vec<DraftRow> = additional
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, draft)| DraftRow::Seat(index, draft))
        .collect();
    if include_creator {
        rows.insert(creator_position.min(rows.len()), DraftRow::You);
    }
    rows
}

/// Swaps two display positions in the drafted roster (an up/down button in
/// the draft table) and reports back the only two pieces of state that
/// actually need to change — `draft_rows` above is always recomputed fresh
/// from `creator_position`/`additional_seats`, so this doesn't touch
/// `include_creator` at all; "you" stays included, just possibly at a new
/// position. Out-of-range indices (nothing to swap, e.g. already at an
/// edge) are a no-op.
fn swap_draft_rows(
    include_creator: bool,
    creator_position: usize,
    additional: &[AdditionalSeatDraft],
    a: usize,
    b: usize,
) -> (usize, Vec<AdditionalSeatDraft>) {
    let mut rows = draft_rows(include_creator, creator_position, additional);
    if a < rows.len() && b < rows.len() {
        rows.swap(a, b);
    }
    let new_creator_position = rows
        .iter()
        .position(|row| matches!(row, DraftRow::You))
        .unwrap_or(0);
    let new_additional = rows
        .into_iter()
        .filter_map(|row| match row {
            DraftRow::Seat(_, draft) => Some(draft),
            DraftRow::You => None,
        })
        .collect();
    (new_creator_position, new_additional)
}

/// Seeds the draft signals from a preset and opens the draft view. A plain
/// function (rather than a closure captured by multiple buttons) sidesteps
/// the `FnMut`-borrow ambiguity of sharing one closure across several
/// `onclick` handlers — each button just calls this directly with its own
/// `NewGameKind` and the (`Copy`) signals.
fn start_draft(
    kind: NewGameKind,
    mut include_creator: Signal<bool>,
    mut creator_position: Signal<usize>,
    mut additional_seats: Signal<Vec<AdditionalSeatDraft>>,
    mut time_limit_hours: Signal<u32>,
    mut drafting: Signal<bool>,
    mut variant: Signal<String>,
) {
    let (creator, seats) = preset_draft(kind);
    include_creator.set(creator);
    creator_position.set(0);
    additional_seats.set(seats);
    time_limit_hours.set(DEFAULT_TIME_LIMIT_HOURS);
    variant.set("official".to_string());
    drafting.set(true);
}

/// Resolves a draft roster into `CreateSeatRequest`s ready to POST — the
/// creator's own seat lands at `creator_position` among `additional`
/// (clamped to a valid index) rather than always first, so a draft where
/// "you" was moved down actually submits with that turn order.
fn build_seats(
    include_creator: bool,
    creator_position: usize,
    additional: &[AdditionalSeatDraft],
    my_display_name: Option<&str>,
) -> Vec<CreateSeatRequest> {
    let creator_seat = || CreateSeatRequest {
        kind: SeatKind::Human,
        display_name: my_display_name.unwrap_or("Player 1").to_string(),
        engine_id: None,
        claim: Some(SeatClaim::Creator),
    };
    let insert_at = creator_position.min(additional.len());
    let mut seats = Vec::new();
    if include_creator && insert_at == 0 {
        seats.push(creator_seat());
    }
    let engine_count = additional
        .iter()
        .filter(|seat| seat.kind == AdditionalSeatKind::Engine)
        .count();
    let mut engine_index = 0;
    for (index, draft) in additional.iter().enumerate() {
        seats.push(match draft.kind {
            AdditionalSeatKind::Named => CreateSeatRequest {
                kind: SeatKind::Human,
                display_name: draft.name.trim().to_string(),
                engine_id: None,
                claim: Some(SeatClaim::Named {
                    display_name: draft.name.trim().to_string(),
                }),
            },
            AdditionalSeatKind::Open => CreateSeatRequest {
                kind: SeatKind::Human,
                display_name: "Open seat".to_string(),
                engine_id: None,
                claim: Some(SeatClaim::Open),
            },
            AdditionalSeatKind::Email => CreateSeatRequest {
                kind: SeatKind::Human,
                display_name: draft.name.trim().to_string(),
                engine_id: None,
                claim: Some(SeatClaim::Email {
                    email: draft.name.trim().to_string(),
                }),
            },
            AdditionalSeatKind::Engine => {
                engine_index += 1;
                let label = if engine_count > 1 {
                    format!("Greedy {engine_index}")
                } else {
                    "Greedy".to_string()
                };
                CreateSeatRequest {
                    kind: SeatKind::Engine,
                    display_name: label,
                    engine_id: Some(DEFAULT_ENGINE_ID.to_string()),
                    claim: None,
                }
            }
        });
        if include_creator && index + 1 == insert_at {
            seats.push(creator_seat());
        }
    }
    seats
}

#[component]
#[allow(clippy::too_many_arguments)]
pub fn GamesPanel(
    server_url: String,
    token: Option<String>,
    summaries: Vec<GameSummaryDto>,
    selected_id: Option<String>,
    current_game: Option<GameStateDto>,
    viewer_player_id: Option<String>,
    /// game_id -> the `created_at` of the last chat message this device has
    /// seen for that game — see `crate::local_storage::StoredChatWatermarks`.
    /// Used only to decide whether to show the unread-mail icon in the list.
    chat_watermarks: HashMap<String, i64>,
    is_loading: bool,
    my_display_name: Option<String>,
    can_start: bool,
    on_select: EventHandler<String>,
    on_start: EventHandler<()>,
    on_send_chat: EventHandler<String>,
    on_custom_new_game: EventHandler<CustomGameSubmission>,
    on_accept_invitation: EventHandler<String>,
    on_reject_invitation: EventHandler<String>,
    on_remove_game: EventHandler<String>,
    on_reorder_seats: EventHandler<(u8, u8)>,
    on_send_invitation: EventHandler<u8>,
    on_remove_seat: EventHandler<u8>,
    on_withdraw_seat: EventHandler<u8>,
    on_force_resign: EventHandler<u8>,
    on_add_seat: EventHandler<AddSeatSubmission>,
    on_refresh: EventHandler<()>,
) -> Element {
    let mut drafting = use_signal(|| false);
    let chat_draft = use_signal(String::new);
    let mut include_creator = use_signal(|| true);
    let mut creator_position = use_signal(|| 0usize);
    let mut additional_seats = use_signal(Vec::<AdditionalSeatDraft>::new);
    let mut time_limit_hours = use_signal(|| DEFAULT_TIME_LIMIT_HOURS);
    let mut variant = use_signal(|| "official".to_string());
    // Owned here (see `player_table`/`add_seat_row`'s own doc comments on
    // why they can't create these themselves) — reset per `GamesPanel`
    // render, not per row, but since only one row's detail view is ever
    // shown at a time that's not observably different from being per-row.
    let confirm_seat_action = use_signal(|| None::<SeatConfirmAction>);
    let add_seat_kind = use_signal(|| AdditionalSeatKind::Named);
    let add_seat_name = use_signal(String::new);

    let can_create = my_display_name.is_some();

    let (your_turn, rest): (Vec<_>, Vec<_>) = summaries
        .iter()
        .cloned()
        .partition(|s| s.relationship == GameRelationship::YourTurn);
    // A `Creator` game (e.g. Engine vs Engine, where you hold no seat) is
    // grouped alongside seated participant games — same section, since both
    // mean "yours to watch/manage," just with or without a rack.
    let (participant, rest): (Vec<_>, Vec<_>) = rest.into_iter().partition(|s| {
        matches!(
            s.relationship,
            GameRelationship::Participant | GameRelationship::Creator
        )
    });
    let (invited_named, invited_open): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|s| s.relationship == GameRelationship::InvitedByName);

    let section = {
        let current_game = current_game.clone();
        let viewer_player_id = viewer_player_id.clone();
        let chat_watermarks = chat_watermarks.clone();
        let server_url = server_url.clone();
        let token = token.clone();
        move |title: &'static str, rows: Vec<GameSummaryDto>, show_invite_actions: bool| {
            if rows.is_empty() {
                return rsx! {};
            }
            let row_elements = rows.into_iter().map(|summary| {
                let has_unread = has_unread_chat(&summary, &chat_watermarks);
                game_row(
                    &summary,
                    selected_id.as_deref(),
                    show_invite_actions,
                    has_unread,
                    current_game.as_ref(),
                    viewer_player_id.as_deref(),
                    can_start,
                    is_loading,
                    &server_url,
                    token.as_deref(),
                    drafting,
                    chat_draft,
                    confirm_seat_action,
                    add_seat_kind,
                    add_seat_name,
                    on_select,
                    on_start,
                    on_send_chat,
                    on_accept_invitation,
                    on_reject_invitation,
                    on_remove_game,
                    on_reorder_seats,
                    on_send_invitation,
                    on_remove_seat,
                    on_withdraw_seat,
                    on_force_resign,
                    on_add_seat,
                )
            });
            rsx! {
                div { class: "games-list-section",
                    h3 { class: "games-list-section-title", "{title}" }
                    div { class: "games-list", {row_elements} }
                }
            }
        }
    };

    // "You" interleaved with the other seats at whatever position you've
    // been moved to — see `draft_rows`. Reorder buttons work purely by
    // swapping display positions and reading the result back into
    // `creator_position`/`additional_seats`, so they apply uniformly to
    // every row, "you" included, regardless of which preset button opened
    // this draft (even a one-click "vs Engine"/"Bot Showdown" game gets a
    // chance to reorder here, since it's the one screen every game — not
    // just ones waiting on an invitation — passes through before it's
    // created).
    let rows_for_render = draft_rows(include_creator(), creator_position(), &additional_seats());
    let last_row_position = rows_for_render.len().saturating_sub(1);
    let draft_row_elements = rows_for_render
        .into_iter()
        .enumerate()
        .map(|(position, row)| {
            let reorder = rsx! {
                span { class: "player-table-reorder",
                    button {
                        class: "player-table-reorder-button",
                        r#type: "button",
                        disabled: position == 0,
                        title: "Move up (plays earlier)",
                        onclick: move |_| {
                            let (new_position, new_seats) = swap_draft_rows(
                                include_creator(),
                                creator_position(),
                                &additional_seats(),
                                position,
                                position.saturating_sub(1),
                            );
                            creator_position.set(new_position);
                            additional_seats.set(new_seats);
                        },
                        "▲"
                    }
                    button {
                        class: "player-table-reorder-button",
                        r#type: "button",
                        disabled: position >= last_row_position,
                        title: "Move down (plays later)",
                        onclick: move |_| {
                            let (new_position, new_seats) = swap_draft_rows(
                                include_creator(),
                                creator_position(),
                                &additional_seats(),
                                position,
                                position + 1,
                            );
                            creator_position.set(new_position);
                            additional_seats.set(new_seats);
                        },
                        "▼"
                    }
                }
            };
            match row {
                DraftRow::You => rsx! {
                    tr { key: "you",
                        td { {reorder} "{my_display_name.clone().unwrap_or_default()} (you)" }
                        td { "Human" }
                        td {
                            button {
                                class: "toggle-button toggle-button-muted seat-draft-remove",
                                onclick: move |_| include_creator.set(false),
                                "Remove"
                            }
                        }
                    }
                },
                DraftRow::Seat(index, draft) => rsx! {
                    tr { key: "{index}",
                        td {
                            {reorder}
                            if draft.kind == AdditionalSeatKind::Named {
                                NameAutocompleteInput {
                                    value: draft.name.clone(),
                                    on_change: move |value| {
                                        additional_seats.with_mut(|seats| {
                                            if let Some(seat) = seats.get_mut(index) {
                                                seat.name = value;
                                            }
                                        });
                                    },
                                    server_url: server_url.clone(),
                                    token: token.clone(),
                                    placeholder: "Display name to invite".to_string(),
                                }
                            } else if draft.kind == AdditionalSeatKind::Email {
                                input {
                                    class: "seat-draft-name-input",
                                    r#type: "email",
                                    placeholder: "Email to invite",
                                    value: "{draft.name}",
                                    oninput: move |event| {
                                        additional_seats.with_mut(|seats| {
                                            if let Some(seat) = seats.get_mut(index) {
                                                seat.name = event.value();
                                            }
                                        });
                                    },
                                }
                            } else {
                                "{seat_draft_label(draft.kind)}"
                            }
                        }
                        td { "{seat_draft_kind_label(draft.kind)}" }
                        td {
                            button {
                                class: "toggle-button toggle-button-muted seat-draft-remove",
                                onclick: move |_| {
                                    additional_seats.with_mut(|seats| {
                                        if index < seats.len() {
                                            seats.remove(index);
                                        }
                                    });
                                },
                                "Remove"
                            }
                        }
                    }
                },
            }
        });

    let can_submit_builder = additional_seats().iter().all(|seat| {
        !matches!(
            seat.kind,
            AdditionalSeatKind::Named | AdditionalSeatKind::Email
        ) || !seat.name.trim().is_empty()
    }) && (include_creator() || !additional_seats().is_empty());

    // A draft with only engine seats (plus optionally you) has nobody left
    // to hear from — creating it lands straight on "Ready to start" with
    // no invitation in flight, so the button says so instead of "Invite".
    let needs_invitations = additional_seats().iter().any(|seat| {
        matches!(
            seat.kind,
            AdditionalSeatKind::Named | AdditionalSeatKind::Open | AdditionalSeatKind::Email
        )
    });
    let submit_label = if needs_invitations { "Invite" } else { "Start" };

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
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| start_draft(NewGameKind::VsFriend, include_creator, creator_position, additional_seats, time_limit_hours, drafting, variant),
                        "Play Friend"
                    }
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| start_draft(NewGameKind::VsHuman, include_creator, creator_position, additional_seats, time_limit_hours, drafting, variant),
                        "Play Stranger"
                    }
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| start_draft(NewGameKind::VsEngine, include_creator, creator_position, additional_seats, time_limit_hours, drafting, variant),
                        "Play Greedy Bot"
                    }
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| start_draft(NewGameKind::EngineVsEngine, include_creator, creator_position, additional_seats, time_limit_hours, drafting, variant),
                        "Bot Showdown!"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        disabled: is_loading,
                        onclick: move |_| start_draft(NewGameKind::VsHuman, include_creator, creator_position, additional_seats, time_limit_hours, drafting, variant),
                        "New Custom Game..."
                    }
                }

                if drafting() {
                    div { class: "games-panel-detail game-builder",
                        div { class: "game-row-top",
                            span { class: "game-status-badge game-status-new", "New" }
                        }
                        table { class: "player-table player-table-draft",
                            thead {
                                tr {
                                    th { "Player" }
                                    th { "Kind" }
                                    th {}
                                }
                            }
                            tbody { {draft_row_elements} }
                        }
                        if !include_creator() {
                            button {
                                class: "toggle-button toggle-button-muted",
                                onclick: move |_| include_creator.set(true),
                                "+ Add yourself as a player"
                            }
                        }
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
                                    additional_seats.with_mut(|seats| seats.push(AdditionalSeatDraft { kind: AdditionalSeatKind::Email, name: String::new() }));
                                },
                                "+ Invite by email"
                            }
                            button {
                                class: "toggle-button toggle-button-muted",
                                onclick: move |_| {
                                    additional_seats.with_mut(|seats| seats.push(AdditionalSeatDraft { kind: AdditionalSeatKind::Engine, name: String::new() }));
                                },
                                "+ Bot"
                            }
                        }
                        div { class: "game-builder-variant",
                            label { "Edition: " }
                            select {
                                value: "{variant()}",
                                onchange: move |event| variant.set(event.value()),
                                for name in rules_shared::VariantRules::EDITION_NAMES {
                                    option { value: "{name}", "{edition_label(name)}" }
                                }
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
                                    if let Ok(hours) = event.value().parse::<u32>()
                                        && hours > 0 {
                                            time_limit_hours.set(hours);
                                        }
                                },
                            }
                        }
                        div { class: "game-builder-add-row",
                            button {
                                class: "toggle-button",
                                disabled: is_loading || !can_submit_builder,
                                onclick: move |_| {
                                    let seats = build_seats(include_creator(), creator_position(), &additional_seats(), my_display_name.as_deref());
                                    on_custom_new_game.call(CustomGameSubmission {
                                        seats,
                                        move_time_limit_seconds: Some(time_limit_hours() as u64 * 3600),
                                        variant: Some(variant()),
                                        start_immediately: !needs_invitations,
                                    });
                                    drafting.set(false);
                                },
                                "{submit_label}"
                            }
                            button {
                                class: "toggle-button toggle-button-muted",
                                onclick: move |_| drafting.set(false),
                                "Cancel"
                            }
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

#[allow(clippy::too_many_arguments)]
fn game_row(
    summary: &GameSummaryDto,
    selected_id: Option<&str>,
    show_invite_actions: bool,
    has_unread_chat: bool,
    current_game: Option<&GameStateDto>,
    viewer_player_id: Option<&str>,
    can_start: bool,
    is_loading: bool,
    server_url: &str,
    token: Option<&str>,
    mut drafting: Signal<bool>,
    mut chat_draft: Signal<String>,
    confirm_seat_action: Signal<Option<SeatConfirmAction>>,
    add_seat_kind: Signal<AdditionalSeatKind>,
    add_seat_name: Signal<String>,
    on_select: EventHandler<String>,
    on_start: EventHandler<()>,
    on_send_chat: EventHandler<String>,
    on_accept_invitation: EventHandler<String>,
    on_reject_invitation: EventHandler<String>,
    on_remove_game: EventHandler<String>,
    on_reorder_seats: EventHandler<(u8, u8)>,
    on_send_invitation: EventHandler<u8>,
    on_remove_seat: EventHandler<u8>,
    on_withdraw_seat: EventHandler<u8>,
    on_force_resign: EventHandler<u8>,
    on_add_seat: EventHandler<AddSeatSubmission>,
) -> Element {
    let is_selected = selected_id == Some(summary.id.as_str());
    let row_class = if is_selected {
        "game-row game-row-active"
    } else {
        "game-row"
    };
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
    let relative_time = format_relative_time(summary.last_activity_at);
    let select_id = summary.id.clone();
    let invitation_id = summary.invitation_id.clone();

    // Prefer the fully-loaded game (has scores + move history + chat) when
    // it matches this row; fall back to the summary's participants (scores,
    // no history, no chat — the summary never carries messages) while the
    // full load is still in flight.
    let (participants, moves, messages, creator_player_id): (
        Vec<ParticipantDto>,
        Vec<MoveRecordDto>,
        Vec<ChatMessageDto>,
        Option<String>,
    ) = match current_game.filter(|g| g.id == summary.id) {
        Some(g) => (
            g.participants.clone(),
            g.moves.clone(),
            g.messages.clone(),
            g.creator_player_id.clone(),
        ),
        None => (summary.participants.clone(), Vec::new(), Vec::new(), None),
    };
    let loaded_matches = current_game.is_some_and(|g| g.id == summary.id);
    // Only known once the full game (not just the list summary) has
    // loaded — every management control below stays hidden until then,
    // same as `can_reorder` already waits on `loaded_matches`.
    let viewer_is_creator =
        viewer_player_id.is_some() && creator_player_id.as_deref() == viewer_player_id;
    // Mirrors the server's `resolve_viewer_access` `Participant` tier
    // exactly — a genuinely claimed seat. A creator watching their own game
    // (e.g. Bot Showdown) or a spectator never gets chat.
    let can_chat = viewer_player_id.is_some_and(|viewer_id| {
        participants
            .iter()
            .any(|participant| participant.player_id.as_deref() == Some(viewer_id))
    });
    // Removing a finished game is available to a seated participant
    // (`can_chat`'s exact condition) *or* an unseated creator watching
    // their own game (e.g. Bot Showdown) — mirrors the server's
    // `remove_for_player`, which checks the caller's seat first and falls
    // back to `creator_player_id` only when they hold no seat at all.
    // `relationship == Creator` is only ever set for that unseated case
    // (see `list_games`), so it never overlaps with `can_chat`.
    let can_remove = can_chat || summary.relationship == GameRelationship::Creator;
    let ready = is_ready_to_start(&participants);
    // Same enable condition as the Start button below — reordering and
    // starting are both creator-only, "this row's full state is loaded,
    // every seat's filled, and we're online" operations.
    let can_reorder = summary.status == GameStatus::Waiting
        && can_start
        && loaded_matches
        && ready
        && viewer_is_creator;
    let badge_class = format!(
        "game-status-badge game-status-{}",
        status_slug(&summary.status, ready)
    );

    rsx! {
        div { key: "{summary.id}", class: "game-row-wrapper",
            button {
                class: "{row_class}",
                onclick: move |_| {
                    drafting.set(false);
                    on_select.call(select_id.clone());
                },
                div { class: "game-row-top",
                    span { class: "{badge_class}", "{status_label(&summary.status, ready)}" }
                    if has_unread_chat {
                        span { class: "game-row-unread-mail", title: "New message", "✉" }
                    }
                    span { class: "game-row-time", "{relative_time}" }
                }
                p { class: "game-row-participants", "{participants_label}" }
                p { class: "game-row-variant", "{edition_label(&summary.variant)}" }
            }
            if is_selected {
                div { class: "games-panel-detail",
                    {player_table(
                        &participants,
                        &moves,
                        viewer_player_id,
                        creator_player_id.as_deref(),
                        &summary.variant,
                        summary.status,
                        can_reorder,
                        viewer_is_creator,
                        confirm_seat_action,
                        on_reorder_seats,
                        on_send_invitation,
                        on_remove_seat,
                        on_withdraw_seat,
                        on_force_resign,
                    )}
                    if summary.status == GameStatus::Waiting && loaded_matches && viewer_is_creator {
                        {add_seat_row(
                            summary.id.clone(),
                            server_url.to_string(),
                            token.map(str::to_string),
                            add_seat_kind,
                            add_seat_name,
                            on_add_seat,
                        )}
                    }
                    if summary.status == GameStatus::Waiting {
                        div { class: "game-builder-add-row",
                            button {
                                class: "toggle-button",
                                disabled: is_loading || !(can_start && loaded_matches && ready && viewer_is_creator),
                                onclick: move |_| on_start.call(()),
                                "Start"
                            }
                            if !ready {
                                span { class: "games-panel-hint", "Waiting for every seat to be filled" }
                            } else if !viewer_is_creator {
                                span { class: "games-panel-hint", "Only the creator can start" }
                            }
                        }
                    }
                    if summary.status == GameStatus::Finished && can_remove {
                        div { class: "game-builder-add-row",
                            button {
                                class: "toggle-button toggle-button-muted",
                                disabled: is_loading,
                                onclick: {
                                    let remove_id = summary.id.clone();
                                    move |_| on_remove_game.call(remove_id.clone())
                                },
                                "Remove"
                            }
                        }
                    }
                    if can_chat {
                        div { class: "chat-panel",
                            div { class: "chat-messages",
                                if messages.is_empty() {
                                    p { class: "chat-empty", "No messages yet" }
                                }
                                for message in messages.iter() {
                                    {
                                        let is_own = viewer_player_id == Some(message.player_id.as_str());
                                        let message_class = if is_own {
                                            "chat-message chat-message-own"
                                        } else {
                                            "chat-message"
                                        };
                                        rsx! {
                                            div { key: "{message.id}", class: "{message_class}",
                                                span { class: "chat-message-sender", "{message.display_name}" }
                                                span { class: "chat-message-body", "{message.body}" }
                                                span { class: "chat-message-time", "{format_relative_time(message.created_at)}" }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "chat-composer",
                                input {
                                    class: "chat-input",
                                    r#type: "text",
                                    placeholder: "Say something...",
                                    value: "{chat_draft}",
                                    oninput: move |event| chat_draft.set(event.value()),
                                    onkeydown: move |event| {
                                        if event.key() == Key::Enter {
                                            event.prevent_default();
                                            let body = chat_draft().trim().to_string();
                                            if !body.is_empty() {
                                                chat_draft.set(String::new());
                                                on_send_chat.call(body);
                                            }
                                        }
                                    },
                                }
                                button {
                                    class: "toggle-button",
                                    disabled: chat_draft().trim().is_empty(),
                                    onclick: move |_| {
                                        let body = chat_draft().trim().to_string();
                                        if !body.is_empty() {
                                            chat_draft.set(String::new());
                                            on_send_chat.call(body);
                                        }
                                    },
                                    "Send"
                                }
                            }
                        }
                    }
                }
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

/// Collins is an English dictionary — linking to it for a German or
/// Spanish word would send the reader to a lookup for a language it
/// doesn't cover. Both English editions' word lists (SOWPODS for
/// official/Wordfeud, ENABLE2K for North American) are genuinely English;
/// everything else isn't.
fn is_english_dictionary(variant: &str) -> bool {
    rules_shared::VariantRules::by_name(variant)
        .is_some_and(|rules| matches!(rules.language.as_str(), "sowpods" | "enable2k"))
}

/// A seat-management action awaiting user confirmation before it's actually
/// dispatched — see `player_table`'s confirmation modal. Only the actions
/// with real consequences (kicking someone already confirmed, giving up a
/// seat, ending a game early) go through this; sending a fresh invitation
/// or removing a not-yet-accepted seat fire immediately, matching the
/// plan's "nothing's lost" reasoning.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SeatConfirmAction {
    RemoveClaimedSeat(u8),
    Withdraw(u8),
    ForceResign(u8),
}

fn seat_invitation_status_label(status: SeatInvitationStatus) -> &'static str {
    match status {
        SeatInvitationStatus::NotSent => "Not sent yet",
        SeatInvitationStatus::Pending => "Invited — awaiting response",
        SeatInvitationStatus::Rejected => "Declined",
    }
}

#[allow(clippy::too_many_arguments)]
fn player_table(
    participants: &[ParticipantDto],
    moves: &[MoveRecordDto],
    viewer_player_id: Option<&str>,
    creator_player_id: Option<&str>,
    variant: &str,
    game_status: GameStatus,
    // Turn order is just seat position (see `GameSession::swap_seats`), so
    // reordering only makes sense — and is only offered — before the game
    // starts, once every seat is filled (matching the server's own gate).
    can_reorder: bool,
    // The single gate on every management control below (send/resend an
    // invitation, add/remove a seat, force-resign) — the creator is this
    // game's manager, nobody else. Doesn't affect what a seated participant
    // can already do on their own behalf (chat, play, withdraw their own
    // claim), none of which ever needed creator permission.
    viewer_is_creator: bool,
    // Created once, unconditionally, in `GamesPanel` — `player_table` is
    // itself only ever called conditionally (`if is_selected`), so it can't
    // safely call `use_signal` itself without risking hook-order drift
    // across renders; taking the signal as a plain parameter sidesteps
    // that entirely, same reasoning as `drafting`/`chat_draft` above.
    mut confirm_action: Signal<Option<SeatConfirmAction>>,
    on_reorder_seats: EventHandler<(u8, u8)>,
    on_send_invitation: EventHandler<u8>,
    on_remove_seat: EventHandler<u8>,
    on_withdraw_seat: EventHandler<u8>,
    on_force_resign: EventHandler<u8>,
) -> Element {
    let link_words = is_english_dictionary(variant);
    let last_seat = participants.len().saturating_sub(1) as u8;
    let show_manage_column =
        game_status == GameStatus::Waiting || game_status == GameStatus::Active;

    let rows = participants.iter().map(|participant| {
        let is_you = viewer_player_id.is_some() && participant.player_id.as_deref() == viewer_player_id;
        let is_creators_seat = creator_player_id.is_some() && participant.player_id.as_deref() == creator_player_id;
        // A resigned/force-resigned/timed-out seat stays in the roster
        // (never removed mid-game) but is done playing — greyed out
        // rather than dropped, so the table still shows who was in the
        // game. Which of the three it was is already visible in that
        // seat's own "Last move" cell.
        let row_class = match (is_you, participant.resigned) {
            (true, true) => "player-table-you player-table-exited",
            (true, false) => "player-table-you",
            (false, true) => "player-table-exited",
            (false, false) => "",
        };
        let cell = last_move_cell(moves, participant.seat_number);
        let seat_number = participant.seat_number;
        let is_claimed = participant.player_id.is_some();

        let manage_cell = if game_status == GameStatus::Waiting {
            let status_label = participant.invitation_status.map(seat_invitation_status_label);
            rsx! {
                if let Some(label) = status_label {
                    span { class: "player-table-invitation-status", "{label}" }
                }
                if viewer_is_creator && !is_creators_seat {
                    if matches!(participant.invitation_status, Some(SeatInvitationStatus::NotSent) | Some(SeatInvitationStatus::Rejected)) {
                        button {
                            class: "toggle-button toggle-button-muted",
                            onclick: move |_| on_send_invitation.call(seat_number),
                            "Send"
                        }
                    }
                    button {
                        class: "toggle-button toggle-button-muted seat-draft-remove",
                        onclick: move |_| {
                            if is_claimed {
                                confirm_action.set(Some(SeatConfirmAction::RemoveClaimedSeat(seat_number)));
                            } else {
                                on_remove_seat.call(seat_number);
                            }
                        },
                        "Remove"
                    }
                }
                if is_you && !is_creators_seat {
                    button {
                        class: "toggle-button toggle-button-muted",
                        onclick: move |_| confirm_action.set(Some(SeatConfirmAction::Withdraw(seat_number))),
                        "Withdraw"
                    }
                }
            }
        } else if game_status == GameStatus::Active {
            // A multi-player game keeps going once at least 2 seats are
            // still active (see `GameSession::handle_seat_exit`), so an
            // already-resigned/force-resigned/timed-out seat can be sat
            // right next to still-playing ones here — must check
            // `participant.resigned` explicitly, not just `game_status`.
            rsx! {
                if viewer_is_creator && !is_creators_seat && !participant.resigned {
                    button {
                        class: "toggle-button toggle-button-muted",
                        onclick: move |_| confirm_action.set(Some(SeatConfirmAction::ForceResign(seat_number))),
                        "Force-resign"
                    }
                }
            }
        } else {
            rsx! {}
        };

        rsx! {
            tr { key: "{participant.seat_number}", class: "{row_class}",
                td {
                    if can_reorder {
                        span { class: "player-table-reorder",
                            button {
                                class: "player-table-reorder-button",
                                r#type: "button",
                                disabled: seat_number == 0,
                                title: "Move up (plays earlier)",
                                onclick: move |_| on_reorder_seats.call((seat_number, seat_number.saturating_sub(1))),
                                "▲"
                            }
                            button {
                                class: "player-table-reorder-button",
                                r#type: "button",
                                disabled: seat_number >= last_seat,
                                title: "Move down (plays later)",
                                onclick: move |_| on_reorder_seats.call((seat_number, seat_number + 1)),
                                "▼"
                            }
                        }
                    }
                    "{participant.display_name}"
                    if let Some(rating) = participant.current_rating {
                        span { class: "player-table-rating", " ({rating:.0})" }
                    }
                    if is_you {
                        span { class: "player-table-you-tag", " (you)" }
                    }
                }
                td { "{seat_kind_label(&participant.kind)}" }
                td { class: "player-table-score", "{participant.score}" }
                td { {render_last_move(&cell, link_words)} }
                if show_manage_column {
                    td { class: "player-table-manage", {manage_cell} }
                }
            }
        }
    });

    let confirm_copy = confirm_action().map(|action| match action {
        SeatConfirmAction::RemoveClaimedSeat(seat) => (
            "Remove this player?",
            "They've already claimed this seat — removing it takes them out of the game entirely.".to_string(),
            seat,
        ),
        SeatConfirmAction::Withdraw(seat) => (
            "Withdraw from this seat?",
            "You'll give up your claim — the creator can invite someone else, or you back in, afterward.".to_string(),
            seat,
        ),
        SeatConfirmAction::ForceResign(seat) => (
            "End the game and resign this player?",
            "This finishes the game immediately in favor of whoever's left — there's no undoing it.".to_string(),
            seat,
        ),
    });

    rsx! {
        table { class: "player-table",
            thead {
                tr {
                    th { "Player" }
                    th { "Kind" }
                    th { "Score" }
                    th { "Last move" }
                    if show_manage_column {
                        th { class: "player-table-manage" }
                    }
                }
            }
            tbody { {rows} }
        }
        if let Some((title, copy, seat)) = confirm_copy {
            div { class: "modal-backdrop",
                div { class: "modal-card",
                    h2 { class: "modal-title", "{title}" }
                    p { class: "modal-copy", "{copy}" }
                    div { class: "modal-actions",
                        button {
                            class: "toggle-button toggle-button-muted",
                            onclick: move |_| confirm_action.set(None),
                            "Cancel"
                        }
                        button {
                            class: "toggle-button",
                            onclick: move |_| {
                                let action = confirm_action();
                                confirm_action.set(None);
                                match action {
                                    Some(SeatConfirmAction::RemoveClaimedSeat(_)) => on_remove_seat.call(seat),
                                    Some(SeatConfirmAction::Withdraw(_)) => on_withdraw_seat.call(seat),
                                    Some(SeatConfirmAction::ForceResign(_)) => on_force_resign.call(seat),
                                    None => {}
                                }
                            },
                            "Confirm"
                        }
                    }
                }
            }
        }
    }
}

/// Lets the creator add a new seat to an already-created `Waiting` game —
/// the post-creation counterpart to the pre-creation draft builder's own
/// "+ Invite by name"/"+ Open seat"/"+ Bot" row, reusing the same
/// `AdditionalSeatKind` vocabulary for a consistent picker. Deliberately
/// doesn't send an invitation itself (see `AddSeatSubmission`'s doc
/// comment) — sending is a separate `player_table` "Send" button, once the
/// new seat shows up there as `NotSent`.
#[allow(clippy::too_many_arguments)]
fn add_seat_row(
    game_id: String,
    server_url: String,
    token: Option<String>,
    // Owned by `GamesPanel`, same reasoning as `player_table`'s
    // `confirm_action` — this is called conditionally, so it can't safely
    // create its own signals.
    mut kind: Signal<AdditionalSeatKind>,
    mut name: Signal<String>,
    on_add_seat: EventHandler<AddSeatSubmission>,
) -> Element {
    let can_submit = !matches!(
        kind(),
        AdditionalSeatKind::Named | AdditionalSeatKind::Email
    ) || !name().trim().is_empty();
    rsx! {
        div { class: "game-builder-add-row",
            select {
                value: match kind() {
                    AdditionalSeatKind::Named => "named",
                    AdditionalSeatKind::Open => "open",
                    AdditionalSeatKind::Email => "email",
                    AdditionalSeatKind::Engine => "engine",
                },
                onchange: move |event| {
                    kind.set(match event.value().as_str() {
                        "open" => AdditionalSeatKind::Open,
                        "email" => AdditionalSeatKind::Email,
                        "engine" => AdditionalSeatKind::Engine,
                        _ => AdditionalSeatKind::Named,
                    });
                },
                option { value: "named", "Invite by name" }
                option { value: "open", "Open seat (any player may claim)" }
                option { value: "email", "Invite by email" }
                option { value: "engine", "Bot (Greedy)" }
            }
            if kind() == AdditionalSeatKind::Named {
                NameAutocompleteInput {
                    value: name(),
                    on_change: move |value| name.set(value),
                    server_url: server_url.clone(),
                    token: token.clone(),
                    placeholder: "Display name to invite".to_string(),
                }
            } else if kind() == AdditionalSeatKind::Email {
                input {
                    class: "seat-draft-name-input",
                    r#type: "email",
                    placeholder: "Email to invite",
                    value: "{name}",
                    oninput: move |event| name.set(event.value()),
                }
            }
            button {
                class: "toggle-button toggle-button-muted",
                disabled: !can_submit,
                onclick: move |_| {
                    let (seat_kind, display_name, engine_id, claim) = match kind() {
                        AdditionalSeatKind::Named => (
                            SeatKind::Human,
                            name().trim().to_string(),
                            None,
                            Some(SeatClaim::Named { display_name: name().trim().to_string() }),
                        ),
                        AdditionalSeatKind::Open => (
                            SeatKind::Human,
                            "Open seat".to_string(),
                            None,
                            Some(SeatClaim::Open),
                        ),
                        AdditionalSeatKind::Email => (
                            SeatKind::Human,
                            name().trim().to_string(),
                            None,
                            Some(SeatClaim::Email { email: name().trim().to_string() }),
                        ),
                        AdditionalSeatKind::Engine => (
                            SeatKind::Engine,
                            "Bot".to_string(),
                            Some(DEFAULT_ENGINE_ID.to_string()),
                            None,
                        ),
                    };
                    on_add_seat.call(AddSeatSubmission {
                        game_id: game_id.clone(),
                        kind: seat_kind,
                        display_name,
                        engine_id,
                        claim,
                    });
                    name.set(String::new());
                },
                "+ Add seat"
            }
        }
    }
}

/// A "Named" seat's display-name input, with a live filtered dropdown of
/// matching registered players (`GET /players/search`) — verifying the
/// name actually exists is otherwise only enforced late, when the
/// invitation is actually sent (a 404 from `invite_player_to_game`/
/// `create_game`), which is a worse time to discover a typo than while
/// still typing. A genuine `#[component]`, not a plain fn like
/// `add_seat_row`/`player_table` — it owns real per-keystroke state
/// (`suggestions`) that must survive across renders, which only a properly
/// mounted/unmounted Dioxus component can safely do when it's rendered
/// conditionally (see those two functions' own doc comments on why a bare
/// fn calling `use_signal` can't).
/// Controlled-input style (`value` + `on_change`), not a raw `Signal<String>`
/// — the original pre-creation draft builder keeps its Named seat's name
/// inside a `Vec<AdditionalSeatDraft>` element, which a plain `Signal`
/// can't point into directly, so this needs to work through a callback
/// either way (a signal-backed caller just does `move |v| signal.set(v)`).
#[component]
fn NameAutocompleteInput(
    value: String,
    on_change: EventHandler<String>,
    server_url: String,
    token: Option<String>,
    placeholder: String,
) -> Element {
    let mut suggestions = use_signal(Vec::<String>::new);
    let mut searched_and_empty = use_signal(|| false);

    let trigger_search = move |query: String| {
        let server_url = server_url.clone();
        let token = token.clone();
        spawn(async move {
            let trimmed = query.trim();
            if trimmed.len() < 2 {
                suggestions.set(Vec::new());
                searched_and_empty.set(false);
                return;
            }
            if let Ok(names) =
                crate::app::search_players(&server_url, trimmed, token.as_deref()).await
            {
                searched_and_empty.set(names.is_empty());
                suggestions.set(names);
            }
        });
    };

    rsx! {
        div { class: "name-autocomplete",
            input {
                class: "seat-draft-name-input",
                placeholder: "{placeholder}",
                value: "{value}",
                oninput: move |event| {
                    let query = event.value();
                    on_change.call(query.clone());
                    trigger_search(query);
                },
            }
            if !suggestions().is_empty() {
                div { class: "name-autocomplete-dropdown",
                    for suggestion in suggestions() {
                        button {
                            class: "name-autocomplete-item",
                            r#type: "button",
                            // `onmousedown` (not `onclick`) so the
                            // selection registers before the input's own
                            // `onblur` would otherwise hide this dropdown
                            // first — there is no `onblur` here today, but
                            // this is the standard-enough pattern that
                            // adding one later won't silently break
                            // picking a suggestion.
                            onmousedown: move |event| {
                                event.prevent_default();
                                on_change.call(suggestion.clone());
                                suggestions.set(Vec::new());
                            },
                            "{suggestion}"
                        }
                    }
                }
            } else if searched_and_empty() {
                span { class: "name-autocomplete-empty", "No matching player" }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum LastMoveCell {
    None,
    Note(String),
    Word { word: String, score_delta: i32 },
}

fn last_move_cell(moves: &[MoveRecordDto], seat_number: u8) -> LastMoveCell {
    match moves
        .iter()
        .rev()
        .find(|record| record.seat_number == seat_number)
    {
        None => LastMoveCell::None,
        Some(record) if record.move_type == "place" => LastMoveCell::Word {
            word: record.main_word.clone().unwrap_or_default(),
            score_delta: record.score_delta,
        },
        Some(record) => LastMoveCell::Note(action_note(record)),
    }
}

fn render_last_move(cell: &LastMoveCell, link_words: bool) -> Element {
    match cell {
        LastMoveCell::None => rsx! {
            span { class: "player-table-last-move-empty", "—" }
        },
        LastMoveCell::Note(note) => rsx! {
            span { class: "player-table-last-move-note", "{note}" }
        },
        LastMoveCell::Word { word, score_delta } => {
            let delta = *score_delta;
            if link_words {
                let url = format!(
                    "https://www.collinsdictionary.com/dictionary/english/{}",
                    word.to_lowercase()
                );
                rsx! {
                    a {
                        class: "player-table-last-move-word",
                        href: "{url}",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "{word}"
                    }
                    span { class: "player-table-last-move-score", " +{delta}" }
                }
            } else {
                rsx! {
                    span { class: "player-table-last-move-word", "{word}" }
                    span { class: "player-table-last-move-score", " +{delta}" }
                }
            }
        }
    }
}

/// Pass/exchange/resign rows have no word or score, so this builds the
/// short note shown in that slot instead. Exchange's tile count is parsed
/// out of the existing `description` text (e.g. "Alice exchanged 3 tiles")
/// rather than adding a new field, since the server already formats it.
fn action_note(record: &MoveRecordDto) -> String {
    match record.move_type.as_str() {
        "pass" => "passed".to_string(),
        "resign" => "resigned".to_string(),
        "force_resign" => "resigned (forced)".to_string(),
        "timeout" => "retired (exceeded time limit)".to_string(),
        "exchange" => {
            let count = record
                .description
                .split_whitespace()
                .find_map(|token| token.parse::<u32>().ok())
                .unwrap_or(0);
            format!(
                "exchanged {count} letter{}",
                if count == 1 { "" } else { "s" }
            )
        }
        other => other.to_string(),
    }
}

fn seat_kind_label(kind: &SeatKind) -> &'static str {
    match kind {
        SeatKind::Human => "Human",
        SeatKind::Engine => "Bot",
    }
}

fn seat_draft_label(kind: AdditionalSeatKind) -> &'static str {
    match kind {
        AdditionalSeatKind::Named => "Invite by name",
        AdditionalSeatKind::Open => "Open seat (any player may claim)",
        AdditionalSeatKind::Email => "Invite by email",
        AdditionalSeatKind::Engine => "Bot (Greedy)",
    }
}

fn seat_draft_kind_label(kind: AdditionalSeatKind) -> &'static str {
    match kind {
        AdditionalSeatKind::Engine => "Bot",
        AdditionalSeatKind::Named | AdditionalSeatKind::Open | AdditionalSeatKind::Email => "Human",
    }
}

/// Whether every human seat has a real occupant — mirrors the server's own
/// `start_game` check exactly (see `crates/server-game/src/app.rs`), so
/// "ready" here means "the Start request would actually succeed", not just
/// a cosmetic guess. A `Waiting` game only changes to this once every
/// invitation has been responded to (accepted or the seat otherwise
/// filled) — an unclaimed `Open` or unaccepted `Named` seat keeps it at
/// plain "Waiting".
fn is_ready_to_start(participants: &[ParticipantDto]) -> bool {
    participants
        .iter()
        .all(|p| p.kind == SeatKind::Engine || p.player_id.is_some())
}

/// True if this game has a chat message this device hasn't marked as seen
/// yet (see `crate::local_storage::StoredChatWatermarks`). A game with no
/// messages at all, or whose latest message matches our stored watermark,
/// is never unread.
fn has_unread_chat(summary: &GameSummaryDto, chat_watermarks: &HashMap<String, i64>) -> bool {
    summary
        .last_message_at
        .as_ref()
        .is_some_and(|latest| chat_watermarks.get(&summary.id) != Some(latest))
}

fn status_label(status: &GameStatus, ready_to_start: bool) -> &'static str {
    match status {
        GameStatus::Waiting if ready_to_start => "Ready to start",
        GameStatus::Waiting => "Waiting",
        GameStatus::Active => "Playing",
        GameStatus::Finished => "Finished",
    }
}

fn status_slug(status: &GameStatus, ready_to_start: bool) -> &'static str {
    match status {
        GameStatus::Waiting if ready_to_start => "ready",
        GameStatus::Waiting => "waiting",
        GameStatus::Active => "active",
        GameStatus::Finished => "finished",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vs_engine_seats_are_one_human_one_engine() {
        let (include_creator, additional) = preset_draft(NewGameKind::VsEngine);
        let seats = build_seats(include_creator, 0, &additional, Some("Alice"));
        assert_eq!(seats.len(), 2);
        assert_eq!(seats[0].kind, SeatKind::Human);
        assert_eq!(seats[0].display_name, "Alice");
        assert_eq!(seats[1].kind, SeatKind::Engine);
        assert!(seats[1].engine_id.is_some());
    }

    #[test]
    fn vs_friend_seat_is_a_named_invitation_waiting_for_a_name() {
        let (include_creator, additional) = preset_draft(NewGameKind::VsFriend);
        assert!(include_creator);
        assert_eq!(additional.len(), 1);
        assert_eq!(additional[0].kind, AdditionalSeatKind::Named);
        assert!(additional[0].name.is_empty());
    }

    #[test]
    fn vs_human_seats_are_both_human() {
        let (include_creator, additional) = preset_draft(NewGameKind::VsHuman);
        let seats = build_seats(include_creator, 0, &additional, Some("Alice"));
        assert_eq!(seats.len(), 2);
        assert!(seats.iter().all(|seat| seat.kind == SeatKind::Human));
        assert_eq!(seats[0].display_name, "Alice");
    }

    #[test]
    fn engine_vs_engine_has_no_creator_seat_and_ignores_display_name() {
        let (include_creator, additional) = preset_draft(NewGameKind::EngineVsEngine);
        assert!(!include_creator);
        let seats = build_seats(include_creator, 0, &additional, Some("Alice"));
        assert_eq!(seats.len(), 2);
        assert!(seats.iter().all(|seat| seat.kind == SeatKind::Engine));
        assert!(seats.iter().all(|seat| seat.engine_id.is_some()));
        assert!(seats.iter().all(|seat| seat.display_name != "Alice"));
    }

    #[test]
    fn anonymous_creator_gets_a_generic_name_instead_of_a_hardcoded_one() {
        let (include_creator, additional) = preset_draft(NewGameKind::VsEngine);
        let seats = build_seats(include_creator, 0, &additional, None);
        assert_ne!(seats[0].display_name, "Alice");
        assert!(!seats[0].display_name.is_empty());
    }

    #[test]
    fn removing_the_creator_row_seats_only_the_others() {
        let seats = build_seats(
            false,
            0,
            &[AdditionalSeatDraft {
                kind: AdditionalSeatKind::Engine,
                name: String::new(),
            }],
            Some("Alice"),
        );
        assert_eq!(seats.len(), 1);
        assert_eq!(seats[0].kind, SeatKind::Engine);
    }

    fn engine_seat() -> AdditionalSeatDraft {
        AdditionalSeatDraft {
            kind: AdditionalSeatKind::Engine,
            name: String::new(),
        }
    }

    #[test]
    fn draft_rows_puts_you_at_position_zero_by_default() {
        let rows = draft_rows(true, 0, &[engine_seat()]);
        assert_eq!(rows, vec![DraftRow::You, DraftRow::Seat(0, engine_seat())]);
    }

    #[test]
    fn draft_rows_can_place_you_after_other_seats() {
        let rows = draft_rows(true, 1, &[engine_seat()]);
        assert_eq!(rows, vec![DraftRow::Seat(0, engine_seat()), DraftRow::You]);
    }

    #[test]
    fn draft_rows_omits_you_when_not_included() {
        let rows = draft_rows(false, 0, &[engine_seat(), engine_seat()]);
        assert_eq!(
            rows,
            vec![
                DraftRow::Seat(0, engine_seat()),
                DraftRow::Seat(1, engine_seat())
            ]
        );
    }

    #[test]
    fn swap_draft_rows_moves_you_down_past_a_seat() {
        // Bot-Showdown-style "vs Engine": you're first, one engine second —
        // this is exactly the scenario that motivated adding reordering to
        // the draft itself, since a one-click preset never passes through a
        // `Waiting` screen where the post-creation reorder endpoint would
        // otherwise be reachable.
        let (new_position, new_seats) = swap_draft_rows(true, 0, &[engine_seat()], 0, 1);
        assert_eq!(new_position, 1);
        assert_eq!(new_seats, vec![engine_seat()]);
        // The engine seat itself is unchanged, just now first.
        assert_eq!(
            draft_rows(true, new_position, &new_seats),
            vec![DraftRow::Seat(0, engine_seat()), DraftRow::You]
        );
    }

    #[test]
    fn swap_draft_rows_reorders_two_additional_seats_leaving_you_in_place() {
        let named = AdditionalSeatDraft {
            kind: AdditionalSeatKind::Named,
            name: "Bob".to_string(),
        };
        // you, engine, named -- swap the two additional seats (positions 1, 2).
        let (new_position, new_seats) =
            swap_draft_rows(true, 0, &[engine_seat(), named.clone()], 1, 2);
        assert_eq!(
            new_position, 0,
            "you weren't involved in the swap, so your position is unchanged"
        );
        assert_eq!(new_seats, vec![named, engine_seat()]);
    }

    #[test]
    fn swap_draft_rows_out_of_range_is_a_no_op() {
        let (new_position, new_seats) = swap_draft_rows(true, 0, &[engine_seat()], 0, 5);
        assert_eq!(new_position, 0);
        assert_eq!(new_seats, vec![engine_seat()]);
    }

    #[test]
    fn build_seats_honors_a_non_zero_creator_position() {
        let seats = build_seats(true, 1, &[engine_seat()], Some("Alice"));
        assert_eq!(seats.len(), 2);
        assert_eq!(seats[0].kind, SeatKind::Engine);
        assert_eq!(seats[1].kind, SeatKind::Human);
        assert_eq!(seats[1].display_name, "Alice");
    }

    #[test]
    fn last_move_cell_picks_the_most_recent_record_for_that_seat() {
        let moves = vec![
            MoveRecordDto {
                move_number: 1,
                seat_number: 0,
                move_type: "place".to_string(),
                main_word: Some("CAT".to_string()),
                score_delta: 10,
                positions: Vec::new(),
                description: String::new(),
            },
            MoveRecordDto {
                move_number: 2,
                seat_number: 1,
                move_type: "pass".to_string(),
                main_word: None,
                score_delta: 0,
                positions: Vec::new(),
                description: String::new(),
            },
            MoveRecordDto {
                move_number: 3,
                seat_number: 0,
                move_type: "place".to_string(),
                main_word: Some("DOG".to_string()),
                score_delta: 8,
                positions: Vec::new(),
                description: String::new(),
            },
        ];
        match last_move_cell(&moves, 0) {
            LastMoveCell::Word { word, score_delta } => {
                assert_eq!(word, "DOG");
                assert_eq!(score_delta, 8);
            }
            other => panic!("expected a word cell, got {other:?}"),
        }
    }

    #[test]
    fn last_move_cell_is_none_when_seat_has_no_moves() {
        assert_eq!(last_move_cell(&[], 0), LastMoveCell::None);
    }

    #[test]
    fn only_english_editions_link_to_an_english_dictionary() {
        assert!(is_english_dictionary("official"));
        assert!(is_english_dictionary("wordfeud"));
        assert!(is_english_dictionary("north_american"));
        assert!(!is_english_dictionary("german"));
        assert!(!is_english_dictionary("spanish"));
        assert!(!is_english_dictionary("not-a-real-edition"));
    }

    fn participant(kind: SeatKind, player_id: Option<&str>) -> ParticipantDto {
        ParticipantDto {
            seat_number: 0,
            kind,
            display_name: "Someone".to_string(),
            player_id: player_id.map(str::to_string),
            engine_id: None,
            score: 0,
            invitation_status: None,
            invited_email: None,
            rating_before: None,
            rating_after: None,
            current_rating: None,
            resigned: false,
        }
    }

    #[test]
    fn ready_to_start_when_every_human_seat_is_claimed() {
        let participants = vec![
            participant(SeatKind::Human, Some("p1")),
            participant(SeatKind::Engine, None),
        ];
        assert!(is_ready_to_start(&participants));
    }

    #[test]
    fn not_ready_while_a_human_seat_is_unclaimed() {
        let participants = vec![
            participant(SeatKind::Human, Some("p1")),
            participant(SeatKind::Human, None),
        ];
        assert!(!is_ready_to_start(&participants));
    }

    #[test]
    fn all_engine_seats_are_always_ready() {
        let participants = vec![
            participant(SeatKind::Engine, None),
            participant(SeatKind::Engine, None),
        ];
        assert!(is_ready_to_start(&participants));
    }

    fn summary_with_last_message_at(last_message_at: Option<i64>) -> GameSummaryDto {
        GameSummaryDto {
            id: "game-1".to_string(),
            status: GameStatus::Active,
            variant: "official".to_string(),
            current_seat: 0,
            participants: vec![],
            last_activity_at: 0,
            move_time_limit_seconds: 0,
            turn_started_at: 0,
            relationship: GameRelationship::Participant,
            invitation_id: None,
            last_message_at,
        }
    }

    #[test]
    fn no_unread_chat_when_the_game_has_never_had_a_message() {
        let summary = summary_with_last_message_at(None);
        assert!(!has_unread_chat(&summary, &HashMap::new()));
    }

    #[test]
    fn unread_chat_when_theres_no_watermark_for_the_game_yet() {
        let summary = summary_with_last_message_at(Some(100));
        assert!(has_unread_chat(&summary, &HashMap::new()));
    }

    #[test]
    fn no_unread_chat_when_the_watermark_matches_the_latest_message() {
        let summary = summary_with_last_message_at(Some(100));
        let mut watermarks = HashMap::new();
        watermarks.insert("game-1".to_string(), 100);
        assert!(!has_unread_chat(&summary, &watermarks));
    }

    #[test]
    fn unread_chat_when_the_watermark_is_stale() {
        let summary = summary_with_last_message_at(Some(200));
        let mut watermarks = HashMap::new();
        watermarks.insert("game-1".to_string(), 100);
        assert!(has_unread_chat(&summary, &watermarks));
    }
}
