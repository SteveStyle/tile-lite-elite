use serde::{Deserialize, Serialize};

/// The API contract version this build implements. Both server and client
/// binaries embed whatever this constant was at *their own* build time, so
/// comparing a client's compiled-in value against what a server reports at
/// `/health` detects real drift (e.g. a desktop client that predates a
/// breaking server change) rather than just tautologically matching itself.
///
/// Bump `major` for a breaking change to routes/DTOs (old clients can't be
/// trusted to work — should be treated as incompatible), `minor` for an
/// additive/non-breaking one (old clients still work, just without whatever
/// the change added). There's deliberately no patch/build component here:
/// those never change the wire contract, so including them would make the
/// compatibility check fire on every routine bugfix deploy. Release/build
/// numbering for display purposes is a separate concern — see each
/// binary's own `app_version()`.
pub const API_VERSION: ApiVersion = ApiVersion { major: 1, minor: 0 };

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiVersion {
    pub major: u32,
    pub minor: u32,
}

impl std::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthDto {
    pub status: String,
    pub api_version: ApiVersion,
}

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
    /// One or two characters — most tiles are one, but a digraph tile
    /// (e.g. Spanish's CH/LL/RR) is a single physical tile/board
    /// square/rack slot that displays two.
    Letter { letter: String },
    Blank { acting_as: Option<String> },
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
    /// You created this game but hold no seat in it (e.g. an Engine vs
    /// Engine game you set up to watch).
    Creator,
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
    /// The bundled edition this game was created under (e.g. "official",
    /// "wordfeud", "north_american") — see `GameStateDto.variant`.
    pub variant: String,
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
    /// One or two characters — see `TileDto::Letter`.
    pub letter: Option<String>,
    pub is_blank: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RackDto {
    /// One count per letter in whichever alphabet the game's edition uses
    /// (26 for every Latin-alphabet edition, 29 for German, ...) — a `Vec`
    /// rather than a fixed-size array specifically so this crate doesn't
    /// need to depend on `rules_shared` just to know `MAX_ALPHABET_SIZE`,
    /// and so older/shorter snapshots on either side of the wire still
    /// deserialize fine (the receiving end pads to whatever width it needs).
    pub counts: Vec<u8>,
    pub blanks: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MoveRecordDto {
    pub move_number: i64,
    pub seat_number: u8,
    pub move_type: String,
    pub main_word: Option<String>,
    pub score_delta: i32,
    /// Board squares this move placed a tile on — empty for anything but
    /// `"place"`.
    pub positions: Vec<PositionDto>,
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
    /// Set only when the game ended because someone went out (emptied
    /// their rack) — the seat that received the standard end-of-game rack
    /// bonus (everyone else's remaining rack value), and how many points
    /// that was. `None` for a scoreless-turn-limit ending, a resignation,
    /// or a timeout, none of which involve that transfer.
    pub final_bonus_seat: Option<u8>,
    pub final_bonus_points: Option<i32>,
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
