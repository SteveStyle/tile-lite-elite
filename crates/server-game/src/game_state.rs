use std::sync::Arc;
use std::time::Duration;

use api::{
    BoardCellDto, ChatMessageDto, DirectionDto, EngineProfileDto, GameStateDto, GameStatus,
    MoveCandidateDto, MoveRecordDto, ParticipantDto, PositionDto, PremiumDto, RackDto,
    SeatInvitationStatus, SeatKind, TileDto, TilePlacementDto,
};
use engine_core::{EngineAction, EngineMetadata, EngineRequest, GreedyEngine, GameEngine};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rules_shared::{
    Alphabet, BoardCell, BoardState, Direction, FilledCell, GameState, Letter, MoveCandidate, Rack,
    RulesEngine, Tile, TilePlacement, VariantRules, format_move_error,
};
use serde::{Deserialize, Serialize};

use crate::persistence::InvitationRecord;

#[derive(Clone)]
pub struct EngineRegistry {
    pub engines: Vec<Arc<dyn GameEngine>>,
}

impl Default for EngineRegistry {
    fn default() -> Self {
        Self {
            engines: vec![Arc::new(GreedyEngine::new())],
        }
    }
}

impl EngineRegistry {
    pub fn metadata(&self) -> Vec<EngineProfileDto> {
        self.engines
            .iter()
            .map(|engine| engine_profile_from_metadata(engine.metadata()))
            .collect()
    }

    pub fn find(&self, id: &str) -> Option<Arc<dyn GameEngine>> {
        self.engines
            .iter()
            .find(|engine| engine.metadata().id == id)
            .cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveRecord {
    pub move_number: i64,
    pub seat_number: u8,
    pub move_type: String,
    pub main_word: Option<String>,
    pub score_delta: i32,
    pub description: String,
    /// Board squares this move placed a tile on — empty for anything but
    /// `"place"` (pass/exchange/resign/timeout touch no squares). `#[serde(default)]`
    /// so game snapshots persisted before this field existed still deserialize.
    #[serde(default)]
    pub positions: Vec<PositionDto>,
}

/// A single chat message posted to a game. `player_id`/`display_name` are
/// denormalized snapshots at send time (matching `ParticipantState.display_name`'s
/// own precedent of not joining back to `players`), not a live lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessageRecord {
    pub id: String,
    pub player_id: String,
    pub display_name: String,
    pub body: String,
    pub created_at: String,
}

/// Cap on a single chat message's length — plain hygiene, not intended to be
/// user-configurable.
const MAX_CHAT_MESSAGE_LEN: usize = 1000;

/// What a specific caller (identified by `player_id`, or `None` if not
/// logged in) is allowed to see of a game's full state. Distinct from
/// `GameSummaryDto`'s visibility (creator/participant/invited, used by
/// `list_games`), which is unaffected by any of this — a `Waiting` game
/// with an open or named invitation has nothing to redact yet (empty
/// board, no moves, zero scores), so an invitee who hasn't accepted only
/// ever needs the summary, never the full `GameStateDto` this gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerAccess {
    /// Not logged in, or logged in but neither the creator nor a seated
    /// participant of this specific game. Callers must reject outright
    /// (401/403) rather than return a redacted-to-nothing payload.
    Rejected,
    /// Created this game but holds no seat in it (e.g. watching a Bot
    /// Showdown they set up). Sees board/moves/participants, never any
    /// rack, never chat.
    Creator,
    /// Holds a seat. Sees everything the creator sees, plus chat, plus
    /// their own rack (other seats' racks stay redacted).
    Participant { seat_number: u8 },
}

/// The single shared authorization check for a game's full state — used
/// identically by every HTTP handler that returns a `GameStateDto` and by
/// the `game_events` WebSocket, so the rule can't drift between them.
pub fn resolve_viewer_access(session: &GameSession, player_id: Option<&str>) -> ViewerAccess {
    let Some(player_id) = player_id else {
        return ViewerAccess::Rejected;
    };
    // Participant checked first: a creator who also claimed a seat (e.g.
    // the "vs Engine"/"Play Friend" presets) must resolve to the strictly
    // more-permissive `Participant` tier, not `Creator`.
    if let Some(participant) = session
        .participants
        .iter()
        .find(|participant| participant.player_id.as_deref() == Some(player_id))
    {
        return ViewerAccess::Participant {
            seat_number: participant.seat_number,
        };
    }
    if session.creator_player_id.as_deref() == Some(player_id) {
        return ViewerAccess::Creator;
    }
    ViewerAccess::Rejected
}

/// Redacts a fully-built `GameStateDto` down to what `access` is allowed to
/// see. Deliberately a separate step from `to_dto()` itself — `to_dto()` is
/// also used internally for persistence and as the canonical broadcast
/// payload, both of which need full fidelity; redaction only happens at the
/// point a `GameStateDto` actually leaves the server to a specific caller.
/// Callers must have already turned `ViewerAccess::Rejected` into a 401/403
/// before reaching here — this function has no rejected case to handle.
pub fn redact_game_state(mut dto: GameStateDto, access: &ViewerAccess) -> GameStateDto {
    let visible_seat = match access {
        ViewerAccess::Rejected => None,
        ViewerAccess::Creator => None,
        ViewerAccess::Participant { seat_number } => Some(*seat_number),
    };
    for (seat_number, rack) in dto.racks.iter_mut().enumerate() {
        if visible_seat != Some(seat_number as u8) {
            *rack = RackDto {
                counts: Vec::new(),
                blanks: 0,
            };
        }
    }
    if !matches!(access, ViewerAccess::Participant { .. }) {
        dto.messages.clear();
    }
    dto
}

/// Fills in each unclaimed Human seat's `invitation_status` from that
/// seat's invitation history. Deliberately a separate post-processing step
/// from `to_dto()` itself, same reasoning as `redact_game_state` — `to_dto`
/// is synchronous and has no database access, while invitation records
/// live in SQLite, so only the handlers where this is actually meaningful
/// (a `Waiting` game) fetch `invitations` (via
/// `persistence::get_invitations_for_game`) and call this; everywhere else
/// every seat is already claimed by the time it matters (`start_game`
/// requires full seating), so `invitation_status: None` from `to_dto()`
/// is already correct and this call is skipped entirely.
pub fn attach_invitation_status(dto: &mut GameStateDto, invitations: &[InvitationRecord]) {
    for participant in &mut dto.participants {
        if participant.kind != SeatKind::Human || participant.player_id.is_some() {
            continue;
        }
        // A seat can accumulate more than one invitation over time (sent,
        // rejected, resent) — the most recent one is what's live, matching
        // how `invite_player_to_game` itself only ever blocks on a
        // *pending* one existing already, not on history.
        let latest = invitations
            .iter()
            .filter(|invitation| invitation.seat_number == participant.seat_number)
            .max_by(|a, b| a.created_at.cmp(&b.created_at));
        participant.invitation_status = Some(match latest.map(|i| i.status.as_str()) {
            Some("pending") => SeatInvitationStatus::Pending,
            Some("rejected") => SeatInvitationStatus::Rejected,
            // "accepted" can't happen here (player_id would be Some, we'd
            // have `continue`d above) and "cancelled" means a past
            // invitation was superseded/withdrawn — both, plus no
            // invitation at all, mean this seat hasn't been (re)sent yet.
            _ => SeatInvitationStatus::NotSent,
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantState {
    pub seat_number: u8,
    pub kind: SeatKind,
    pub display_name: String,
    pub player_id: Option<String>,
    pub engine_id: Option<String>,
    pub score: i32,
    pub rack: Rack,
    pub resigned: bool,
    /// This seat's player has hidden the (finished) game from their own
    /// games list — see `GameSession::remove_for_player`. Purely a
    /// per-viewer display concern: the game itself, and every other
    /// participant's view of it, is completely unaffected. Missing on any
    /// game persisted before this field existed, same as `resigned` would
    /// be if it were added today.
    #[serde(default)]
    pub removed_by_player: bool,
    /// Set only for a seat created with `api::SeatClaim::Email`, and only
    /// until claimed — see `api::ParticipantDto.invited_email`'s doc
    /// comment. Missing on any game persisted before this field existed,
    /// same as `removed_by_player`.
    #[serde(default)]
    pub invited_email: Option<String>,
    /// The `turn_number` a "your time is running low" reminder email was
    /// last sent for on this seat, if any — compared against the live
    /// `turn_number` to tell whether one is still owed this turn. Never
    /// needs resetting: a new turn always has a `turn_number` that doesn't
    /// match whatever was stored here. Missing on any game persisted
    /// before this field existed, same as `removed_by_player`.
    #[serde(default)]
    pub reminder_sent_turn: Option<i64>,
}

/// Number of consecutive scoreless plays (passes or exchanges), summed
/// across all seats, that ends the game with no one going out. This is the
/// standard tournament rule; in heads-up (2-player) play it amounts to each
/// player passing three times in a row.
const SCORELESS_TURN_LIMIT: u8 = 6;

/// Default per-move time limit: 72 hours, chosen for async play where
/// opponents aren't expected to be online at the same time.
pub const DEFAULT_MOVE_TIME_LIMIT_SECONDS: u64 = 72 * 60 * 60;

#[derive(Debug, Clone)]
pub struct GameSession {
    pub id: String,
    pub status: GameStatus,
    pub variant: String,
    pub language: String,
    pub board_layout: String,
    pub turn_number: i64,
    pub current_seat: u8,
    pub winner_seat: Option<u8>,
    /// Set only when someone went out (emptied their rack) — the standard
    /// end-of-game rack bonus, where every other seat's remaining rack
    /// value is deducted from them and handed to whoever went out. A
    /// scoreless-turn-limit ending or a resignation/timeout has no such
    /// transfer, so these stay `None` for those.
    pub final_bonus_seat: Option<u8>,
    pub final_bonus_points: Option<i32>,
    /// Who created this game — distinct from `participants`, since a
    /// creator isn't necessarily seated in it (e.g. Engine vs Engine).
    /// Lets `list_games` show a game to the person who set it up even when
    /// they hold no seat at all. `None` only for games persisted before
    /// this field existed.
    pub creator_player_id: Option<String>,
    /// The creator has hidden this (finished) game from their own games
    /// list — the `creator_player_id` counterpart to
    /// `ParticipantState::removed_by_player`. Belongs to the game itself
    /// rather than to any seat because an unseated creator (e.g. an Engine
    /// vs Engine game set up to watch) has no seat to carry the flag —
    /// `remove_for_player` only ever sets this when the caller isn't seated
    /// at all; a seated creator's removal goes on their own seat instead,
    /// same as any other participant.
    pub removed_by_creator: bool,
    pub random_seed: u64,
    pub rules: VariantRules,
    pub state: GameState,
    pub bag: Vec<Tile>,
    pub participants: Vec<ParticipantState>,
    pub moves: Vec<MoveRecord>,
    pub messages: Vec<ChatMessageRecord>,
    pub consecutive_scoreless_turns: u8,
    pub move_time_limit_seconds: u64,
    /// Unix seconds (as a string, matching the rest of the codebase's
    /// timestamp convention) when `current_seat`'s turn began — reset every
    /// time the turn advances. Meaningless until the game is `Active`.
    pub turn_started_at: String,
}

impl GameSession {
    pub fn new(
        id: String,
        participants: Vec<ParticipantState>,
        creator_player_id: Option<String>,
        random_seed: u64,
        rules: VariantRules,
        move_time_limit_seconds: u64,
    ) -> Self {
        let dictionary = rules_shared::dictionary_by_name(&rules.language)
            .expect("game rules should reference a known dictionary");
        let state = GameState::new(&rules, dictionary);
        let mut bag = build_bag(&rules);
        shuffle_bag(&mut bag, random_seed);

        Self {
            id,
            status: GameStatus::Waiting,
            variant: rules.name.clone(),
            language: rules.language.clone(),
            // Board layout is bundled into the edition, not an independent
            // axis (see `VariantRules`) — the DTO field stays around for
            // display/API compatibility, mirroring the edition name.
            board_layout: rules.name.clone(),
            turn_number: 0,
            current_seat: 0,
            winner_seat: None,
            final_bonus_seat: None,
            final_bonus_points: None,
            creator_player_id,
            removed_by_creator: false,
            random_seed,
            rules,
            state,
            bag,
            participants,
            moves: Vec::new(),
            messages: Vec::new(),
            consecutive_scoreless_turns: 0,
            move_time_limit_seconds,
            turn_started_at: now_unix_seconds().to_string(),
        }
    }

    /// Swaps two seats' positions — and with them, turn order, since
    /// `current_seat`/rack refill/every other turn-taking mechanism walks
    /// `participants` by index (see `start`, `apply_place_move`, etc.), not
    /// by any separately-tracked ordering. Only meaningful before the game
    /// starts (turn order is fixed the moment play begins) and only once
    /// every seat is actually filled — this sidesteps the game's pending
    /// `game_invitations` rows (keyed by `seat_number`) ever going stale,
    /// since a pending invitation only exists for an unclaimed seat, and
    /// none can exist once every seat has a real occupant.
    pub fn swap_seats(&mut self, seat_a: u8, seat_b: u8) -> Result<(), String> {
        if self.status != GameStatus::Waiting {
            return Err("Seats can only be reordered before the game starts".to_string());
        }
        if self
            .participants
            .iter()
            .any(|participant| participant.kind == SeatKind::Human && participant.player_id.is_none())
        {
            return Err("Every seat must be filled before seats can be reordered".to_string());
        }
        let a = seat_a as usize;
        let b = seat_b as usize;
        if a >= self.participants.len() || b >= self.participants.len() {
            return Err("Unknown seat".to_string());
        }
        if a != b {
            self.participants.swap(a, b);
            self.participants[a].seat_number = a as u8;
            self.participants[b].seat_number = b as u8;
        }
        Ok(())
    }

    /// Adds a new seat to the roster — appended at the end regardless of
    /// whatever `seat.seat_number` the caller set, since `GameSession`
    /// alone owns seat numbering (see `swap_seats`'s doc comment on why
    /// every other seat-touching method treats it as authoritative). A
    /// fresh Human seat starts unclaimed with no invitation of its own —
    /// sending one is a separate, explicit follow-up call (see
    /// `invite_player_to_game` in app.rs), not part of adding the seat.
    pub fn add_seat(&mut self, mut seat: ParticipantState) -> Result<(), String> {
        if self.status != GameStatus::Waiting {
            return Err("Seats can only be added before the game starts".to_string());
        }
        seat.seat_number = self.participants.len() as u8;
        self.participants.push(seat);
        Ok(())
    }

    /// Removes a seat entirely — never the creator's own, and only before
    /// the game starts (once `Active`, `force_resign` is the equivalent
    /// action for an unresponsive seat; removing one mid-game would break
    /// move history/scoring that already depends on it existing). Works
    /// regardless of claim status — this is also how the creator kicks a
    /// confirmed participant, not just cancels an outstanding invite.
    /// Renumbers every subsequent seat to keep `participants[i].seat_number
    /// == i`, an invariant relied on throughout (`apply_place_move`,
    /// `apply_resign`, etc. all index straight into `participants` by seat
    /// number). The caller is responsible for keeping any `game_invitations`
    /// rows for those renumbered seats in sync — `GameSession` has no
    /// notion of invitations at all, that's a persistence-layer concern.
    pub fn remove_seat(&mut self, seat_number: u8) -> Result<(), String> {
        if self.status != GameStatus::Waiting {
            return Err("Seats can only be removed before the game starts".to_string());
        }
        let index = seat_number as usize;
        let participant = self
            .participants
            .get(index)
            .ok_or_else(|| "Unknown seat".to_string())?;
        if participant.player_id.is_some() && participant.player_id == self.creator_player_id {
            return Err("The creator's own seat can't be removed".to_string());
        }
        self.participants.remove(index);
        for (new_index, participant) in self.participants.iter_mut().enumerate() {
            participant.seat_number = new_index as u8;
        }
        Ok(())
    }

    /// Lets whoever holds a claimed non-creator seat give it back up before
    /// the game starts — the seat stays in the roster (same `seat_number`,
    /// still `Human`), just unclaimed again, unlike `remove_seat` which
    /// deletes it outright. The caller is responsible for flipping that
    /// seat's invitation back to `"rejected"`, same
    /// persistence-is-a-separate-concern reasoning as `remove_seat`.
    pub fn withdraw_seat(&mut self, seat_number: u8) -> Result<(), String> {
        if self.status != GameStatus::Waiting {
            return Err("A seat can only be withdrawn before the game starts".to_string());
        }
        let participant = self
            .participants
            .get_mut(seat_number as usize)
            .ok_or_else(|| "Unknown seat".to_string())?;
        if participant.player_id.is_none() {
            return Err("That seat isn't claimed".to_string());
        }
        if participant.player_id == self.creator_player_id {
            return Err("The creator's own seat can't be withdrawn".to_string());
        }
        participant.player_id = None;
        Ok(())
    }

    pub fn start(&mut self) {
        if self.status != GameStatus::Waiting {
            return;
        }

        for participant in &mut self.participants {
            refill_rack(&mut participant.rack, &mut self.bag, self.rules.rack_size);
        }

        self.status = GameStatus::Active;
        self.turn_number = 1;
        self.current_seat = 0;
        self.turn_started_at = now_unix_seconds().to_string();
    }

    /// Auto-retires the current seat if it has sat on its turn past
    /// `move_time_limit_seconds`, exactly as if that player had resigned —
    /// removes just this seat and moves play on to the next active one
    /// (see `handle_seat_exit`), same as `apply_resign`, just triggered by
    /// a deadline instead of a manual action. Returns whether anything
    /// changed, so callers know whether to persist and broadcast. There's
    /// no background scheduler in this server, so this is checked lazily
    /// whenever a game is touched (see `expire_overdue_turns` in `app.rs`)
    /// rather than firing exactly on the deadline.
    pub fn apply_move_timeout(&mut self) -> bool {
        if self.status != GameStatus::Active {
            return false;
        }
        let Ok(started) = self.turn_started_at.parse::<u64>() else {
            return false;
        };
        if now_unix_seconds().saturating_sub(started) < self.move_time_limit_seconds {
            return false;
        }

        let seat = self.current_seat;
        let Some(participant) = self.participants.get_mut(seat as usize) else {
            return false;
        };
        participant.resigned = true;
        let display_name = participant.display_name.clone();
        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number: seat,
            move_type: "timeout".to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: format!("{display_name} was retired for exceeding the move time limit"),
        });
        self.handle_seat_exit(seat);
        true
    }

    /// Seconds left before the current seat's turn times out (see
    /// `apply_move_timeout`), or `None` if the game isn't active or
    /// `turn_started_at` fails to parse. Saturates at zero rather than
    /// going negative once the deadline has already passed.
    pub fn seconds_remaining_on_turn(&self) -> Option<u64> {
        if self.status != GameStatus::Active {
            return None;
        }
        let started = self.turn_started_at.parse::<u64>().ok()?;
        let elapsed = now_unix_seconds().saturating_sub(started);
        Some(self.move_time_limit_seconds.saturating_sub(elapsed))
    }

    /// A lightweight summary for games-list views. `last_activity_at` is
    /// supplied by the caller since move timestamps live in the persistence
    /// layer, not on `GameSession` itself.
    pub fn to_summary_dto(&self, last_activity_at: String) -> api::GameSummaryDto {
        api::GameSummaryDto {
            id: self.id.clone(),
            status: self.status.clone(),
            variant: self.variant.clone(),
            current_seat: self.current_seat,
            participants: self
                .participants
                .iter()
                .map(|participant| ParticipantDto {
                    seat_number: participant.seat_number,
                    kind: participant.kind.clone(),
                    display_name: participant.display_name.clone(),
                    player_id: participant.player_id.clone(),
                    engine_id: participant.engine_id.clone(),
                    score: participant.score,
                    // Not meaningful in a list-view summary.
                    invitation_status: None,
                    invited_email: participant.invited_email.clone(),
                    rating_before: None,
                    rating_after: None,
                    current_rating: None,
                    resigned: participant.resigned,
                })
                .collect(),
            last_activity_at,
            move_time_limit_seconds: self.move_time_limit_seconds,
            turn_started_at: self.turn_started_at.clone(),
            last_message_at: self.messages.last().map(|message| message.created_at.clone()),
            // Caller-relative fields — `list_games` fills these in per
            // requester since "why does this game show up" depends on who's
            // asking, not on the game itself.
            relationship: api::GameRelationship::Participant,
            invitation_id: None,
        }
    }

    pub fn to_dto(&self) -> GameStateDto {
        GameStateDto {
            id: self.id.clone(),
            status: self.status.clone(),
            creator_player_id: self.creator_player_id.clone(),
            variant: self.variant.clone(),
            language: self.language.clone(),
            board_layout: self.board_layout.clone(),
            turn_number: self.turn_number,
            current_seat: self.current_seat,
            winner_seat: self.winner_seat,
            final_bonus_seat: self.final_bonus_seat,
            final_bonus_points: self.final_bonus_points,
            bag_count: self.bag.len(),
            move_time_limit_seconds: self.move_time_limit_seconds,
            turn_started_at: self.turn_started_at.clone(),
            participants: self
                .participants
                .iter()
                .map(|participant| ParticipantDto {
                    seat_number: participant.seat_number,
                    kind: participant.kind.clone(),
                    display_name: participant.display_name.clone(),
                    player_id: participant.player_id.clone(),
                    engine_id: participant.engine_id.clone(),
                    score: participant.score,
                    // Populated afterward, only by handlers where it's
                    // meaningful — see `attach_invitation_status` in app.rs
                    // and `ParticipantDto::invitation_status`'s doc comment.
                    invitation_status: None,
                    invited_email: participant.invited_email.clone(),
                    // Filled in afterward — see `stats::attach_current_ratings`
                    // (always, any status) and `stats::attach_rating_deltas`
                    // (only for a Finished game whose ending actually moved
                    // rating).
                    rating_before: None,
                    rating_after: None,
                    current_rating: None,
                    resigned: participant.resigned,
                })
                .collect(),
            board: board_to_dto(&self.state.board, &self.rules.alphabet),
            racks: self
                .participants
                .iter()
                .map(|participant| RackDto {
                    counts: participant.rack.counts.to_vec(),
                    blanks: participant.rack.blanks,
                })
                .collect(),
            moves: self
                .moves
                .iter()
                .map(|record| MoveRecordDto {
                    move_number: record.move_number,
                    seat_number: record.seat_number,
                    move_type: record.move_type.clone(),
                    main_word: record.main_word.clone(),
                    score_delta: record.score_delta,
                    positions: record.positions.clone(),
                    description: record.description.clone(),
                })
                .collect(),
            messages: self
                .messages
                .iter()
                .map(|record| ChatMessageDto {
                    id: record.id.clone(),
                    player_id: record.player_id.clone(),
                    display_name: record.display_name.clone(),
                    body: record.body.clone(),
                    created_at: record.created_at.clone(),
                })
                .collect(),
        }
    }

    /// Rejects unless `player_id` currently holds a seat — the same
    /// `resolve_viewer_access` rule used for chat *viewing* governs
    /// *posting* too, so the two can't drift apart. Not gated on game
    /// status: players can still chat after a game finishes, and for the
    /// week until it's auto-expired.
    pub fn post_chat_message(
        &mut self,
        player_id: &str,
        display_name: &str,
        body: String,
    ) -> Result<(), String> {
        if !matches!(
            resolve_viewer_access(self, Some(player_id)),
            ViewerAccess::Participant { .. }
        ) {
            return Err("Only seated players can chat in this game".to_string());
        }
        let body = body.trim().to_string();
        if body.is_empty() {
            return Err("Chat message cannot be empty".to_string());
        }
        if body.chars().count() > MAX_CHAT_MESSAGE_LEN {
            return Err(format!(
                "Chat message cannot exceed {MAX_CHAT_MESSAGE_LEN} characters"
            ));
        }
        self.messages.push(ChatMessageRecord {
            id: uuid::Uuid::new_v4().to_string(),
            player_id: player_id.to_string(),
            display_name: display_name.to_string(),
            body,
            created_at: now_unix_seconds().to_string(),
        });
        Ok(())
    }

    pub fn apply_place_move(
        &mut self,
        seat_number: u8,
        candidate: MoveCandidate,
    ) -> Result<(), String> {
        ensure_active_turn(self, seat_number)?;
        let rules_engine = RulesEngine {
            rules: &self.rules,
            dictionary: rules_shared::dictionary_by_name(&self.rules.language)
                .expect("game rules should reference a known dictionary"),
        };
        let participant = self
            .participants
            .get_mut(seat_number as usize)
            .ok_or_else(|| format!("Unknown seat {seat_number}"))?;
        let validated = rules_engine
            .validate_game_move(&self.state, Some(&participant.rack), &candidate)
            .map_err(|error| format_move_error(&error))?;

        for placement in &candidate.tiles {
            if !participant.rack.consume_tile(placement.tile) {
                return Err("Rack no longer matches move".to_string());
            }
        }

        rules_engine
            .apply_move_to_game(&mut self.state, &validated)
            .map_err(|error| format!("{error:?}"))?;

        participant.score += validated.score.total as i32;
        refill_rack(&mut participant.rack, &mut self.bag, self.rules.rack_size);
        let went_out = participant.rack.is_empty();
        let positions = candidate
            .tiles
            .iter()
            .map(|placement| match candidate.direction {
                Direction::Horizontal => PositionDto {
                    x: candidate.start.x + placement.offset,
                    y: candidate.start.y,
                },
                Direction::Vertical => PositionDto {
                    x: candidate.start.x,
                    y: candidate.start.y + placement.offset,
                },
            })
            .collect();
        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number,
            move_type: "place".to_string(),
            main_word: Some(validated.preview.main_word.clone()),
            score_delta: validated.score.total as i32,
            positions,
            description: format!(
                "{} played {} for {}",
                participant.display_name, validated.preview.main_word, validated.score.total
            ),
        });
        self.consecutive_scoreless_turns = 0;
        if went_out {
            self.finish_game(Some(seat_number));
        } else {
            self.advance_turn();
        }
        Ok(())
    }

    pub fn apply_pass(&mut self, seat_number: u8) -> Result<(), String> {
        ensure_active_turn(self, seat_number)?;
        let participant = self
            .participants
            .get(seat_number as usize)
            .ok_or_else(|| format!("Unknown seat {seat_number}"))?;
        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number,
            move_type: "pass".to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: format!("{} passed", participant.display_name),
        });
        self.consecutive_scoreless_turns += 1;
        if self.consecutive_scoreless_turns >= SCORELESS_TURN_LIMIT {
            self.finish_game(None);
        } else {
            self.advance_turn();
        }
        Ok(())
    }

    pub fn apply_exchange(&mut self, seat_number: u8, tiles: Vec<Tile>) -> Result<(), String> {
        ensure_active_turn(self, seat_number)?;
        if self.bag.len() < tiles.len() {
            return Err("Not enough tiles left in bag to exchange".to_string());
        }
        let participant = self
            .participants
            .get_mut(seat_number as usize)
            .ok_or_else(|| format!("Unknown seat {seat_number}"))?;

        for tile in &tiles {
            if !participant.rack.consume_tile(*tile) {
                return Err("Rack does not contain exchange tiles".to_string());
            }
        }

        self.bag.extend(tiles.iter().copied().map(reset_blank_tile));
        shuffle_bag(&mut self.bag, self.random_seed ^ self.turn_number as u64);
        refill_rack(&mut participant.rack, &mut self.bag, self.rules.rack_size);

        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number,
            move_type: "exchange".to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: format!(
                "{} exchanged {} tiles",
                participant.display_name,
                tiles.len()
            ),
        });
        self.consecutive_scoreless_turns += 1;
        if self.consecutive_scoreless_turns >= SCORELESS_TURN_LIMIT {
            self.finish_game(None);
        } else {
            self.advance_turn();
        }
        Ok(())
    }

    pub fn apply_resign(&mut self, seat_number: u8) -> Result<(), String> {
        ensure_active_turn(self, seat_number)?;
        self.finish_via_resignation(seat_number, "resign", "resigned")
    }

    /// The game manager's override: removes a seat on behalf of a player
    /// who isn't necessarily on turn — unlike self-`apply_resign` (which
    /// requires it to be exactly that seat's turn, since a player resigns
    /// *in place of* taking their move), a creator dealing with a player
    /// who's gone quiet needs to act regardless of whose turn it currently
    /// is. Same effect otherwise: see `finish_via_resignation` — this seat
    /// drops out, everyone else keeps playing (the whole game only ends
    /// once at most one active seat remains).
    pub fn force_resign(&mut self, seat_number: u8) -> Result<(), String> {
        if self.status != GameStatus::Active {
            return Err("The game must be active to force-resign a seat".to_string());
        }
        let participant = self
            .participants
            .get(seat_number as usize)
            .ok_or_else(|| format!("Unknown seat {seat_number}"))?;
        if participant.player_id.as_deref() == self.creator_player_id.as_deref()
            && participant.player_id.is_some()
        {
            return Err("The creator's own seat can't be force-resigned".to_string());
        }
        if participant.resigned {
            return Err("That seat has already resigned".to_string());
        }
        self.finish_via_resignation(seat_number, "force_resign", "resigned (by the game creator)")
    }

    /// `move_type` is `"resign"` for a voluntary self-resignation or
    /// `"force_resign"` for the creator's override — distinct values so
    /// downstream code (rating: a win by force-resignation doesn't move
    /// rating, unlike a win by voluntary resignation) can tell them apart
    /// without parsing `reason`/`description` text.
    ///
    /// Removes just this one seat — the game only ends outright once
    /// `handle_seat_exit` finds at most one active seat left; with more
    /// than that still playing, this seat simply drops out and everyone
    /// else continues, matching a real multi-player table where one
    /// player quitting shouldn't end the game for the rest.
    fn finish_via_resignation(&mut self, seat_number: u8, move_type: &str, reason: &str) -> Result<(), String> {
        let participant = self
            .participants
            .get_mut(seat_number as usize)
            .ok_or_else(|| format!("Unknown seat {seat_number}"))?;
        participant.resigned = true;
        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number,
            move_type: move_type.to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: format!("{} {reason}", participant.display_name),
        });
        self.handle_seat_exit(seat_number);
        Ok(())
    }

    /// The admin cleanup tool's own ending path (`admin_force_end_game` in
    /// app.rs) — for abandoned/stuck games, bypassing normal win/resign/
    /// timeout logic entirely (no `winner_seat`, no `resigned` flag, can
    /// even apply to a `Waiting` game with unclaimed seats). Pushes a
    /// terminal move so downstream code can tell this ending apart from
    /// every other kind — specifically, rating: an admin force-end
    /// shouldn't move anyone's ELO, same reasoning as a timeout or a
    /// creator-forced resignation not moving it either.
    pub fn admin_force_finish(&mut self) {
        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number: self.current_seat,
            move_type: "admin_force_end".to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: "Game force-ended by admin".to_string(),
        });
        self.status = GameStatus::Finished;
    }

    /// Hides a finished game from one seat's player — a purely per-viewer
    /// display concern (see `ParticipantState::removed_by_player`), not a
    /// deletion: the game keeps playing for/existing to everyone else, and
    /// still expires normally via the existing 7-day auto-delete regardless
    /// of who's removed it. Only offered for finished games, matching the
    /// UI's "Remove" button, which only appears once a game is over.
    pub fn remove_for_player(&mut self, player_id: &str) -> Result<(), String> {
        if self.status != GameStatus::Finished {
            return Err("Only finished games can be removed".to_string());
        }
        // A seated caller (including a creator who also claimed a seat,
        // e.g. the "vs Engine"/"Play Friend" presets) has their own seat to
        // carry the flag, checked first same as `resolve_viewer_access`
        // preferring `Participant` over `Creator`. Only an unseated
        // creator (e.g. watching an Engine vs Engine game) falls through to
        // the game-level flag, since they have no seat of their own.
        if let Some(participant) = self
            .participants
            .iter_mut()
            .find(|participant| participant.player_id.as_deref() == Some(player_id))
        {
            participant.removed_by_player = true;
            return Ok(());
        }
        if self.creator_player_id.as_deref() == Some(player_id) {
            self.removed_by_creator = true;
            return Ok(());
        }
        Err("You are not a participant in this game".to_string())
    }

    /// Runs the current seat's engine (if any) and applies its chosen action
    /// through exactly the same `apply_*` methods a human client's HTTP
    /// action would go through, so the server stays authoritative over
    /// engine-originated moves as much as human ones.
    ///
    /// The engine runs on a blocking-friendly thread pool (via
    /// `spawn_blocking`, since move search is CPU-bound, not I/O-bound) and
    /// is subject to `engine_timeout`. If the engine hasn't responded by
    /// then, the seat auto-passes rather than stalling the game.
    pub async fn maybe_run_engine_turn(
        &mut self,
        engines: &EngineRegistry,
        engine_timeout: Duration,
    ) -> Result<bool, String> {
        if self.status != GameStatus::Active {
            return Ok(false);
        }

        let current = self
            .participants
            .get(self.current_seat as usize)
            .ok_or_else(|| "Current seat missing".to_string())?;

        if current.kind != SeatKind::Engine {
            return Ok(false);
        }

        let engine_id = current
            .engine_id
            .clone()
            .ok_or_else(|| "Engine seat missing engine id".to_string())?;
        let engine = engines
            .find(&engine_id)
            .ok_or_else(|| format!("Unknown engine: {engine_id}"))?;
        if !engine
            .metadata()
            .supported_variants
            .iter()
            .any(|variant| variant == &self.variant)
        {
            return Err(format!(
                "Engine '{engine_id}' does not support the '{}' variant",
                self.variant
            ));
        }
        let rack = current.rack;
        let seat_number = self.current_seat;
        let state_snapshot = self.state.clone();
        let rules_snapshot = self.rules.clone();
        let time_budget_ms = engine_timeout.as_millis() as u64;

        let outcome = tokio::time::timeout(
            engine_timeout,
            tokio::task::spawn_blocking(move || {
                engine.choose_action(EngineRequest {
                    state: &state_snapshot,
                    seat_number,
                    rack: &rack,
                    rules: &rules_snapshot,
                    time_budget_ms: Some(time_budget_ms),
                })
            }),
        )
        .await;

        let response = match outcome {
            Ok(Ok(response)) => response,
            Ok(Err(join_error)) => {
                return Err(format!(
                    "Engine '{engine_id}' panicked while choosing a move: {join_error}"
                ));
            }
            Err(_elapsed) => {
                tracing::warn!(
                    game_id = %self.id,
                    engine_id,
                    seat_number,
                    budget_ms = engine_timeout.as_millis() as u64,
                    "engine exceeded its move budget; auto-passing"
                );
                self.apply_pass(seat_number)?;
                return Ok(true);
            }
        };

        match response.action {
            EngineAction::Place(candidate) => {
                self.apply_place_move(self.current_seat, candidate)?
            }
            EngineAction::Pass => self.apply_pass(self.current_seat)?,
            EngineAction::Exchange(tiles) => self.apply_exchange(self.current_seat, tiles)?,
            EngineAction::Resign => self.apply_resign(self.current_seat)?,
        }

        Ok(true)
    }

    /// Ends the game via the standard endgame scoring adjustment: every
    /// participant's score is reduced by the value of the tiles left on
    /// their rack, and if `goer_out` went out (emptied their rack while
    /// tiles remained for everyone else), they additionally receive the sum
    /// of every other rack's value. `goer_out` is `None` when the game ends
    /// by a scoreless-turn streak instead, in which case only the
    /// deductions apply.
    fn finish_game(&mut self, goer_out: Option<u8>) {
        let letter_values = self.rules.letter_values;
        let rack_value = |rack: &Rack| -> i32 {
            rack.counts
                .iter()
                .zip(letter_values.iter())
                .map(|(&count, &value)| count as i32 * value as i32)
                .sum::<i32>()
        };

        let mut opponents_total = 0i32;
        for participant in &mut self.participants {
            let value = rack_value(&participant.rack);
            if Some(participant.seat_number) != goer_out {
                opponents_total += value;
            }
            participant.score -= value;
        }
        if let Some(seat) = goer_out {
            if let Some(participant) = self
                .participants
                .iter_mut()
                .find(|participant| participant.seat_number == seat)
            {
                participant.score += opponents_total;
            }
            self.final_bonus_seat = Some(seat);
            self.final_bonus_points = Some(opponents_total);
        }

        self.status = GameStatus::Finished;
        self.winner_seat = self.compute_winner_seat();
    }

    /// Only among seats that were still active when the game ended — a
    /// seat that already resigned/timed out/was force-resigned earlier in
    /// this same game has a score frozen from whenever it left, which
    /// must never win purely because a plain unfiltered `max` happened to
    /// land on it (reachable the moment a game can continue with some
    /// seats already exited, see `handle_seat_exit`).
    fn compute_winner_seat(&self) -> Option<u8> {
        let max_score = self.participants.iter().filter(|p| !p.resigned).map(|p| p.score).max()?;
        let mut leaders = self
            .participants
            .iter()
            .filter(|participant| !participant.resigned && participant.score == max_score);
        let first = leaders.next()?;
        if leaders.next().is_some() {
            None
        } else {
            Some(first.seat_number)
        }
    }

    /// Checked before finding a next seat, not after — once a game can
    /// accumulate exits from several individual resignations/timeouts
    /// across its own history (see `handle_seat_exit`), a plain "advance
    /// to current+1" would loop forever hunting for an active seat if at
    /// most one remains. Also used directly by `handle_seat_exit`, which
    /// needs the identical check outside of a turn-advance (a
    /// force-resignation targeting a seat that isn't on turn still needs
    /// to end the game immediately if it was the second-to-last active
    /// seat, without otherwise touching whose turn it is).
    fn finish_if_one_or_fewer_active(&mut self) -> bool {
        if self.participants.iter().filter(|p| !p.resigned).count() > 1 {
            return false;
        }
        self.status = GameStatus::Finished;
        self.winner_seat = self
            .participants
            .iter()
            .find(|participant| !participant.resigned)
            .map(|participant| participant.seat_number);
        true
    }

    fn advance_turn(&mut self) {
        if self.status == GameStatus::Finished {
            return;
        }
        if self.finish_if_one_or_fewer_active() {
            return;
        }

        let seat_count = self.participants.len();
        let mut next_seat = (self.current_seat as usize + 1) % seat_count;
        while self.participants[next_seat].resigned {
            next_seat = (next_seat + 1) % seat_count;
        }
        self.current_seat = next_seat as u8;
        self.turn_number += 1;
        self.turn_started_at = now_unix_seconds().to_string();
    }

    /// Applies the turn-order consequence of a seat leaving the game
    /// (resignation, force-resignation, or a move timeout) — called after
    /// `participant.resigned` has already been set to `true` for
    /// `seat_number`. If this leaves at most one active seat, the game
    /// ends now. Otherwise, only if it was `seat_number`'s own turn does
    /// play move on (like a pass, via `advance_turn`) — a force-
    /// resignation targeting a seat that wasn't on turn leaves the
    /// current turn undisturbed, matching how `force_resign` already
    /// works today (it can target any seat regardless of whose turn it
    /// is).
    fn handle_seat_exit(&mut self, seat_number: u8) {
        if self.finish_if_one_or_fewer_active() {
            return;
        }
        if seat_number == self.current_seat {
            self.advance_turn();
        }
    }
}

fn now_unix_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs()
}

fn ensure_active_turn(session: &GameSession, seat_number: u8) -> Result<(), String> {
    if session.status != GameStatus::Active {
        return Err("Game is not active".to_string());
    }
    if session.current_seat != seat_number {
        return Err(format!("It is not seat {seat_number}'s turn"));
    }
    Ok(())
}

fn build_bag(rules: &VariantRules) -> Vec<Tile> {
    let mut bag = Vec::new();
    for (index, count) in rules.tile_distribution.iter().copied().enumerate() {
        for _ in 0..count {
            bag.push(Tile::Letter(Letter::from(index as u8)));
        }
    }
    for _ in 0..rules.blank_tiles {
        bag.push(Tile::Blank { acting_as: None });
    }
    bag
}

fn shuffle_bag(bag: &mut [Tile], seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    for index in (1..bag.len()).rev() {
        let swap_index = rng.gen_range(0..=index);
        bag.swap(index, swap_index);
    }
}

fn refill_rack(rack: &mut Rack, bag: &mut Vec<Tile>, rack_size: u8) {
    while rack.count() < rack_size && !bag.is_empty() {
        let tile = bag.pop().expect("checked non-empty bag");
        match tile {
            Tile::Letter(letter) => rack.add_letter(letter),
            Tile::Blank { .. } => rack.blanks += 1,
        }
    }
}

fn reset_blank_tile(tile: Tile) -> Tile {
    match tile {
        Tile::Blank { .. } => Tile::Blank { acting_as: None },
        Tile::Letter(letter) => Tile::Letter(letter),
    }
}

fn board_to_dto(board: &BoardState, alphabet: &Alphabet) -> Vec<BoardCellDto> {
    board
        .cells
        .iter()
        .map(|cell| match cell {
            BoardCell::Empty(empty) => BoardCellDto {
                premium: premium_to_dto(empty.premium),
                letter: None,
                is_blank: false,
            },
            BoardCell::Filled(FilledCell { letter, is_blank }) => BoardCellDto {
                premium: PremiumDto::Blank,
                letter: Some(to_dto_letter(*letter, alphabet)),
                is_blank: *is_blank,
            },
        })
        .collect()
}

pub fn board_from_dto(cells: &[BoardCellDto], alphabet: &Alphabet) -> Result<BoardState, String> {
    if cells.len() != BoardState::WIDTH * BoardState::HEIGHT {
        return Err(format!(
            "Expected {} board cells, got {}",
            BoardState::WIDTH * BoardState::HEIGHT,
            cells.len()
        ));
    }

    let mut board = BoardState::default();
    for (index, cell) in cells.iter().enumerate() {
        let x = (index % BoardState::WIDTH) as u8;
        let y = (index / BoardState::WIDTH) as u8;
        let pos = rules_shared::Position::new(x, y);
        let board_cell = match &cell.letter {
            Some(letter) => BoardCell::Filled(FilledCell {
                letter: to_rules_letter(letter, alphabet),
                is_blank: cell.is_blank,
            }),
            None => BoardCell::Empty(rules_shared::EmptyCell {
                premium: premium_from_dto(cell.premium.clone()),
            }),
        };
        board.set(pos, board_cell);
    }

    Ok(board)
}

fn premium_to_dto(premium: rules_shared::Premium) -> PremiumDto {
    match premium {
        rules_shared::Premium::Blank => PremiumDto::Blank,
        rules_shared::Premium::DoubleLetter => PremiumDto::DoubleLetter,
        rules_shared::Premium::TripleLetter => PremiumDto::TripleLetter,
        rules_shared::Premium::DoubleWord => PremiumDto::DoubleWord,
        rules_shared::Premium::TripleWord => PremiumDto::TripleWord,
    }
}

fn premium_from_dto(premium: PremiumDto) -> rules_shared::Premium {
    match premium {
        PremiumDto::Blank => rules_shared::Premium::Blank,
        PremiumDto::DoubleLetter => rules_shared::Premium::DoubleLetter,
        PremiumDto::TripleLetter => rules_shared::Premium::TripleLetter,
        PremiumDto::DoubleWord => rules_shared::Premium::DoubleWord,
        PremiumDto::TripleWord => rules_shared::Premium::TripleWord,
    }
}

pub fn move_candidate_from_dto(candidate: MoveCandidateDto, alphabet: &Alphabet) -> MoveCandidate {
    MoveCandidate {
        start: PositionDtoToRules::convert(candidate.start),
        direction: match candidate.direction {
            DirectionDto::Horizontal => Direction::Horizontal,
            DirectionDto::Vertical => Direction::Vertical,
        },
        tiles: candidate
            .tiles
            .into_iter()
            .map(|placement| TilePlacement {
                offset: placement.offset,
                tile: tile_from_dto(placement.tile, alphabet),
            })
            .collect(),
    }
}

pub fn move_candidate_to_dto(candidate: &MoveCandidate, alphabet: &Alphabet) -> MoveCandidateDto {
    MoveCandidateDto {
        start: PositionDto {
            x: candidate.start.x,
            y: candidate.start.y,
        },
        direction: match candidate.direction {
            Direction::Horizontal => DirectionDto::Horizontal,
            Direction::Vertical => DirectionDto::Vertical,
        },
        tiles: candidate
            .tiles
            .iter()
            .map(|placement| TilePlacementDto {
                offset: placement.offset,
                tile: tile_to_dto(placement.tile, alphabet),
            })
            .collect(),
    }
}

/// `rules_shared::Letter::from(char)`/`Letter::as_char()` are raw
/// ASCII-offset arithmetic — only correct for the standard Latin alphabet,
/// wrong for Ä/Ö/Ü (or any letter past index 25), and can't represent a
/// digraph tile (Spanish's CH/LL/RR) at all, since it's two characters.
/// Every tile/board letter crossing this boundary belongs to the game's
/// own `VariantRules.alphabet`, so this is a genuine internal invariant,
/// not defensive-for-user-input.
fn to_rules_letter(s: &str, alphabet: &Alphabet) -> Letter {
    alphabet
        .to_letter(s)
        .expect("tile letter should belong to the game's alphabet")
}

fn to_dto_letter(letter: Letter, alphabet: &Alphabet) -> String {
    alphabet
        .to_grapheme(letter)
        .expect("tile letter should belong to the game's alphabet")
        .to_string()
}

pub fn tile_from_dto(tile: TileDto, alphabet: &Alphabet) -> Tile {
    match tile {
        TileDto::Letter { letter } => Tile::Letter(to_rules_letter(&letter, alphabet)),
        TileDto::Blank { acting_as } => Tile::Blank {
            acting_as: acting_as.map(|letter| to_rules_letter(&letter, alphabet)),
        },
    }
}

pub fn tile_to_dto(tile: Tile, alphabet: &Alphabet) -> TileDto {
    match tile {
        Tile::Letter(letter) => TileDto::Letter {
            letter: to_dto_letter(letter, alphabet),
        },
        Tile::Blank { acting_as } => TileDto::Blank {
            acting_as: acting_as.map(|letter| to_dto_letter(letter, alphabet)),
        },
    }
}

fn engine_profile_from_metadata(metadata: &EngineMetadata) -> EngineProfileDto {
    EngineProfileDto {
        id: metadata.id.clone(),
        name: metadata.name.clone(),
        version: metadata.version.clone(),
        author: metadata.author.clone(),
        description: metadata.description.clone(),
        supports_timed_play: metadata.capabilities.supports_timed_play,
        supports_analysis: metadata.capabilities.supports_analysis,
        supports_ranking: metadata.capabilities.supports_ranking,
    }
}

struct PositionDtoToRules;

impl PositionDtoToRules {
    fn convert(position: PositionDto) -> rules_shared::Position {
        rules_shared::Position::new(position.x, position.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn participant(seat_number: u8, kind: SeatKind, player_id: Option<&str>) -> ParticipantState {
        ParticipantState {
            seat_number,
            kind,
            display_name: format!("Seat {seat_number}"),
            player_id: player_id.map(str::to_string),
            engine_id: None,
            score: 0,
            rack: Rack::default(),
            resigned: false,
            removed_by_player: false,
            invited_email: None,
            reminder_sent_turn: None,
        }
    }

    fn two_human_game(creator_player_id: Option<&str>) -> GameSession {
        GameSession::new(
            "game-1".to_string(),
            vec![
                participant(0, SeatKind::Human, Some("alice")),
                participant(1, SeatKind::Human, Some("bob")),
            ],
            creator_player_id.map(str::to_string),
            42,
            VariantRules::official(),
            3600,
        )
    }

    #[test]
    fn resolve_viewer_access_prefers_participant_over_creator() {
        // Alice both created the game and claimed seat 0 (the common "vs
        // Engine"/"Play Friend" preset case) — must resolve to the
        // strictly more-permissive `Participant`, not `Creator`.
        let game = two_human_game(Some("alice"));
        assert_eq!(
            resolve_viewer_access(&game, Some("alice")),
            ViewerAccess::Participant { seat_number: 0 }
        );
    }

    #[test]
    fn resolve_viewer_access_recognizes_an_unseated_creator() {
        let game = two_human_game(Some("carol"));
        assert_eq!(
            resolve_viewer_access(&game, Some("carol")),
            ViewerAccess::Creator
        );
    }

    #[test]
    fn resolve_viewer_access_recognizes_a_seated_non_creator() {
        let game = two_human_game(Some("carol"));
        assert_eq!(
            resolve_viewer_access(&game, Some("bob")),
            ViewerAccess::Participant { seat_number: 1 }
        );
    }

    #[test]
    fn resolve_viewer_access_rejects_an_unrelated_logged_in_player() {
        let game = two_human_game(Some("carol"));
        assert_eq!(
            resolve_viewer_access(&game, Some("mallory")),
            ViewerAccess::Rejected
        );
    }

    #[test]
    fn resolve_viewer_access_rejects_anyone_not_logged_in() {
        let game = two_human_game(Some("carol"));
        assert_eq!(resolve_viewer_access(&game, None), ViewerAccess::Rejected);
    }

    #[test]
    fn redact_game_state_creator_tier_hides_every_rack_and_all_chat() {
        let mut game = two_human_game(Some("carol"));
        game.participants[0].rack.counts[0] = 3;
        game.messages.push(ChatMessageRecord {
            id: "m1".to_string(),
            player_id: "alice".to_string(),
            display_name: "Alice".to_string(),
            body: "hi".to_string(),
            created_at: "0".to_string(),
        });
        let dto = redact_game_state(game.to_dto(), &ViewerAccess::Creator);
        assert!(dto.racks.iter().all(|rack| rack.counts.is_empty()));
        assert!(dto.messages.is_empty());
    }

    #[test]
    fn redact_game_state_participant_tier_keeps_only_their_own_rack() {
        let mut game = two_human_game(Some("carol"));
        game.participants[0].rack.counts[0] = 3;
        game.participants[1].rack.counts[1] = 5;
        game.messages.push(ChatMessageRecord {
            id: "m1".to_string(),
            player_id: "alice".to_string(),
            display_name: "Alice".to_string(),
            body: "hi".to_string(),
            created_at: "0".to_string(),
        });
        let dto = redact_game_state(game.to_dto(), &ViewerAccess::Participant { seat_number: 0 });
        // Seat 0 (this viewer) keeps their own rack contents...
        assert_eq!(dto.racks[0].counts[0], 3);
        // ...but seat 1's rack — the opponent's tiles — is redacted, not
        // leaked. This is the direct regression test for the original
        // finding: every seat's rack used to travel unconditionally.
        assert!(dto.racks[1].counts.is_empty());
        assert_eq!(dto.messages.len(), 1);
    }

    #[test]
    fn post_chat_message_rejects_a_non_seated_player() {
        let mut game = two_human_game(Some("carol"));
        let result = game.post_chat_message("mallory", "Mallory", "hi".to_string());
        assert!(result.is_err());
        assert!(game.messages.is_empty());
    }

    #[test]
    fn post_chat_message_rejects_an_unseated_creator() {
        let mut game = two_human_game(Some("carol"));
        let result = game.post_chat_message("carol", "Carol", "hi".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn post_chat_message_rejects_an_empty_body() {
        let mut game = two_human_game(Some("alice"));
        let result = game.post_chat_message("alice", "Alice", "   ".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn post_chat_message_rejects_an_over_length_body() {
        let mut game = two_human_game(Some("alice"));
        let body = "x".repeat(MAX_CHAT_MESSAGE_LEN + 1);
        let result = game.post_chat_message("alice", "Alice", body);
        assert!(result.is_err());
    }

    #[test]
    fn post_chat_message_appends_for_a_seated_player() {
        let mut game = two_human_game(Some("alice"));
        game.post_chat_message("bob", "Bob", "  gg  ".to_string())
            .expect("bob is seated and should be able to chat");
        assert_eq!(game.messages.len(), 1);
        assert_eq!(game.messages[0].player_id, "bob");
        assert_eq!(game.messages[0].display_name, "Bob");
        // Trimmed, matching `post_chat_message`'s own doc comment.
        assert_eq!(game.messages[0].body, "gg");
    }

    #[test]
    fn remove_for_player_rejects_a_game_that_is_not_finished() {
        let mut game = two_human_game(Some("alice"));
        let result = game.remove_for_player("alice");
        assert!(result.is_err());
    }

    #[test]
    fn remove_for_player_rejects_someone_not_seated_in_the_game() {
        let mut game = two_human_game(Some("alice"));
        game.start();
        game.apply_resign(0).expect("alice should be able to resign");
        let result = game.remove_for_player("carol");
        assert!(result.is_err());
    }

    #[test]
    fn remove_for_player_only_marks_the_calling_seat() {
        let mut game = two_human_game(Some("alice"));
        game.start();
        game.apply_resign(0).expect("alice should be able to resign");
        game.remove_for_player("alice")
            .expect("alice is seated in this finished game");
        assert!(game.participants[0].removed_by_player);
        assert!(!game.participants[1].removed_by_player);
    }

    #[test]
    fn remove_for_player_falls_back_to_the_game_level_flag_for_an_unseated_creator() {
        // An Engine vs Engine game: the creator holds no seat at all, so
        // there's no `ParticipantState` for `remove_for_player` to mark —
        // it should fall back to `GameSession.removed_by_creator` instead.
        let mut game = GameSession::new(
            "game-2".to_string(),
            vec![
                participant(0, SeatKind::Engine, None),
                participant(1, SeatKind::Engine, None),
            ],
            Some("carol".to_string()),
            42,
            VariantRules::official(),
            3600,
        );
        game.status = GameStatus::Finished;
        game.remove_for_player("carol")
            .expect("carol created this game and should be able to remove it");
        assert!(game.removed_by_creator);
        assert!(game.participants.iter().all(|p| !p.removed_by_player));
    }

    #[test]
    fn swap_seats_reorders_participants_and_updates_seat_numbers() {
        let mut game = two_human_game(Some("alice"));
        assert_eq!(game.participants[0].player_id.as_deref(), Some("alice"));
        assert_eq!(game.participants[1].player_id.as_deref(), Some("bob"));

        game.swap_seats(0, 1).expect("both seats are filled and the game hasn't started");

        assert_eq!(game.participants[0].player_id.as_deref(), Some("bob"));
        assert_eq!(game.participants[0].seat_number, 0);
        assert_eq!(game.participants[1].player_id.as_deref(), Some("alice"));
        assert_eq!(game.participants[1].seat_number, 1);
    }

    #[test]
    fn swap_seats_rejects_an_unknown_seat() {
        let mut game = two_human_game(Some("alice"));
        let result = game.swap_seats(0, 5);
        assert!(result.is_err());
    }

    #[test]
    fn swap_seats_rejects_once_the_game_has_started() {
        let mut game = two_human_game(Some("alice"));
        game.start();
        let result = game.swap_seats(0, 1);
        assert!(result.is_err());
    }

    #[test]
    fn swap_seats_rejects_while_a_human_seat_is_still_unclaimed() {
        let mut game = GameSession::new(
            "game-3".to_string(),
            vec![
                participant(0, SeatKind::Human, Some("alice")),
                participant(1, SeatKind::Human, None),
            ],
            Some("alice".to_string()),
            42,
            VariantRules::official(),
            3600,
        );
        let result = game.swap_seats(0, 1);
        assert!(result.is_err());
    }
}
