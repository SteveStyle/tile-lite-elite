use api::{BoardCellDto, GameStatus, SeatKind};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite, sqlite::SqlitePoolOptions};

use crate::game_state::{GameSession, MoveRecord, ParticipantState, board_from_dto};
use rules_shared::{GameState, SOWPODS, Tile, VariantRules};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedGame {
    id: String,
    status: GameStatus,
    variant: String,
    language: String,
    board_layout: String,
    turn_number: i64,
    current_seat: u8,
    winner_seat: Option<u8>,
    random_seed: u64,
    board: Vec<BoardCellDto>,
    bag: Vec<Tile>,
    participants: Vec<ParticipantState>,
    moves: Vec<MoveRecord>,
}

pub async fn connect(database_url: &str) -> Result<Pool<Sqlite>, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
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
            display_name text not null,
            email text not null,
            email_verification_code_hash text,
            email_verification_sent_at text,
            email_verified_at text,
            recovery_secret_hash text not null,
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
            invited_player_id text not null,
            inviting_player_id text not null,
            seat_number integer not null,
            status text not null,
            created_at text not null,
            responded_at text,
            unique(game_id, invited_player_id, seat_number)
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
        turn_number: session.turn_number,
        current_seat: session.current_seat,
        winner_seat: session.winner_seat,
        random_seed: session.random_seed,
        board: session.to_dto().board,
        bag: session.bag.clone(),
        participants: session.participants.clone(),
        moves: session.moves.clone(),
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
        let rules = VariantRules::official();
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
            random_seed: persisted.random_seed,
            rules,
            state,
            bag: persisted.bag,
            participants: persisted.participants,
            moves: persisted.moves,
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
    pub recovery_secret_hash: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_seen_at: Option<String>,
}

pub async fn create_player(
    pool: &Pool<Sqlite>,
    id: &str,
    display_name: &str,
    email: &str,
    recovery_secret_hash: &str,
) -> Result<PlayerRecord, sqlx::Error> {
    let now = now_iso();
    sqlx::query(
        "insert into players (id, display_name, email, recovery_secret_hash, created_at, updated_at)
         values (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(id)
    .bind(display_name)
    .bind(email)
    .bind(recovery_secret_hash)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(PlayerRecord {
        id: id.to_string(),
        display_name: display_name.to_string(),
        email: email.to_string(),
        recovery_secret_hash: recovery_secret_hash.to_string(),
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
        "select id, display_name, email, recovery_secret_hash, created_at, updated_at, last_seen_at
         from players where display_name = ?1",
    )
    .bind(display_name)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| PlayerRecord {
        id: r.get(0),
        display_name: r.get(1),
        email: r.get(2),
        recovery_secret_hash: r.get(3),
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
        "select id, display_name, email, recovery_secret_hash, created_at, updated_at, last_seen_at
         from players where id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| PlayerRecord {
        id: r.get(0),
        display_name: r.get(1),
        email: r.get(2),
        recovery_secret_hash: r.get(3),
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
    pub invited_player_id: String,
    pub inviting_player_id: String,
    pub seat_number: u8,
    pub status: String,
    pub created_at: String,
    pub responded_at: Option<String>,
}

pub async fn create_invitation(
    pool: &Pool<Sqlite>,
    id: &str,
    game_id: &str,
    invited_player_id: &str,
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
        invited_player_id: invited_player_id.to_string(),
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
    let rows = sqlx::query(
        "select id, game_id, invited_player_id, inviting_player_id, seat_number, status, created_at, responded_at
         from game_invitations where invited_player_id = ?1",
    )
    .bind(player_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| InvitationRecord {
            id: r.get(0),
            game_id: r.get(1),
            invited_player_id: r.get(2),
            inviting_player_id: r.get(3),
            seat_number: r.get::<i64, _>(4) as u8,
            status: r.get(5),
            created_at: r.get(6),
            responded_at: r.get(7),
        })
        .collect())
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
