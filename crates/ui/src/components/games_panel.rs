use crate::edition_label::edition_label;
use crate::time_format::format_relative_time;
use api::{
    CreateSeatRequest, GameRelationship, GameStateDto, GameStatus, GameSummaryDto, MoveRecordDto,
    ParticipantDto, SeatClaim, SeatKind,
};
use dioxus::prelude::*;

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

/// Seeds the draft signals from a preset and opens the draft view. A plain
/// function (rather than a closure captured by multiple buttons) sidesteps
/// the `FnMut`-borrow ambiguity of sharing one closure across several
/// `onclick` handlers — each button just calls this directly with its own
/// `NewGameKind` and the (`Copy`) signals.
fn start_draft(
    kind: NewGameKind,
    mut include_creator: Signal<bool>,
    mut additional_seats: Signal<Vec<AdditionalSeatDraft>>,
    mut time_limit_hours: Signal<u32>,
    mut drafting: Signal<bool>,
    mut variant: Signal<String>,
) {
    let (creator, seats) = preset_draft(kind);
    include_creator.set(creator);
    additional_seats.set(seats);
    time_limit_hours.set(DEFAULT_TIME_LIMIT_HOURS);
    variant.set("official".to_string());
    drafting.set(true);
}

/// Resolves a draft roster into `CreateSeatRequest`s ready to POST.
fn build_seats(
    include_creator: bool,
    additional: &[AdditionalSeatDraft],
    my_display_name: Option<&str>,
) -> Vec<CreateSeatRequest> {
    let mut seats = Vec::new();
    if include_creator {
        seats.push(CreateSeatRequest {
            kind: SeatKind::Human,
            display_name: my_display_name.unwrap_or("Player 1").to_string(),
            engine_id: None,
            claim: Some(SeatClaim::Creator),
        });
    }
    let engine_count = additional
        .iter()
        .filter(|seat| seat.kind == AdditionalSeatKind::Engine)
        .count();
    let mut engine_index = 0;
    for draft in additional {
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
    }
    seats
}

#[component]
#[allow(clippy::too_many_arguments)]
pub fn GamesPanel(
    summaries: Vec<GameSummaryDto>,
    selected_id: Option<String>,
    current_game: Option<GameStateDto>,
    viewer_player_id: Option<String>,
    is_loading: bool,
    my_display_name: Option<String>,
    can_start: bool,
    on_select: EventHandler<String>,
    on_start: EventHandler<()>,
    on_custom_new_game: EventHandler<CustomGameSubmission>,
    on_accept_invitation: EventHandler<String>,
    on_reject_invitation: EventHandler<String>,
    on_refresh: EventHandler<()>,
) -> Element {
    let mut drafting = use_signal(|| false);
    let mut include_creator = use_signal(|| true);
    let mut additional_seats = use_signal(Vec::<AdditionalSeatDraft>::new);
    let mut time_limit_hours = use_signal(|| DEFAULT_TIME_LIMIT_HOURS);
    let mut variant = use_signal(|| "official".to_string());

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
        move |title: &'static str, rows: Vec<GameSummaryDto>, show_invite_actions: bool| {
            if rows.is_empty() {
                return rsx! {};
            }
            let row_elements = rows.into_iter().map(|summary| {
                game_row(
                    &summary,
                    selected_id.as_deref(),
                    show_invite_actions,
                    current_game.as_ref(),
                    viewer_player_id.as_deref(),
                    can_start,
                    is_loading,
                    drafting,
                    on_select,
                    on_start,
                    on_accept_invitation,
                    on_reject_invitation,
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

    let seat_rows = additional_seats().into_iter().enumerate().map(|(index, draft)| {
        let removable_index = index;
        rsx! {
            tr { key: "{index}",
                td {
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
                                if removable_index < seats.len() {
                                    seats.remove(removable_index);
                                }
                            });
                        },
                        "Remove"
                    }
                }
            }
        }
    });

    let can_submit_builder = additional_seats()
        .iter()
        .all(|seat| seat.kind != AdditionalSeatKind::Named || !seat.name.trim().is_empty())
        && (include_creator() || !additional_seats().is_empty());

    // A draft with only engine seats (plus optionally you) has nobody left
    // to hear from — creating it lands straight on "Ready to start" with
    // no invitation in flight, so the button says so instead of "Invite".
    let needs_invitations = additional_seats()
        .iter()
        .any(|seat| matches!(seat.kind, AdditionalSeatKind::Named | AdditionalSeatKind::Open));
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
                    span { class: "games-panel-new-game-label", "New game:" }
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| start_draft(NewGameKind::VsEngine, include_creator, additional_seats, time_limit_hours, drafting, variant),
                        "vs Engine"
                    }
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| start_draft(NewGameKind::VsHuman, include_creator, additional_seats, time_limit_hours, drafting, variant),
                        "vs Human"
                    }
                    button {
                        class: "toggle-button",
                        disabled: is_loading,
                        onclick: move |_| start_draft(NewGameKind::EngineVsEngine, include_creator, additional_seats, time_limit_hours, drafting, variant),
                        "Engine vs Engine"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        disabled: is_loading,
                        onclick: move |_| start_draft(NewGameKind::VsHuman, include_creator, additional_seats, time_limit_hours, drafting, variant),
                        "Custom game..."
                    }
                }

                if drafting() {
                    div { class: "games-panel-detail game-builder",
                        div { class: "game-row-top",
                            span { class: "game-status-badge game-status-new", "New" }
                        }
                        table { class: "player-table",
                            thead {
                                tr {
                                    th { "Player" }
                                    th { "Kind" }
                                    th {}
                                }
                            }
                            tbody {
                                if include_creator() {
                                    tr { key: "you",
                                        td { "{my_display_name.clone().unwrap_or_default()} (you)" }
                                        td { "Human" }
                                        td {
                                            button {
                                                class: "toggle-button toggle-button-muted seat-draft-remove",
                                                onclick: move |_| include_creator.set(false),
                                                "Remove"
                                            }
                                        }
                                    }
                                }
                                {seat_rows}
                            }
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
                                    additional_seats.with_mut(|seats| seats.push(AdditionalSeatDraft { kind: AdditionalSeatKind::Engine, name: String::new() }));
                                },
                                "+ Engine"
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
                                    if let Ok(hours) = event.value().parse::<u32>() {
                                        if hours > 0 {
                                            time_limit_hours.set(hours);
                                        }
                                    }
                                },
                            }
                        }
                        div { class: "game-builder-add-row",
                            button {
                                class: "toggle-button",
                                disabled: is_loading || !can_submit_builder,
                                onclick: move |_| {
                                    let seats = build_seats(include_creator(), &additional_seats(), my_display_name.as_deref());
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
    current_game: Option<&GameStateDto>,
    viewer_player_id: Option<&str>,
    can_start: bool,
    is_loading: bool,
    mut drafting: Signal<bool>,
    on_select: EventHandler<String>,
    on_start: EventHandler<()>,
    on_accept_invitation: EventHandler<String>,
    on_reject_invitation: EventHandler<String>,
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
    let relative_time = format_relative_time(&summary.last_activity_at);
    let select_id = summary.id.clone();
    let invitation_id = summary.invitation_id.clone();

    // Prefer the fully-loaded game (has scores + move history) when it
    // matches this row; fall back to the summary's participants (scores,
    // no history) while the full load is still in flight.
    let (participants, moves): (Vec<ParticipantDto>, Vec<MoveRecordDto>) =
        match current_game.filter(|g| g.id == summary.id) {
            Some(g) => (g.participants.clone(), g.moves.clone()),
            None => (summary.participants.clone(), Vec::new()),
        };
    let loaded_matches = current_game.is_some_and(|g| g.id == summary.id);
    let ready = is_ready_to_start(&participants);
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
                    span { class: "game-row-time", "{relative_time}" }
                }
                p { class: "game-row-participants", "{participants_label}" }
                p { class: "game-row-variant", "{edition_label(&summary.variant)}" }
            }
            if is_selected {
                div { class: "games-panel-detail",
                    {player_table(&participants, &moves, viewer_player_id, &summary.variant)}
                    if summary.status == GameStatus::Waiting {
                        div { class: "game-builder-add-row",
                            button {
                                class: "toggle-button",
                                disabled: is_loading || !(can_start && loaded_matches && ready),
                                onclick: move |_| on_start.call(()),
                                "Start"
                            }
                            if !ready {
                                span { class: "games-panel-hint", "Waiting for every seat to be filled" }
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

fn player_table(
    participants: &[ParticipantDto],
    moves: &[MoveRecordDto],
    viewer_player_id: Option<&str>,
    variant: &str,
) -> Element {
    let link_words = is_english_dictionary(variant);
    let rows = participants.iter().map(|participant| {
        let is_you = viewer_player_id.is_some() && participant.player_id.as_deref() == viewer_player_id;
        let row_class = if is_you { "player-table-you" } else { "" };
        let cell = last_move_cell(moves, participant.seat_number);
        rsx! {
            tr { key: "{participant.seat_number}", class: "{row_class}",
                td {
                    "{participant.display_name}"
                    if is_you {
                        span { class: "player-table-you-tag", " (you)" }
                    }
                }
                td { "{seat_kind_label(&participant.kind)}" }
                td { class: "player-table-score", "{participant.score}" }
                td { {render_last_move(&cell, link_words)} }
            }
        }
    });
    rsx! {
        table { class: "player-table",
            thead {
                tr {
                    th { "Player" }
                    th { "Kind" }
                    th { "Score" }
                    th { "Last move" }
                }
            }
            tbody { {rows} }
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
    match moves.iter().rev().find(|record| record.seat_number == seat_number) {
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
        "timeout" => "retired (exceeded time limit)".to_string(),
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

fn seat_kind_label(kind: &SeatKind) -> &'static str {
    match kind {
        SeatKind::Human => "Human",
        SeatKind::Engine => "Engine",
    }
}

fn seat_draft_label(kind: AdditionalSeatKind) -> &'static str {
    match kind {
        AdditionalSeatKind::Named => "Invite by name",
        AdditionalSeatKind::Open => "Open seat (any player may claim)",
        AdditionalSeatKind::Engine => "Engine (Greedy)",
    }
}

fn seat_draft_kind_label(kind: AdditionalSeatKind) -> &'static str {
    match kind {
        AdditionalSeatKind::Engine => "Engine",
        AdditionalSeatKind::Named | AdditionalSeatKind::Open => "Human",
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
        let seats = build_seats(include_creator, &additional, Some("Alice"));
        assert_eq!(seats.len(), 2);
        assert_eq!(seats[0].kind, SeatKind::Human);
        assert_eq!(seats[0].display_name, "Alice");
        assert_eq!(seats[1].kind, SeatKind::Engine);
        assert!(seats[1].engine_id.is_some());
    }

    #[test]
    fn vs_human_seats_are_both_human() {
        let (include_creator, additional) = preset_draft(NewGameKind::VsHuman);
        let seats = build_seats(include_creator, &additional, Some("Alice"));
        assert_eq!(seats.len(), 2);
        assert!(seats.iter().all(|seat| seat.kind == SeatKind::Human));
        assert_eq!(seats[0].display_name, "Alice");
    }

    #[test]
    fn engine_vs_engine_has_no_creator_seat_and_ignores_display_name() {
        let (include_creator, additional) = preset_draft(NewGameKind::EngineVsEngine);
        assert!(!include_creator);
        let seats = build_seats(include_creator, &additional, Some("Alice"));
        assert_eq!(seats.len(), 2);
        assert!(seats.iter().all(|seat| seat.kind == SeatKind::Engine));
        assert!(seats.iter().all(|seat| seat.engine_id.is_some()));
        assert!(seats.iter().all(|seat| seat.display_name != "Alice"));
    }

    #[test]
    fn anonymous_creator_gets_a_generic_name_instead_of_a_hardcoded_one() {
        let (include_creator, additional) = preset_draft(NewGameKind::VsEngine);
        let seats = build_seats(include_creator, &additional, None);
        assert_ne!(seats[0].display_name, "Alice");
        assert!(!seats[0].display_name.is_empty());
    }

    #[test]
    fn removing_the_creator_row_seats_only_the_others() {
        let seats = build_seats(
            false,
            &[AdditionalSeatDraft { kind: AdditionalSeatKind::Engine, name: String::new() }],
            Some("Alice"),
        );
        assert_eq!(seats.len(), 1);
        assert_eq!(seats[0].kind, SeatKind::Engine);
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
        let participants = vec![participant(SeatKind::Engine, None), participant(SeatKind::Engine, None)];
        assert!(is_ready_to_start(&participants));
    }
}
