use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    Human,
    Engine,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GameStatus {
    Waiting,
    Active,
    Finished,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirectionDto {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PremiumDto {
    Blank,
    DoubleLetter,
    TripleLetter,
    DoubleWord,
    TripleWord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PositionDto {
    pub x: u8,
    pub y: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TileDto {
    Letter { letter: char },
    Blank { acting_as: Option<char> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TilePlacementDto {
    pub offset: u8,
    pub tile: TileDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MoveCandidateDto {
    pub start: PositionDto,
    pub direction: DirectionDto,
    pub tiles: Vec<TilePlacementDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayerActionDto {
    Place { candidate: MoveCandidateDto },
    Pass,
    Exchange { tiles: Vec<TileDto> },
    Resign,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameActionRequest {
    pub seat_number: u8,
    pub action: PlayerActionDto,
}

/// How a `Human` seat gets filled at game-creation time. Ignored for
/// `Engine` seats, which are always filled immediately.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SeatClaim {
    /// Bound immediately to the authenticated caller creating the game. At
    /// most one seat per request may use this.
    Creator,
    /// Pending until the named player accepts the invitation.
    Named { display_name: String },
    /// Pending until any logged-in player accepts — first to accept claims
    /// the seat.
    Open,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSeatRequest {
    pub kind: SeatKind,
    pub display_name: String,
    pub engine_id: Option<String>,
    /// Required for `Human` seats; ignored for `Engine` seats.
    pub claim: Option<SeatClaim>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateGameRequest {
    pub seats: Vec<CreateSeatRequest>,
    pub seed: Option<u64>,
    pub variant: Option<String>,
    pub language: Option<String>,
    pub board_layout: Option<String>,
    /// How long a seat may sit on its turn before being auto-retired.
    /// `None` falls back to the server default (72 hours).
    pub move_time_limit_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StartGameRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EngineProfileDto {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub supports_timed_play: bool,
    pub supports_analysis: bool,
    pub supports_ranking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParticipantDto {
    pub seat_number: u8,
    pub kind: SeatKind,
    pub display_name: String,
    pub player_id: Option<String>,
    pub engine_id: Option<String>,
    pub score: i32,
}

/// Why a game appears in a particular caller's `GET /games` list — the
/// server returns one flat, tagged list rather than pre-split buckets, so
/// the client can group/sort/filter however it wants.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GameRelationship {
    /// You hold a claimed seat, the game is active, and it's your turn.
    YourTurn,
    /// You hold a claimed seat, but it's not (currently) your turn.
    Participant,
    /// You've been invited by name to a specific seat.
    InvitedByName,
    /// A seat in this game is open to any logged-in player.
    InvitedOpen,
}

/// A lightweight summary of a game, cheap enough to fetch in bulk for a
/// games list. Deliberately excludes the board/rack/move-log detail that
/// `GameStateDto` carries — fetch the full game by id once it's selected.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameSummaryDto {
    pub id: String,
    pub status: GameStatus,
    pub current_seat: u8,
    pub participants: Vec<ParticipantDto>,
    /// Seconds since the Unix epoch (as a string, matching the server's
    /// storage format) of the most recent move, or the game's creation
    /// time if no moves have been made yet.
    pub last_activity_at: String,
    pub move_time_limit_seconds: u64,
    /// Seconds since the Unix epoch when `current_seat`'s turn began.
    /// Meaningless while `status` isn't `Active`.
    pub turn_started_at: String,
    pub relationship: GameRelationship,
    /// Set when `relationship` is `InvitedByName` or `InvitedOpen` — the
    /// invitation to accept/reject directly from the list.
    pub invitation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardCellDto {
    pub premium: PremiumDto,
    pub letter: Option<char>,
    pub is_blank: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RackDto {
    pub counts: [u8; 26],
    pub blanks: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MoveRecordDto {
    pub move_number: i64,
    pub seat_number: u8,
    pub move_type: String,
    pub main_word: Option<String>,
    pub score_delta: i32,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameStateDto {
    pub id: String,
    pub status: GameStatus,
    pub variant: String,
    pub language: String,
    pub board_layout: String,
    pub turn_number: i64,
    pub current_seat: u8,
    pub winner_seat: Option<u8>,
    pub bag_count: usize,
    pub move_time_limit_seconds: u64,
    /// Seconds since the Unix epoch when `current_seat`'s turn began.
    /// Meaningless while `status` isn't `Active`.
    pub turn_started_at: String,
    pub participants: Vec<ParticipantDto>,
    pub board: Vec<BoardCellDto>,
    pub racks: Vec<RackDto>,
    pub moves: Vec<MoveRecordDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GameEventDto {
    StateUpdated { game: GameStateDto },
    GameStarted { game: GameStateDto },
    GameFinished { game: GameStateDto },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiError {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewMoveRequest {
    pub seat_number: u8,
    pub candidate: MoveCandidateDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PreviewMoveResponse {
    pub is_legal: bool,
    pub headline: String,
    pub detail: String,
    pub score: Option<i16>,
}

// ========== Authentication ==========

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterPlayerRequest {
    pub display_name: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerSessionDto {
    pub player_id: String,
    pub session_token: String,
    pub display_name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoginPlayerRequest {
    pub display_name: String,
    pub password: String,
}

/// Self-service — the caller proves they know the current password rather
/// than relying solely on holding a valid session token (a "remember me"
/// token could otherwise be enough to hijack the account by itself).
/// Distinct from the admin CLI's `AdminResetPasswordRequest`, which is
/// loopback-gated and doesn't require the old password.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidateSessionRequest {
    pub session_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerDto {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

// ========== Game Invitations ==========

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvitationStatus {
    Pending,
    Accepted,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameInvitationDto {
    pub id: String,
    pub game_id: String,
    /// `None` for an open/stranger invitation.
    pub invited_player_id: Option<String>,
    pub inviting_player_id: String,
    pub seat_number: u8,
    pub status: InvitationStatus,
    pub created_at: String,
    pub responded_at: Option<String>,
    pub inviting_player_display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvitePlayerRequest {
    /// `None` invites any logged-in player (open/stranger) rather than one
    /// specific person by name.
    pub invited_display_name: Option<String>,
    pub seat_number: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvitationResponseRequest {
    pub accept: bool,
}

// ========== Admin ==========
//
// The /admin/* endpoints these types serve are restricted to loopback
// callers only (see `server-game`'s admin route guard) — there's no
// per-request auth beyond "you're running on the same machine as the
// server," so these types intentionally aren't reachable by the ordinary
// player-facing clients.

/// A game summary with `created_at`, for age-based filtering/display in the
/// admin CLI — the ordinary player-facing `GameSummaryDto` deliberately
/// doesn't carry this.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminGameSummaryDto {
    pub id: String,
    pub status: GameStatus,
    pub created_at: String,
    pub last_activity_at: String,
    pub participants: Vec<ParticipantDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminResetPasswordRequest {
    pub new_password: String,
}
