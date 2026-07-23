use api::{BoardCellDto, GameStatus, SeatKind};
use serde::{Deserialize, Serialize};
use sqlx::{
    Pool, Row, Sqlite,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::str::FromStr;

use crate::game_state::{
    ChatMessageRecord, GameSession, MoveRecord, ParticipantState, board_from_dto,
};
use rules_shared::{Alphabet, GameState, Premium, Score, Tile, VariantRules};

/// Mirrors `rules_shared::VariantRules` field-for-field, but as its own type
/// rather than deriving `Serialize`/`Deserialize` directly on the internal
/// one — that type's shape is expected to keep evolving (board size,
/// alphabet width) as more editions/languages are added, and pinning the DB
/// schema straight to it would turn each of those changes into a data
/// migration. This struct is the DB's problem to keep stable; `VariantRules`
/// is free to change shape as long as the conversions below keep up.
///
/// `letter_values`/`tile_distribution` are `Vec<u8>` (not a fixed-size
/// array) specifically so a game persisted before the alphabet widened
/// beyond 26 letters (every game up to and including north_american/
/// wordfeud) still deserializes as-is — same reasoning, and the same
/// zero-pad-on-load technique, as `rules_shared::Rack.counts`'s existing
/// `deserialize_letter_array`. `alphabet` is `#[serde(default)]`'d to
/// `Alphabet::latin26()` for the same reason: every game persisted before
/// this field existed was implicitly that alphabet.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedVariantRules {
    name: String,
    language: String,
    letter_values: Vec<u8>,
    tile_distribution: Vec<u8>,
    #[serde(default = "default_latin26_alphabet")]
    alphabet: Alphabet,
    blank_tiles: u8,
    rack_size: u8,
    width: u8,
    height: u8,
    bingo_bonus: Score,
    premiums: Vec<Premium>,
}

fn default_latin26_alphabet() -> Alphabet {
    Alphabet::latin26()
}

impl From<&VariantRules> for PersistedVariantRules {
    fn from(rules: &VariantRules) -> Self {
        Self {
            name: rules.name.clone(),
            language: rules.language.clone(),
            letter_values: rules.letter_values.to_vec(),
            tile_distribution: rules.tile_distribution.to_vec(),
            alphabet: rules.alphabet.clone(),
            blank_tiles: rules.blank_tiles,
            rack_size: rules.rack_size,
            width: rules.width,
            height: rules.height,
            bingo_bonus: rules.bingo_bonus,
            premiums: rules.premiums.to_vec(),
        }
    }
}

impl TryFrom<PersistedVariantRules> for VariantRules {
    type Error = String;

    fn try_from(persisted: PersistedVariantRules) -> Result<Self, Self::Error> {
        let premiums: [Premium; 225] = persisted
            .premiums
            .try_into()
            .map_err(|_| "persisted premiums length did not match the board size".to_string())?;
        if persisted.letter_values.len() > rules_shared::MAX_ALPHABET_SIZE
            || persisted.tile_distribution.len() > rules_shared::MAX_ALPHABET_SIZE
        {
            return Err("persisted letter table longer than MAX_ALPHABET_SIZE".to_string());
        }
        let mut letter_values = [0u8; rules_shared::MAX_ALPHABET_SIZE];
        letter_values[..persisted.letter_values.len()].copy_from_slice(&persisted.letter_values);
        let mut tile_distribution = [0u8; rules_shared::MAX_ALPHABET_SIZE];
        tile_distribution[..persisted.tile_distribution.len()]
            .copy_from_slice(&persisted.tile_distribution);
        Ok(VariantRules {
            name: persisted.name,
            alphabet: persisted.alphabet,
            language: persisted.language,
            letter_values,
            tile_distribution,
            blank_tiles: persisted.blank_tiles,
            rack_size: persisted.rack_size,
            width: persisted.width,
            height: persisted.height,
            bingo_bonus: persisted.bingo_bonus,
            premiums,
        })
    }
}

/// A game snapshot persisted before per-game rules existed has no `rules`
/// field to deserialize — falls back to official, matching the hardcoded
/// behavior every such game was actually created and played under.
fn default_persisted_variant_rules() -> PersistedVariantRules {
    PersistedVariantRules::from(&VariantRules::official())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedGame {
    id: String,
    status: GameStatus,
    variant: String,
    language: String,
    board_layout: String,
    #[serde(default = "default_persisted_variant_rules")]
    rules: PersistedVariantRules,
    turn_number: i64,
    current_seat: u8,
    winner_seat: Option<u8>,
    #[serde(default)]
    final_bonus_seat: Option<u8>,
    #[serde(default)]
    final_bonus_points: Option<i32>,
    // Missing on any game persisted before this field existed — `None` for
    // those, same as `GameSession.creator_player_id`.
    #[serde(default)]
    creator_player_id: Option<String>,
    #[serde(default)]
    removed_by_creator: bool,
    random_seed: u64,
    board: Vec<BoardCellDto>,
    bag: Vec<Tile>,
    participants: Vec<ParticipantState>,
    moves: Vec<MoveRecord>,
    // Missing on any game persisted before chat existed — `Vec::new()` for
    // those, same pattern as `creator_player_id`.
    #[serde(default)]
    messages: Vec<ChatMessageRecord>,
    #[serde(default)]
    consecutive_scoreless_turns: u8,
    #[serde(default = "default_move_time_limit_seconds")]
    move_time_limit_seconds: u64,
    // Defaults to "now" rather than e.g. "0" — an old snapshot predating
    // this field has no real turn-start time to recover, and defaulting to
    // the epoch would make it look instantly overdue the moment it's
    // reloaded, retiring whoever's turn it is before they get a chance.
    #[serde(default = "now_iso")]
    turn_started_at: String,
}

fn default_move_time_limit_seconds() -> u64 {
    crate::game_state::DEFAULT_MOVE_TIME_LIMIT_SECONDS
}

pub async fn connect(database_url: &str) -> Result<Pool<Sqlite>, sqlx::Error> {
    // WAL rather than the default rollback-journal mode: readers (game
    // fetches, the games list, WebSocket state pushes) no longer block
    // behind a writer's transaction, which matters once more than a
    // handful of players are active concurrently. Persists in the database
    // file itself once set — see docs/4.1-configuration.md's "Infrastructure
    // Configuration" section for the pragma verification this replaced.
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Embedded at compile time from `crates/server-game/migrations/` (path is
/// relative to this crate's `CARGO_MANIFEST_DIR`) — see that directory's
/// `README.md` for the rules around adding a new migration.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn migrate(pool: &Pool<Sqlite>) -> Result<(), sqlx::Error> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

pub async fn save_game(pool: &Pool<Sqlite>, session: &GameSession) -> Result<(), sqlx::Error> {
    let now = now_iso();
    let mut tx = pool.begin().await?;

    // Captured before any writes, so the rating step below can tell
    // whether this save is the *first* time this game becomes Finished —
    // the single-fire gate that both makes rating idempotent (re-saving an
    // already-finished game, e.g. via `remove_for_player`, must not re-rate
    // it) and permanently excludes any game that was already 'finished'
    // before this feature shipped (no backfill).
    let prior = sqlx::query("select status, stats_settled_at from games where id = ?1")
        .bind(&session.id)
        .fetch_optional(&mut *tx)
        .await?;
    let prior_status: Option<String> = prior.as_ref().map(|row| row.get(0));
    let prior_stats_settled_at: Option<String> = prior.as_ref().and_then(|row| row.get(1));

    let snapshot_json = serde_json::to_string(&PersistedGame {
        id: session.id.clone(),
        status: session.status,
        variant: session.variant.clone(),
        language: session.language.clone(),
        board_layout: session.board_layout.clone(),
        rules: PersistedVariantRules::from(&session.rules),
        turn_number: session.turn_number,
        current_seat: session.current_seat,
        winner_seat: session.winner_seat,
        final_bonus_seat: session.final_bonus_seat,
        final_bonus_points: session.final_bonus_points,
        creator_player_id: session.creator_player_id.clone(),
        removed_by_creator: session.removed_by_creator,
        random_seed: session.random_seed,
        board: session.to_dto().board,
        bag: session.bag.clone(),
        participants: session.participants.clone(),
        moves: session.moves.clone(),
        messages: session.messages.clone(),
        consecutive_scoreless_turns: session.consecutive_scoreless_turns,
        move_time_limit_seconds: session.move_time_limit_seconds,
        turn_started_at: session.turn_started_at.clone(),
    })
    .expect("game session should serialize");

    sqlx::query(
        "insert into games (
            id, created_at, started_at, ended_at, status, variant, language, board_layout,
            turn_number, current_seat, winner_seat, random_seed, snapshot_json
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        on conflict(id) do update set
            started_at = excluded.started_at,
            ended_at = excluded.ended_at,
            status = excluded.status,
            variant = excluded.variant,
            language = excluded.language,
            board_layout = excluded.board_layout,
            turn_number = excluded.turn_number,
            current_seat = excluded.current_seat,
            winner_seat = excluded.winner_seat,
            random_seed = excluded.random_seed,
            snapshot_json = excluded.snapshot_json",
    )
    .bind(&session.id)
    .bind(&now)
    .bind(if session.status == GameStatus::Waiting {
        None::<String>
    } else {
        Some(now.clone())
    })
    .bind(if session.status == GameStatus::Finished {
        Some(now.clone())
    } else {
        None::<String>
    })
    .bind(status_name(&session.status))
    .bind(&session.variant)
    .bind(&session.language)
    .bind(&session.board_layout)
    .bind(session.turn_number)
    .bind(session.current_seat as i64)
    .bind(session.winner_seat.map(i64::from))
    .bind(session.random_seed as i64)
    .bind(snapshot_json)
    .execute(&mut *tx)
    .await?;

    sqlx::query("delete from game_participants where game_id = ?1")
        .bind(&session.id)
        .execute(&mut *tx)
        .await?;

    // Recomputed on every save, not just the first time a game finishes —
    // it's a pure function of already-immutable-once-Finished session
    // state, so recomputing is always cheap and always reproduces the same
    // values (harmless on a re-save, e.g. `remove_for_player`). `None`
    // outcome/0 bingo_count for a game still in progress.
    let outcomes = crate::stats::compute_participant_outcomes(session);
    for participant in &session.participants {
        let outcome = outcomes
            .iter()
            .find(|o| o.seat_number == participant.seat_number)
            .expect("compute_participant_outcomes covers every participant");
        sqlx::query(
            "insert into game_participants (
                id, game_id, seat_number, kind, display_name, player_id, engine_id, score, joined_at, outcome, bingo_count
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )
        .bind(format!("{}-seat-{}", session.id, participant.seat_number))
        .bind(&session.id)
        .bind(participant.seat_number as i64)
        .bind(kind_name(&participant.kind))
        .bind(&participant.display_name)
        .bind(&participant.player_id)
        .bind(&participant.engine_id)
        .bind(participant.score)
        .bind(&now)
        .bind(outcome.outcome.map(crate::stats::Outcome::as_db_str))
        .bind(outcome.bingo_count)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("delete from game_moves where game_id = ?1")
        .bind(&session.id)
        .execute(&mut *tx)
        .await?;

    for record in &session.moves {
        sqlx::query(
            "insert into game_moves (
                id, game_id, move_number, seat_number, move_type, payload_json, score_delta, created_at
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(format!("{}-move-{}", session.id, record.move_number))
        .bind(&session.id)
        .bind(record.move_number)
        .bind(record.seat_number as i64)
        .bind(&record.move_type)
        .bind(serde_json::to_string(record).expect("move record should serialize"))
        .bind(record.score_delta)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("delete from game_messages where game_id = ?1")
        .bind(&session.id)
        .execute(&mut *tx)
        .await?;

    for record in &session.messages {
        sqlx::query(
            "insert into game_messages (
                id, game_id, player_id, display_name, body, created_at
            ) values (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&record.id)
        .bind(&session.id)
        .bind(&record.player_id)
        .bind(&record.display_name)
        .bind(&record.body)
        .bind(&record.created_at)
        .execute(&mut *tx)
        .await?;
    }

    // The single-fire gate: only the save that actually transitions this
    // game into Finished for the first time ever runs the rating step —
    // never a legacy game that was already 'finished' before this column
    // existed (no backfill), and never a repeat save of an already-rated
    // game.
    if session.status == GameStatus::Finished
        && prior_status.as_deref() != Some("finished")
        && prior_stats_settled_at.is_none()
    {
        crate::stats::settle_ratings(&mut tx, session, &now).await?;
        sqlx::query("update games set stats_settled_at = ?2 where id = ?1")
            .bind(&session.id)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn load_game(pool: &Pool<Sqlite>, id: &str) -> Result<Option<GameSession>, sqlx::Error> {
    let row = sqlx::query("select snapshot_json from games where id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|row| row.get::<String, _>(0)).map(|json| {
        let persisted =
            serde_json::from_str::<PersistedGame>(&json).expect("persisted game should parse");
        let rules = VariantRules::try_from(persisted.rules)
            .expect("persisted variant rules should be valid");
        let board = board_from_dto(&persisted.board, &rules.alphabet)
            .expect("persisted board should be valid");
        let dictionary = rules_shared::dictionary_by_name(&rules.language)
            .expect("persisted rules should reference a known dictionary");
        let state = GameState::from_board(board, &rules, dictionary);
        GameSession {
            id: persisted.id,
            status: persisted.status,
            variant: persisted.variant,
            language: persisted.language,
            board_layout: persisted.board_layout,
            turn_number: persisted.turn_number,
            current_seat: persisted.current_seat,
            winner_seat: persisted.winner_seat,
            final_bonus_seat: persisted.final_bonus_seat,
            final_bonus_points: persisted.final_bonus_points,
            creator_player_id: persisted.creator_player_id,
            removed_by_creator: persisted.removed_by_creator,
            random_seed: persisted.random_seed,
            rules,
            state,
            bag: persisted.bag,
            participants: persisted.participants,
            moves: persisted.moves,
            messages: persisted.messages,
            consecutive_scoreless_turns: persisted.consecutive_scoreless_turns,
            move_time_limit_seconds: persisted.move_time_limit_seconds,
            turn_started_at: persisted.turn_started_at,
        }
    }))
}

pub async fn list_game_ids(pool: &Pool<Sqlite>) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("select id from games order by created_at desc")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<String, _>(0))
        .collect())
}

/// Games finished more than `cutoff` ago — `ended_at` is set once, in
/// `save_game`, the moment a game's status becomes `Finished`; it's only
/// ever tracked here in SQL, never on `GameSession` itself, since nothing
/// else needs to read it back.
pub async fn list_finished_game_ids_older_than(
    pool: &Pool<Sqlite>,
    cutoff: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("select id from games where status = 'finished' and ended_at < ?1")
        .bind(cutoff)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<String, _>(0))
        .collect())
}

/// Last-activity timestamp per game id: the most recent move's `created_at`,
/// falling back to the game's own `created_at` if no moves have been made
/// yet. Used to power the games-list panel without needing a dedicated
/// `updated_at` column on `games`.
pub async fn last_activity_by_game(
    pool: &Pool<Sqlite>,
) -> Result<std::collections::HashMap<String, String>, sqlx::Error> {
    let rows = sqlx::query(
        "select
            g.id,
            coalesce(
                (select max(m.created_at) from game_moves m where m.game_id = g.id),
                g.created_at
            ) as last_activity_at
         from games g",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| (row.get::<String, _>(0), row.get::<String, _>(1)))
        .collect())
}

/// `created_at` per game id — a separate lookup rather than a field on
/// `GameSession` itself, same reasoning as `last_activity_by_game`: nothing
/// in the in-memory session model needs it, only the admin listing does.
pub async fn created_at_by_game(
    pool: &Pool<Sqlite>,
) -> Result<std::collections::HashMap<String, String>, sqlx::Error> {
    let rows = sqlx::query("select id, created_at from games")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.get::<String, _>(0), row.get::<String, _>(1)))
        .collect())
}

// ========== Admin Functions ==========

pub async fn list_players(pool: &Pool<Sqlite>) -> Result<Vec<PlayerRecord>, sqlx::Error> {
    let rows = sqlx::query(
        "select id, display_name, email, password_hash, created_at, updated_at, last_seen_at
         from players order by created_at desc",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| PlayerRecord {
            id: r.get(0),
            display_name: r.get(1),
            email: r.get(2),
            password_hash: r.get(3),
            created_at: r.get(4),
            updated_at: r.get(5),
            last_seen_at: r.get(6),
        })
        .collect())
}

pub async fn update_player_password(
    pool: &Pool<Sqlite>,
    player_id: &str,
    password_hash: &str,
) -> Result<bool, sqlx::Error> {
    let now = now_iso();
    let result =
        sqlx::query("update players set password_hash = ?1, updated_at = ?2 where id = ?3")
            .bind(password_hash)
            .bind(&now)
            .bind(player_id)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Updates display name and/or email — unlike `update_player_password`,
/// doesn't invalidate other sessions (see `UpdatePlayerDetailsRequest`'s doc
/// comment on why these don't need the same "start fresh" treatment). Only
/// the fields that are `Some` change; a `None` leaves that column as-is.
pub async fn update_player_details(
    pool: &Pool<Sqlite>,
    player_id: &str,
    display_name: Option<&str>,
    email: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let now = now_iso();
    let result = sqlx::query(
        "update players
         set display_name = coalesce(?1, display_name),
             email = coalesce(?2, email),
             updated_at = ?3
         where id = ?4",
    )
    .bind(display_name)
    .bind(email)
    .bind(&now)
    .bind(player_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Signs out every session for this player — used after a self-service
/// password change, so a leaked/stolen session token stops working the
/// moment the account holder reacts to it, rather than staying valid
/// indefinitely alongside the new password.
pub async fn invalidate_sessions_for_player(
    pool: &Pool<Sqlite>,
    player_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from sessions where player_id = ?1")
        .bind(player_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Deletes a player along with their sessions and invitations, but
/// preserves game history: `game_participants.player_id` is unclaimed
/// (set to null) rather than deleting the participant row or the game,
/// matching how an anonymous, never-claimed seat already behaves — the
/// seat and its moves stay, just no longer bound to an account.
pub async fn delete_player(pool: &Pool<Sqlite>, player_id: &str) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("delete from sessions where player_id = ?1")
        .bind(player_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "delete from game_invitations where invited_player_id = ?1 or inviting_player_id = ?1",
    )
    .bind(player_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("update game_participants set player_id = null where player_id = ?1")
        .bind(player_id)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query("delete from players where id = ?1")
        .bind(player_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

/// Deletes a game and everything that belongs to it (participants, moves,
/// chat, invitations). Doesn't touch player accounts. Caller is responsible
/// for also dropping it from the in-memory `AppState.games` map — this only
/// handles the database side.
pub async fn delete_game(pool: &Pool<Sqlite>, game_id: &str) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("delete from game_moves where game_id = ?1")
        .bind(game_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from game_messages where game_id = ?1")
        .bind(game_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from game_participants where game_id = ?1")
        .bind(game_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from game_invitations where game_id = ?1")
        .bind(game_id)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query("delete from games where id = ?1")
        .bind(game_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

pub async fn upsert_engine_profiles(
    pool: &Pool<Sqlite>,
    profiles: &[api::EngineProfileDto],
) -> Result<(), sqlx::Error> {
    let now = now_iso();
    for profile in profiles {
        let capabilities_json = serde_json::json!({
            "supports_timed_play": profile.supports_timed_play,
            "supports_analysis": profile.supports_analysis,
            "supports_ranking": profile.supports_ranking,
        })
        .to_string();

        sqlx::query(
            "insert into engine_profiles (
                id, name, version, author, description, capabilities_json, created_at, updated_at
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            on conflict(id) do update set
                name = excluded.name,
                version = excluded.version,
                author = excluded.author,
                description = excluded.description,
                capabilities_json = excluded.capabilities_json,
                updated_at = excluded.updated_at",
        )
        .bind(&profile.id)
        .bind(&profile.name)
        .bind(&profile.version)
        .bind(&profile.author)
        .bind(&profile.description)
        .bind(capabilities_json)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await?;
    }

    Ok(())
}

// ========== Authentication Functions ==========

#[derive(Debug, Clone)]
pub struct PlayerRecord {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub password_hash: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_seen_at: Option<String>,
}

pub async fn create_player(
    pool: &Pool<Sqlite>,
    id: &str,
    display_name: &str,
    email: &str,
    password_hash: &str,
) -> Result<PlayerRecord, sqlx::Error> {
    let now = now_iso();
    sqlx::query(
        "insert into players (id, display_name, email, password_hash, created_at, updated_at)
         values (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(id)
    .bind(display_name)
    .bind(email)
    .bind(password_hash)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(PlayerRecord {
        id: id.to_string(),
        display_name: display_name.to_string(),
        email: email.to_string(),
        password_hash: password_hash.to_string(),
        created_at: now.clone(),
        updated_at: now,
        last_seen_at: None,
    })
}

pub async fn get_player_by_name(
    pool: &Pool<Sqlite>,
    display_name: &str,
) -> Result<Option<PlayerRecord>, sqlx::Error> {
    let row = sqlx::query(
        "select id, display_name, email, password_hash, created_at, updated_at, last_seen_at
         from players where display_name = ?1",
    )
    .bind(display_name)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| PlayerRecord {
        id: r.get(0),
        display_name: r.get(1),
        email: r.get(2),
        password_hash: r.get(3),
        created_at: r.get(4),
        updated_at: r.get(5),
        last_seen_at: r.get(6),
    }))
}

/// Case-sensitive exact match, same as `get_player_by_name` — email
/// normalization (trimming, lowercasing) is the caller's job, matching how
/// `register_player`/`login_player` already treat `display_name`.
///
/// `players.email` deliberately has no `unique` constraint (unlike
/// `display_name`) — one person legitimately running several identities
/// under the same email (the project owner's own multi-account testing
/// setup, at minimum) is an accepted use case, not a bug. The tradeoff this
/// creates: if duplicates exist, `fetch_optional` returns whichever row the
/// query planner visits first, so a `/auth/forgot-password` request can
/// only ever reach one of that email's accounts, arbitrarily, never "all
/// accounts with this email" or "let the requester choose." Acceptable for
/// now — this app has no real user base depending on account recovery — but
/// worth revisiting (e.g. reset-all-matching-accounts, or a picker) if that
/// ever changes.
pub async fn get_player_by_email(
    pool: &Pool<Sqlite>,
    email: &str,
) -> Result<Option<PlayerRecord>, sqlx::Error> {
    let row = sqlx::query(
        "select id, display_name, email, password_hash, created_at, updated_at, last_seen_at
         from players where email = ?1",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| PlayerRecord {
        id: r.get(0),
        display_name: r.get(1),
        email: r.get(2),
        password_hash: r.get(3),
        created_at: r.get(4),
        updated_at: r.get(5),
        last_seen_at: r.get(6),
    }))
}

/// Case-insensitive prefix match on `display_name`, for the "invite by
/// name" autocomplete — a live-typing UX needs something a caller can
/// actually browse toward, unlike `get_player_by_name`'s exact match. Only
/// returns display names (not full records): nothing else about a player
/// belongs in a search-suggestions payload. `limit` keeps a broad/short
/// query (e.g. a single letter) from returning the whole players table.
pub async fn search_players_by_name_prefix(
    pool: &Pool<Sqlite>,
    prefix: &str,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let pattern = format!("{}%", prefix.replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query(
        "select display_name from players
         where display_name like ?1 escape '\\' collate nocase
         order by display_name
         limit ?2",
    )
    .bind(pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| r.get(0)).collect())
}

pub async fn get_player_by_id(
    pool: &Pool<Sqlite>,
    id: &str,
) -> Result<Option<PlayerRecord>, sqlx::Error> {
    let row = sqlx::query(
        "select id, display_name, email, password_hash, created_at, updated_at, last_seen_at
         from players where id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| PlayerRecord {
        id: r.get(0),
        display_name: r.get(1),
        email: r.get(2),
        password_hash: r.get(3),
        created_at: r.get(4),
        updated_at: r.get(5),
        last_seen_at: r.get(6),
    }))
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub player_id: String,
    pub token_hash: Option<String>,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: Option<String>,
    /// Captured from `RegisterPlayerRequest`/`LoginPlayerRequest` at
    /// creation time — stored explicitly (not just inferred from
    /// `expires_at` being `None`) so it's a readable record of intent
    /// rather than something reconstructed from another field's absence.
    pub stay_logged_in: bool,
}

/// Absolute maximum session lifetime. Even a continuously-active session
/// must re-authenticate once it passes this — `expires_at` is set to
/// `created_at + this` at creation and never extended. Uniform for every
/// session: `stay_logged_in` no longer affects server-side expiry, it's
/// purely a client concern (whether the token is persisted across a browser
/// restart). See `app::auth::session_expiry`.
pub const SESSION_MAX_LIFETIME_SECS: u64 = 10 * 24 * 60 * 60;

/// Idle window: a session unused (no authenticated request) for longer than
/// this is treated as logged out, whichever comes first with the absolute
/// cap above. Sliding — `last_seen_at` is bumped on activity (throttled by
/// `LAST_SEEN_BUMP_THROTTLE_SECS`). Enforced in `app::common::player_id_for_token`
/// (rejects the token) and in `delete_expired_sessions` (prunes the row).
pub const SESSION_IDLE_WINDOW_SECS: u64 = 48 * 60 * 60;

/// Only rewrite `last_seen_at` when it's staler than this, so an actively
/// used session doesn't cause a DB write on every single request (the idle
/// window is days, so minute-scale precision is plenty).
pub const LAST_SEEN_BUMP_THROTTLE_SECS: u64 = 15 * 60;

pub async fn create_session(
    pool: &Pool<Sqlite>,
    id: &str,
    player_id: &str,
    token_hash: &str,
    stay_logged_in: bool,
    expires_at: Option<&str>,
) -> Result<SessionRecord, sqlx::Error> {
    let now = now_iso();
    sqlx::query(
        "insert into sessions (id, player_id, token_hash, created_at, last_seen_at, expires_at, stay_logged_in)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(id)
    .bind(player_id)
    .bind(token_hash)
    .bind(&now)
    .bind(&now)
    .bind(expires_at)
    .bind(stay_logged_in)
    .execute(pool)
    .await?;

    Ok(SessionRecord {
        id: id.to_string(),
        player_id: player_id.to_string(),
        token_hash: Some(token_hash.to_string()),
        created_at: now.clone(),
        last_seen_at: now,
        expires_at: expires_at.map(|s| s.to_string()),
        stay_logged_in,
    })
}

pub async fn get_session_by_token_hash(
    pool: &Pool<Sqlite>,
    token_hash: &str,
) -> Result<Option<SessionRecord>, sqlx::Error> {
    let row = sqlx::query(
        "select id, player_id, token_hash, created_at, last_seen_at, expires_at, stay_logged_in
         from sessions where token_hash = ?1",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| SessionRecord {
        id: r.get(0),
        player_id: r.get(1),
        token_hash: r.get(2),
        created_at: r.get(3),
        last_seen_at: r.get(4),
        expires_at: r.get(5),
        stay_logged_in: r.get(6),
    }))
}

pub async fn get_session_by_id(
    pool: &Pool<Sqlite>,
    id: &str,
) -> Result<Option<SessionRecord>, sqlx::Error> {
    let row = sqlx::query(
        "select id, player_id, token_hash, created_at, last_seen_at, expires_at, stay_logged_in
         from sessions where id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| SessionRecord {
        id: r.get(0),
        player_id: r.get(1),
        token_hash: r.get(2),
        created_at: r.get(3),
        last_seen_at: r.get(4),
        expires_at: r.get(5),
        stay_logged_in: r.get(6),
    }))
}

/// Lazy cleanup, same pattern as `expire_old_finished_games`/the
/// move-time-limit retirement — checked whenever something relevant is
/// touched (see the `list_games` call site), not a background scheduler.
/// Prunes a session once it hits *either* limit: past its absolute
/// `expires_at`, or idle (no `last_seen_at` bump) for longer than
/// `SESSION_IDLE_WINDOW_SECS`. The idle clause is what finally bounds
/// `stay_logged_in` rows, which used to have `expires_at is null` and so
/// lived forever. (`expires_at`/`last_seen_at` are unix-second strings, all
/// the same width in this era, so string `<` matches numeric `<`.)
pub async fn delete_expired_sessions(pool: &Pool<Sqlite>) -> Result<(), sqlx::Error> {
    let now = now_iso();
    let idle_cutoff = now
        .parse::<u64>()
        .map(|n| n.saturating_sub(SESSION_IDLE_WINDOW_SECS).to_string())
        .unwrap_or_else(|_| now.clone());
    sqlx::query(
        "delete from sessions
         where (expires_at is not null and expires_at < ?1)
            or last_seen_at < ?2",
    )
    .bind(&now)
    .bind(&idle_cutoff)
    .execute(pool)
    .await?;
    Ok(())
}

/// Deletes the session behind a bearer token — the explicit log-out path.
/// A no-op if the token hash isn't present (already gone / never existed),
/// so the caller never has to distinguish "deleted" from "wasn't there".
pub async fn delete_session_by_token_hash(
    pool: &Pool<Sqlite>,
    token_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from sessions where token_hash = ?1")
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_last_seen(pool: &Pool<Sqlite>, id: &str) -> Result<(), sqlx::Error> {
    let now = now_iso();
    sqlx::query("update sessions set last_seen_at = ?1 where id = ?2")
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ========== Password Reset Functions ==========

#[derive(Debug, Clone)]
pub struct PasswordResetTokenRecord {
    pub id: String,
    pub player_id: String,
    pub token_hash: String,
    pub created_at: String,
    pub expires_at: String,
    pub consumed_at: Option<String>,
}

pub async fn create_password_reset_token(
    pool: &Pool<Sqlite>,
    id: &str,
    player_id: &str,
    token_hash: &str,
    expires_at: &str,
) -> Result<(), sqlx::Error> {
    let now = now_iso();
    sqlx::query(
        "insert into password_reset_tokens (id, player_id, token_hash, created_at, expires_at)
         values (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(id)
    .bind(player_id)
    .bind(token_hash)
    .bind(&now)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_password_reset_token_by_hash(
    pool: &Pool<Sqlite>,
    token_hash: &str,
) -> Result<Option<PasswordResetTokenRecord>, sqlx::Error> {
    let row = sqlx::query(
        "select id, player_id, token_hash, created_at, expires_at, consumed_at
         from password_reset_tokens where token_hash = ?1",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| PasswordResetTokenRecord {
        id: r.get(0),
        player_id: r.get(1),
        token_hash: r.get(2),
        created_at: r.get(3),
        expires_at: r.get(4),
        consumed_at: r.get(5),
    }))
}

pub async fn consume_password_reset_token(
    pool: &Pool<Sqlite>,
    id: &str,
) -> Result<(), sqlx::Error> {
    let now = now_iso();
    sqlx::query("update password_reset_tokens set consumed_at = ?1 where id = ?2")
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Called before issuing a fresh token so an account never has more than
/// one outstanding reset link at a time — otherwise an older, still-valid
/// link sitting in an inbox would stay just as usable as the newest one.
/// Deletes rather than marks-consumed: an unconsumed-but-superseded token
/// isn't "used", it's just moot, and deleting keeps the table from growing
/// unboundedly for accounts that request a reset repeatedly.
pub async fn invalidate_password_reset_tokens_for_player(
    pool: &Pool<Sqlite>,
    player_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from password_reset_tokens where player_id = ?1 and consumed_at is null")
        .bind(player_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ========== Game Invitation Functions ==========

#[derive(Debug, Clone)]
pub struct InvitationRecord {
    pub id: String,
    pub game_id: String,
    /// `None` means an open/stranger invitation, or an email invitation not
    /// yet accepted (see `invited_email`) — visible to any logged-in
    /// player, first to accept claims the seat.
    pub invited_player_id: Option<String>,
    pub inviting_player_id: String,
    pub seat_number: u8,
    pub status: String,
    pub created_at: String,
    pub responded_at: Option<String>,
    /// Set only for a `SeatClaim::Email` invitation — the address the join
    /// link was sent to. Distinguishes it from a plain open/stranger
    /// invitation (both have `invited_player_id: None` until claimed), most
    /// importantly in `get_open_invitations`, which excludes these: an
    /// email invite is only reachable via its mailed link, not general
    /// browsing.
    pub invited_email: Option<String>,
}

fn invitation_from_row(row: sqlx::sqlite::SqliteRow) -> InvitationRecord {
    InvitationRecord {
        id: row.get(0),
        game_id: row.get(1),
        invited_player_id: row.get(2),
        inviting_player_id: row.get(3),
        seat_number: row.get::<i64, _>(4) as u8,
        status: row.get(5),
        created_at: row.get(6),
        responded_at: row.get(7),
        invited_email: row.get(8),
    }
}

const INVITATION_COLUMNS: &str = "id, game_id, invited_player_id, inviting_player_id, seat_number, status, created_at, responded_at, invited_email";

pub async fn create_invitation(
    pool: &Pool<Sqlite>,
    id: &str,
    game_id: &str,
    invited_player_id: Option<&str>,
    inviting_player_id: &str,
    seat_number: u8,
    invited_email: Option<&str>,
) -> Result<InvitationRecord, sqlx::Error> {
    let now = now_iso();
    sqlx::query(
        "insert into game_invitations (id, game_id, invited_player_id, inviting_player_id, seat_number, status, created_at, invited_email)
         values (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
    )
    .bind(id)
    .bind(game_id)
    .bind(invited_player_id)
    .bind(inviting_player_id)
    .bind(seat_number as i64)
    .bind(&now)
    .bind(invited_email)
    .execute(pool)
    .await?;

    Ok(InvitationRecord {
        id: id.to_string(),
        game_id: game_id.to_string(),
        invited_player_id: invited_player_id.map(str::to_string),
        inviting_player_id: inviting_player_id.to_string(),
        seat_number,
        status: "pending".to_string(),
        created_at: now,
        responded_at: None,
        invited_email: invited_email.map(str::to_string),
    })
}

pub async fn get_invitations_for_player(
    pool: &Pool<Sqlite>,
    player_id: &str,
) -> Result<Vec<InvitationRecord>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "select {INVITATION_COLUMNS} from game_invitations where invited_player_id = ?1"
    ))
    .bind(player_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(invitation_from_row).collect())
}

/// Every invitation (any status) ever created for a game — used to compute
/// each unclaimed seat's `api::SeatInvitationStatus` from its most recent
/// row. Unlike `get_invitations_for_player`/`get_open_invitations`, this
/// isn't filtered to `"pending"` — a rejected or cancelled row is exactly
/// as relevant to "what's this seat's history" as a pending one.
pub async fn get_invitations_for_game(
    pool: &Pool<Sqlite>,
    game_id: &str,
) -> Result<Vec<InvitationRecord>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "select {INVITATION_COLUMNS} from game_invitations where game_id = ?1"
    ))
    .bind(game_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(invitation_from_row).collect())
}

/// Pending open/stranger invitations — visible to every logged-in player,
/// not just one specific invitee. Excludes email invitations (also
/// `invited_player_id is null` until claimed): those are only reachable via
/// their mailed link, not general browsing — see `InvitationRecord.
/// invited_email`'s doc comment.
pub async fn get_open_invitations(
    pool: &Pool<Sqlite>,
) -> Result<Vec<InvitationRecord>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "select {INVITATION_COLUMNS} from game_invitations
         where invited_player_id is null and invited_email is null and status = 'pending'"
    ))
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(invitation_from_row).collect())
}

/// Looks up a single invitation by id regardless of status — used by the
/// public preview endpoint an email join-link lands on, before the visitor
/// has necessarily registered or logged in.
pub async fn get_invitation_by_id(
    pool: &Pool<Sqlite>,
    invitation_id: &str,
) -> Result<Option<InvitationRecord>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "select {INVITATION_COLUMNS} from game_invitations where id = ?1"
    ))
    .bind(invitation_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(invitation_from_row))
}

/// The still-pending invitation (if any) already outstanding for this seat —
/// used to stop a seat from being double-invited while one invite is still
/// awaiting a response.
pub async fn get_pending_invitation_for_seat(
    pool: &Pool<Sqlite>,
    game_id: &str,
    seat_number: u8,
) -> Result<Option<InvitationRecord>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "select {INVITATION_COLUMNS} from game_invitations
         where game_id = ?1 and seat_number = ?2 and status = 'pending'"
    ))
    .bind(game_id)
    .bind(seat_number as i64)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(invitation_from_row))
}

pub async fn update_invitation_status(
    pool: &Pool<Sqlite>,
    id: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    let now = now_iso();
    sqlx::query("update game_invitations set status = ?1, responded_at = ?2 where id = ?3")
        .bind(status)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Every seat after a removed one shifts down by one index (see
/// `GameSession::remove_seat`'s doc comment on why seat numbers must stay
/// contiguous) — this keeps every invitation row (`game_invitations.
/// seat_number`, for *any* status, not just live ones, so history stays
/// accurate too) pointing at the same seat it always did, under its new
/// number. Called once per removal, right alongside `save_game`, inside
/// the same handler that called `GameSession::remove_seat`.
pub async fn shift_invitation_seat_numbers_down(
    pool: &Pool<Sqlite>,
    game_id: &str,
    removed_seat_number: u8,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update game_invitations set seat_number = seat_number - 1
         where game_id = ?1 and seat_number > ?2",
    )
    .bind(game_id)
    .bind(removed_seat_number as i64)
    .execute(pool)
    .await?;
    Ok(())
}

pub enum ClaimInvitationError {
    NotFound,
    /// Already responded to (accepted/rejected/cancelled) or, for an open
    /// invitation, already claimed by someone else in a race.
    NoLongerAvailable,
    /// A named invitation belongs to a specific player; anyone else trying
    /// to accept it hits this instead of silently taking their seat.
    NotYourInvitation,
}

/// Atomically accepts an invitation and reports who now owns it. Race-safe
/// for open invitations: the `where status = 'pending'` guard means if two
/// players accept the same open seat at once, only the first `UPDATE` finds
/// a matching row — the second sees 0 rows affected and gets
/// `NoLongerAvailable` instead of silently overwriting the first claim.
pub async fn claim_invitation(
    pool: &Pool<Sqlite>,
    invitation_id: &str,
    claimant_player_id: &str,
) -> Result<InvitationRecord, ClaimInvitationError> {
    let existing = sqlx::query(&format!(
        "select {INVITATION_COLUMNS} from game_invitations where id = ?1"
    ))
    .bind(invitation_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| ClaimInvitationError::NotFound)?
    .map(invitation_from_row)
    .ok_or(ClaimInvitationError::NotFound)?;

    // Only check "is this actually your invitation" while it's still
    // pending. Once accepted, an open invitation's `invited_player_id` has
    // been backfilled to whoever claimed it — checking that here would
    // misreport a late second acceptor as "not your invitation" instead of
    // "no longer available".
    if existing.status == "pending"
        && let Some(named) = &existing.invited_player_id
        && named != claimant_player_id
    {
        return Err(ClaimInvitationError::NotYourInvitation);
    }

    let now = now_iso();
    let result = sqlx::query(
        "update game_invitations
         set status = 'accepted', responded_at = ?1, invited_player_id = coalesce(invited_player_id, ?2)
         where id = ?3 and status = 'pending'",
    )
    .bind(&now)
    .bind(claimant_player_id)
    .bind(invitation_id)
    .execute(pool)
    .await
    .map_err(|_| ClaimInvitationError::NoLongerAvailable)?;

    if result.rows_affected() == 0 {
        return Err(ClaimInvitationError::NoLongerAvailable);
    }

    Ok(InvitationRecord {
        invited_player_id: Some(claimant_player_id.to_string()),
        status: "accepted".to_string(),
        responded_at: Some(now),
        ..existing
    })
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs();
    seconds.to_string()
}

fn status_name(status: &GameStatus) -> &'static str {
    match status {
        GameStatus::Waiting => "waiting",
        GameStatus::Active => "active",
        GameStatus::Finished => "finished",
    }
}

fn kind_name(kind: &SeatKind) -> &'static str {
    match kind {
        SeatKind::Human => "human",
        SeatKind::Engine => "engine",
    }
}

#[allow(dead_code)]
fn _keep_types(_: &ParticipantState, _: &MoveRecord) {}
