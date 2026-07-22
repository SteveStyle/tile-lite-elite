//! Per-participant outcome/bingo bookkeeping and the ELO-style rating step,
//! both triggered from `persistence::save_game` the moment a game's status
//! transitions into `Finished`. Kept in its own module rather than growing
//! `persistence.rs` further, since this is genuinely separate logic from
//! row (de)serialization — see `crates/server-game/migrations/0002_player_ratings_and_stats.sql`
//! for the schema this reads and writes.

use sqlx::{Pool, Row, Sqlite, Transaction};

use crate::game_state::GameSession;

/// The move type of the very last move recorded, if any — for a `Finished`
/// game this is always the terminal action that ended it: `"place"`/
/// `"pass"`/`"exchange"` for a normal ending (someone went out, or the
/// scoreless-turn limit), `"resign"`/`"force_resign"`/`"timeout"` for the
/// three ways a game can be cut short, or `"admin_force_end"` for the admin
/// cleanup tool. `None` only for a game with no moves at all (e.g.
/// admin-force-ending a `Waiting` game that never started).
fn terminal_move_type(session: &GameSession) -> Option<&str> {
    session.moves.last().map(|mv| mv.move_type.as_str())
}

/// A game's outcome for one seat, once it's `Finished`. Mutually exclusive —
/// a finished game assigns exactly one of these to every seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Win,
    Loss,
    Tie,
    Timeout,
    Resigned,
}

impl Outcome {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Outcome::Win => "win",
            Outcome::Loss => "loss",
            Outcome::Tie => "tie",
            Outcome::Timeout => "timeout",
            Outcome::Resigned => "resigned",
        }
    }
}

pub struct ParticipantOutcome {
    pub seat_number: u8,
    /// `None` while the game isn't `Finished` yet.
    pub outcome: Option<Outcome>,
    pub bingo_count: i64,
}

/// Pure and safe to call unconditionally on every save, `Finished` or not —
/// once a game is `Finished` its participants/moves never change again, so
/// recomputing this on a later re-save (e.g. `remove_for_player`) always
/// reproduces the exact same values.
pub fn compute_participant_outcomes(session: &GameSession) -> Vec<ParticipantOutcome> {
    let mut bingo_counts = std::collections::HashMap::new();
    for mv in &session.moves {
        if mv.move_type == "place" && mv.positions.len() == 7 {
            *bingo_counts.entry(mv.seat_number).or_insert(0i64) += 1;
        }
    }
    let bingo_count_for = |seat: u8| *bingo_counts.get(&seat).unwrap_or(&0);

    if session.status != api::GameStatus::Finished {
        return session
            .participants
            .iter()
            .map(|p| ParticipantOutcome {
                seat_number: p.seat_number,
                outcome: None,
                bingo_count: bingo_count_for(p.seat_number),
            })
            .collect();
    }

    // At most one seat can be the resigned/timed-out forfeiter today — any
    // resignation or timeout ends the whole game immediately (see
    // `GameSession::finish_via_resignation`/`apply_move_timeout`) — so
    // there's never more than one to find.
    let forfeiter = session.participants.iter().find(|p| p.resigned);
    let forfeiter_seat = forfeiter.map(|p| p.seat_number);
    let forfeiter_outcome = forfeiter.map(|p| {
        let is_timeout = session
            .moves
            .iter()
            .rev()
            .find(|mv| mv.seat_number == p.seat_number && (mv.move_type == "resign" || mv.move_type == "force_resign" || mv.move_type == "timeout"))
            .is_some_and(|mv| mv.move_type == "timeout");
        if is_timeout { Outcome::Timeout } else { Outcome::Resigned }
    });

    let max_score = session
        .participants
        .iter()
        .filter(|p| Some(p.seat_number) != forfeiter_seat)
        .map(|p| p.score)
        .max();

    session
        .participants
        .iter()
        .map(|p| {
            let outcome = if Some(p.seat_number) == forfeiter_seat {
                forfeiter_outcome.unwrap_or(Outcome::Resigned)
            } else {
                match max_score {
                    Some(max) if p.score == max => {
                        let sharers = session
                            .participants
                            .iter()
                            .filter(|q| Some(q.seat_number) != forfeiter_seat && q.score == max)
                            .count();
                        if sharers > 1 { Outcome::Tie } else { Outcome::Win }
                    }
                    _ => Outcome::Loss,
                }
            };
            ParticipantOutcome {
                seat_number: p.seat_number,
                outcome: Some(outcome),
                bingo_count: bingo_count_for(p.seat_number),
            }
        })
        .collect()
}

const RATING_K: f64 = 32.0;
const DEFAULT_RATING: f64 = 1500.0;
const RATING_FLOOR: f64 = 100.0;

/// Runs the ELO-style rating update for a game that just transitioned to
/// `Finished`, inside `tx` (part of the same transaction as the rest of the
/// save, so a crash mid-way can't leave the game Finished with no rating
/// applied and no way to retry). Deliberately conservative about when it
/// writes anything:
///
/// - Skips entirely (no writes, no `games_rated` change) when the game's
///   terminal move was a timeout or a creator-forced resignation — an
///   administrative/non-organic ending shouldn't move anyone's rating,
///   unlike a *voluntary* resignation, which does (via the forfeit-ranking
///   below).
/// - Skips a seat with neither a `player_id` nor an `engine_id` (an
///   unclaimed seat — reachable via `admin_force_end_game` on a `Waiting`
///   game).
/// - Skips entirely if fewer than 2 such seats remain.
///
/// Seats are ranked by `(!forfeited, score)`, not raw `score` — so a player
/// who resigns while ahead on points still ranks last for rating purposes,
/// matching who the game actually declared the winner to be.
///
/// A subject occupying more than one seat in the same game (e.g. two engine
/// seats both bound to the same bot in a "Bot Showdown") is handled by
/// summing that subject's per-seat deltas into one net update, *not* by
/// skipping the game — two seats for the identical subject always net to
/// an exact zero contribution from their own mutual pairing (for any pair
/// `(i, j)`, `delta_j = K·((1−S_i) − (1−E_i)) = −delta_i`, regardless of
/// score or forfeit status), so this naturally handles pure self-play
/// (nets to zero) and a mixed game (e.g. a human seated against two bot
/// seats, whose combined effect lands as one net update to the bot's row)
/// without any special-casing.
pub async fn settle_ratings(
    tx: &mut Transaction<'_, Sqlite>,
    session: &GameSession,
    ended_at: &str,
) -> Result<(), sqlx::Error> {
    // Timeout, creator-forced resignation, and an admin force-end are all
    // administrative/non-organic endings that shouldn't move anyone's
    // rating — unlike a normal finish or a *voluntary* resignation, which
    // do. `terminal_move_type` also covers a `Waiting` game admin-force-
    // ended with no moves at all (`None`, correctly not administrative in
    // the sense of "skip because of this check" — it's excluded below
    // anyway by having fewer than 2 resolvable seats in the typical case,
    // but if it somehow has 2+ claimed seats with zero moves played, that
    // still isn't an organic ending and also shouldn't rate — so `None`
    // is folded into the skip here too).
    let administrative_ending = !matches!(terminal_move_type(session), Some("place" | "pass" | "exchange" | "resign"));
    if administrative_ending {
        return Ok(());
    }

    let forfeiter_seat = session.participants.iter().find(|p| p.resigned).map(|p| p.seat_number);

    // (subject_kind, subject_id, forfeited, score) per resolvable seat.
    let mut seats: Vec<(&'static str, String, bool, i32)> = Vec::new();
    for p in &session.participants {
        let subject = match (&p.player_id, &p.engine_id) {
            (Some(id), None) => ("player", id.clone()),
            (None, Some(id)) => ("engine", id.clone()),
            _ => continue,
        };
        seats.push((subject.0, subject.1, Some(p.seat_number) == forfeiter_seat, p.score));
    }
    if seats.len() < 2 {
        return Ok(());
    }

    let mut ratings_before = Vec::with_capacity(seats.len());
    for (kind, id, ..) in &seats {
        let row = sqlx::query("select rating from player_ratings where subject_kind = ?1 and subject_id = ?2")
            .bind(*kind)
            .bind(id.as_str())
            .fetch_optional(&mut **tx)
            .await?;
        ratings_before.push(row.map(|r| r.get::<f64, _>(0)).unwrap_or(DEFAULT_RATING));
    }

    let n = seats.len();
    let mut deltas = vec![0.0f64; n];
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let rank_i = (!seats[i].2, seats[i].3);
            let rank_j = (!seats[j].2, seats[j].3);
            let s_i = match rank_i.cmp(&rank_j) {
                std::cmp::Ordering::Greater => 1.0,
                std::cmp::Ordering::Less => 0.0,
                std::cmp::Ordering::Equal => 0.5,
            };
            let e_i = 1.0 / (1.0 + 10f64.powf((ratings_before[j] - ratings_before[i]) / 400.0));
            deltas[i] += (RATING_K / (n as f64 - 1.0)) * (s_i - e_i);
        }
    }

    // Group by subject — a subject occupying multiple seats gets its
    // deltas (and its pre-game rating, identical across those seats since
    // it's the same DB row) summed into one net update.
    let mut by_subject: std::collections::BTreeMap<(&'static str, String), (f64, f64)> = std::collections::BTreeMap::new();
    for (i, (kind, id, ..)) in seats.iter().enumerate() {
        let entry = by_subject.entry((*kind, id.clone())).or_insert((ratings_before[i], 0.0));
        entry.1 += deltas[i];
    }

    for ((kind, id), (rating_before, total_delta)) in by_subject {
        let rating_after = (rating_before + total_delta).max(RATING_FLOOR);
        sqlx::query(
            "insert into player_ratings (subject_kind, subject_id, rating, games_rated, updated_at)
             values (?1, ?2, ?3, 1, ?4)
             on conflict(subject_kind, subject_id) do update set
                rating = excluded.rating,
                games_rated = player_ratings.games_rated + 1,
                updated_at = excluded.updated_at",
        )
        .bind(kind)
        .bind(&id)
        .bind(rating_after)
        .bind(ended_at)
        .execute(&mut **tx)
        .await?;

        sqlx::query(
            "insert into rating_history (id, subject_kind, subject_id, game_id, rating_before, rating_after, created_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(kind)
        .bind(&id)
        .bind(&session.id)
        .bind(rating_before)
        .bind(rating_after)
        .bind(ended_at)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

/// A subject's current rating plus aggregate outcome/bingo counters, never
/// erroring for a subject that's never been rated — it just comes back as
/// rating 1500 with every counter at 0. `subject_kind` is `"player"` or
/// `"engine"`, matching the DB's own string representation.
pub async fn get_subject_stats(
    pool: &Pool<Sqlite>,
    subject_kind: &str,
    subject_id: &str,
) -> Result<api::PlayerStatsDto, sqlx::Error> {
    // The column to filter on differs by subject kind and can't be bound
    // as a parameter, so this is two literal query strings selected by an
    // `if`, not string-built SQL — same approach as `kind_name`/
    // `status_name`'s string-based DB representations elsewhere in this
    // crate.
    let outcome_rows = if subject_kind == "player" {
        sqlx::query(
            "select outcome, count(*) from game_participants
             where player_id = ?1 and outcome is not null group by outcome",
        )
    } else {
        sqlx::query(
            "select outcome, count(*) from game_participants
             where engine_id = ?1 and outcome is not null group by outcome",
        )
    }
    .bind(subject_id)
    .fetch_all(pool)
    .await?;

    let mut wins = 0i64;
    let mut losses = 0i64;
    let mut ties = 0i64;
    let mut timeouts = 0i64;
    let mut resignations = 0i64;
    for row in outcome_rows {
        let outcome: String = row.get(0);
        let count: i64 = row.get(1);
        match outcome.as_str() {
            "win" => wins = count,
            "loss" => losses = count,
            "tie" => ties = count,
            "timeout" => timeouts = count,
            "resigned" => resignations = count,
            _ => {}
        }
    }

    let bingo_count: i64 = if subject_kind == "player" {
        sqlx::query("select coalesce(sum(bingo_count), 0) from game_participants where player_id = ?1")
    } else {
        sqlx::query("select coalesce(sum(bingo_count), 0) from game_participants where engine_id = ?1")
    }
    .bind(subject_id)
    .fetch_one(pool)
    .await?
    .get(0);

    let rating_row = sqlx::query("select rating, games_rated from player_ratings where subject_kind = ?1 and subject_id = ?2")
        .bind(subject_kind)
        .bind(subject_id)
        .fetch_optional(pool)
        .await?;
    let (rating, games_rated) = rating_row
        .map(|row| (row.get::<f64, _>(0), row.get::<i64, _>(1)))
        .unwrap_or((DEFAULT_RATING, 0));

    Ok(api::PlayerStatsDto {
        subject_kind: if subject_kind == "player" { api::RatingSubjectKind::Player } else { api::RatingSubjectKind::Engine },
        subject_id: subject_id.to_string(),
        rating,
        games_rated,
        wins,
        losses,
        ties,
        timeouts,
        resignations,
        bingo_count,
    })
}

/// A subject's rating after every rated game it's played, oldest first —
/// the series a rating-over-time graph plots directly.
pub async fn get_rating_history(
    pool: &Pool<Sqlite>,
    subject_kind: &str,
    subject_id: &str,
) -> Result<Vec<api::RatingPointDto>, sqlx::Error> {
    let rows = sqlx::query(
        "select game_id, rating_after, created_at from rating_history
         where subject_kind = ?1 and subject_id = ?2 order by created_at",
    )
    .bind(subject_kind)
    .bind(subject_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| api::RatingPointDto {
            game_id: row.get(0),
            rating_after: row.get(1),
            created_at: row.get(2),
        })
        .collect())
}

/// Fills in `rating_before`/`rating_after` on every participant of a
/// `Finished` game's DTO whose subject was actually rated by it — a no-op
/// (no DB read at all) for a game still in progress, and leaves
/// `None`/`None` for any seat whose ending skipped rating (see
/// `settle_ratings`). Called right after `save_game` at the handful of
/// call sites that can finish a game, so both the acting player's own HTTP
/// response and the `GameFinished` broadcast carry the same rating info —
/// letting the client show "your rating just moved" at the moment a game
/// ends.
pub async fn attach_rating_deltas(pool: &Pool<Sqlite>, dto: &mut api::GameStateDto) -> Result<(), sqlx::Error> {
    if dto.status != api::GameStatus::Finished {
        return Ok(());
    }
    let rows = sqlx::query(
        "select subject_kind, subject_id, rating_before, rating_after from rating_history where game_id = ?1",
    )
    .bind(&dto.id)
    .fetch_all(pool)
    .await?;

    for row in rows {
        let kind: String = row.get(0);
        let id: String = row.get(1);
        let before: f64 = row.get(2);
        let after: f64 = row.get(3);
        for participant in &mut dto.participants {
            let is_match = match kind.as_str() {
                "player" => participant.player_id.as_deref() == Some(id.as_str()),
                "engine" => participant.engine_id.as_deref() == Some(id.as_str()),
                _ => false,
            };
            if is_match {
                participant.rating_before = Some(before);
                participant.rating_after = Some(after);
            }
        }
    }
    Ok(())
}
