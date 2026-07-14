use api::{BoardCellDto, GameStatus, SeatKind};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite, sqlite::SqliteConnectOptions, sqlite::SqlitePoolOptions};
use std::str::FromStr;

use crate::game_state::{GameSession, MoveRecord, ParticipantState, board_from_dto};
use rules_shared::{Alphabet, GameState, Premium, SOWPODS, Score, Tile, VariantRules};

/// Mirrors `rules_shared::VariantRules` field-for-field, but as its own type
/// rather than deriving `Serialize`/`Deserialize` directly on the internal
/// one — that type's shape is expected to keep evolving (board size,
/// alphabet width) as more editions/languages are added, and pinning the DB
/// schema straight to it would turn each of those changes into a data
/// migration. This struct is the DB's problem to keep stable; `VariantRules`
/// is free to change shape as long as the conversions below keep up.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedVariantRules {
    name: String,
    language: String,
    letter_values: [u8; 26],
    tile_distribution: [u8; 26],
    blank_tiles: u8,
    rack_size: u8,
    width: u8,
    height: u8,
    bingo_bonus: Score,
    premiums: Vec<Premium>,
}

impl From<&VariantRules> for PersistedVariantRules {
    fn from(rules: &VariantRules) -> Self {
        // The persisted format stays 26-wide for now (see
        // rules_shared::MAX_ALPHABET_SIZE's doc comment) — every edition in
        // production today is Alphabet::latin26(), so only the first 26 of
        // the internal (wider) arrays ever hold anything real.
        let mut letter_values = [0u8; 26];
        letter_values.copy_from_slice(&rules.letter_values[..26]);
        let mut tile_distribution = [0u8; 26];
        tile_distribution.copy_from_slice(&rules.tile_distribution[..26]);
        Self {
            name: rules.name.clone(),
            language: rules.language.clone(),
            letter_values,
            tile_distribution,
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
        let mut letter_values = [0u8; rules_shared::MAX_ALPHABET_SIZE];
        letter_values[..26].copy_from_slice(&persisted.letter_values);
        let mut tile_distribution = [0u8; rules_shared::MAX_ALPHABET_SIZE];
        tile_distribution[..26].copy_from_slice(&persisted.tile_distribution);
        Ok(VariantRules {
            name: persisted.name,
            // Every persisted game to date is the standard Latin alphabet —
            // this will need to actually vary once a non-Latin edition can
            // be persisted (Phase 5), presumably keyed off `language`.
            alphabet: Alphabet::latin26(),
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
    random_seed: u64,
    board: Vec<BoardCellDto>,
    bag: Vec<Tile>,
    participants: Vec<ParticipantState>,
    moves: Vec<MoveRecord>,
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
    let options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

pub async fn migrate(pool: &Pool<Sqlite>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "create table if not exists schema_migrations (
            version integer primary key,
            applied_at text not null
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "create table if not exists players (
            id text primary key,
            display_name text not null unique,
            email text not null,
            email_verification_code_hash text,
            email_verification_sent_at text,
            email_verified_at text,
            password_hash text not null,
            created_at text not null,
            updated_at text not null,
            last_seen_at text,
            preferences_json text
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "create table if not exists engine_profiles (
            id text primary key,
            name text not null,
            version text not null,
            author text,
            description text,
            capabilities_json text not null,
            created_at text not null,
            updated_at text not null
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "create table if not exists games (
            id text primary key,
            created_at text not null,
            started_at text,
            ended_at text,
            status text not null,
            variant text not null,
            language text not null,
            board_layout text not null,
            turn_number integer not null,
            current_seat integer not null,
            winner_seat integer,
            random_seed integer,
            notes text,
            snapshot_json text not null
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "create table if not exists game_participants (
            id text primary key,
            game_id text not null,
            seat_number integer not null,
            kind text not null,
            display_name text not null,
            player_id text,
            engine_id text,
            score integer not null default 0,
            joined_at text not null,
            left_at text,
            unique(game_id, seat_number)
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "create table if not exists game_moves (
            id text primary key,
            game_id text not null,
            move_number integer not null,
            seat_number integer not null,
            move_type text not null,
            tiles_json text,
            payload_json text not null,
            score_delta integer not null default 0,
            created_at text not null,
            is_validated integer not null default 1,
            unique(game_id, move_number)
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "create table if not exists sessions (
            id text primary key,
            player_id text not null,
            game_id text,
            token_hash text,
            created_at text not null,
            last_seen_at text not null,
            expires_at text
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "create table if not exists game_invitations (
            id text primary key,
            game_id text not null,
            invited_player_id text,
            inviting_player_id text not null,
            seat_number integer not null,
            status text not null,
            created_at text not null,
            responded_at text
        )",
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn save_game(pool: &Pool<Sqlite>, session: &GameSession) -> Result<(), sqlx::Error> {
    let now = now_iso();
    let snapshot_json = serde_json::to_string(&PersistedGame {
        id: session.id.clone(),
        status: session.status.clone(),
        variant: session.variant.clone(),
        language: session.language.clone(),
        board_layout: session.board_layout.clone(),
        rules: PersistedVariantRules::from(&session.rules),
        turn_number: session.turn_number,
        current_seat: session.current_seat,
        winner_seat: session.winner_seat,
        final_bonus_seat: session.final_bonus_seat,
        final_bonus_points: session.final_bonus_points,
        random_seed: session.random_seed,
        board: session.to_dto().board,
        bag: session.bag.clone(),
        participants: session.participants.clone(),
        moves: session.moves.clone(),
        consecutive_scoreless_turns: session.consecutive_scoreless_turns,
        move_time_limit_seconds: session.move_time_limit_seconds,
        turn_started_at: session.turn_started_at.clone(),
    })
    .expect("game session should serialize");

    sqlx::query(
        "insert into games (
            id, created_at, started_at, ended_at, status, variant, language, board_layout,
            turn_number, current_seat, winner_seat, random_seed, notes, snapshot_json
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
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
            notes = excluded.notes,
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
    .bind(Option::<String>::None)
    .bind(snapshot_json)
    .execute(pool)
    .await?;

    sqlx::query("delete from game_participants where game_id = ?1")
        .bind(&session.id)
        .execute(pool)
        .await?;

    for participant in &session.participants {
        sqlx::query(
            "insert into game_participants (
                id, game_id, seat_number, kind, display_name, player_id, engine_id, score, joined_at, left_at
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        .bind(Option::<String>::None)
        .execute(pool)
        .await?;
    }

    sqlx::query("delete from game_moves where game_id = ?1")
        .bind(&session.id)
        .execute(pool)
        .await?;

    for record in &session.moves {
        sqlx::query(
            "insert into game_moves (
                id, game_id, move_number, seat_number, move_type, tiles_json, payload_json, score_delta, created_at, is_validated
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1)",
        )
        .bind(format!("{}-move-{}", session.id, record.move_number))
        .bind(&session.id)
        .bind(record.move_number)
        .bind(record.seat_number as i64)
        .bind(&record.move_type)
        .bind(Option::<String>::None)
        .bind(serde_json::to_string(record).expect("move record should serialize"))
        .bind(record.score_delta)
        .bind(&now)
        .execute(pool)
        .await?;
    }

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
        let board = board_from_dto(&persisted.board).expect("persisted board should be valid");
        let state = GameState::from_board(board, &rules, &*SOWPODS);
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
            random_seed: persisted.random_seed,
            rules,
            state,
            bag: persisted.bag,
            participants: persisted.participants,
            moves: persisted.moves,
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
    let result = sqlx::query("update players set password_hash = ?1, updated_at = ?2 where id = ?3")
        .bind(password_hash)
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
/// invitations). Doesn't touch player accounts. Caller is responsible for
/// also dropping it from the in-memory `AppState.games` map — this only
/// handles the database side.
pub async fn delete_game(pool: &Pool<Sqlite>, game_id: &str) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("delete from game_moves where game_id = ?1")
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
}

pub async fn create_session(
    pool: &Pool<Sqlite>,
    id: &str,
    player_id: &str,
    token_hash: &str,
    expires_at: Option<&str>,
) -> Result<SessionRecord, sqlx::Error> {
    let now = now_iso();
    sqlx::query(
        "insert into sessions (id, player_id, token_hash, created_at, last_seen_at, expires_at)
         values (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(id)
    .bind(player_id)
    .bind(token_hash)
    .bind(&now)
    .bind(&now)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(SessionRecord {
        id: id.to_string(),
        player_id: player_id.to_string(),
        token_hash: Some(token_hash.to_string()),
        created_at: now.clone(),
        last_seen_at: now,
        expires_at: expires_at.map(|s| s.to_string()),
    })
}

pub async fn get_session_by_token_hash(
    pool: &Pool<Sqlite>,
    token_hash: &str,
) -> Result<Option<SessionRecord>, sqlx::Error> {
    let row = sqlx::query(
        "select id, player_id, token_hash, created_at, last_seen_at, expires_at
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
    }))
}

pub async fn get_session_by_id(
    pool: &Pool<Sqlite>,
    id: &str,
) -> Result<Option<SessionRecord>, sqlx::Error> {
    let row = sqlx::query(
        "select id, player_id, token_hash, created_at, last_seen_at, expires_at
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
    }))
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

// ========== Game Invitation Functions ==========

#[derive(Debug, Clone)]
pub struct InvitationRecord {
    pub id: String,
    pub game_id: String,
    /// `None` means an open/stranger invitation — visible to any logged-in
    /// player, first to accept claims the seat.
    pub invited_player_id: Option<String>,
    pub inviting_player_id: String,
    pub seat_number: u8,
    pub status: String,
    pub created_at: String,
    pub responded_at: Option<String>,
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
    }
}

const INVITATION_COLUMNS: &str =
    "id, game_id, invited_player_id, inviting_player_id, seat_number, status, created_at, responded_at";

pub async fn create_invitation(
    pool: &Pool<Sqlite>,
    id: &str,
    game_id: &str,
    invited_player_id: Option<&str>,
    inviting_player_id: &str,
    seat_number: u8,
) -> Result<InvitationRecord, sqlx::Error> {
    let now = now_iso();
    sqlx::query(
        "insert into game_invitations (id, game_id, invited_player_id, inviting_player_id, seat_number, status, created_at)
         values (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
    )
    .bind(id)
    .bind(game_id)
    .bind(invited_player_id)
    .bind(inviting_player_id)
    .bind(seat_number as i64)
    .bind(&now)
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

/// Pending open/stranger invitations — visible to every logged-in player,
/// not just one specific invitee.
pub async fn get_open_invitations(pool: &Pool<Sqlite>) -> Result<Vec<InvitationRecord>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "select {INVITATION_COLUMNS} from game_invitations
         where invited_player_id is null and status = 'pending'"
    ))
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(invitation_from_row).collect())
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
    if existing.status == "pending" {
        if let Some(named) = &existing.invited_player_id {
            if named != claimant_player_id {
                return Err(ClaimInvitationError::NotYourInvitation);
            }
        }
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
