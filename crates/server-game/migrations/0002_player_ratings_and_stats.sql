-- Per-participant outcome and bingo count, set once a game finishes (see
-- persistence::compute_participant_outcomes). Null until then.
alter table game_participants add column outcome text;
alter table game_participants add column bingo_count integer not null default 0;

-- Single-fire gate for the rating step (persistence::settle_ratings) — null
-- until rating has been applied for this game. Deliberately distinct from
-- games.status: a legacy game that was already 'finished' before this
-- migration shipped must never be rated retroactively (no backfill), and
-- this column staying null forever for those games is what enforces that.
alter table games add column stats_settled_at text;

-- One row per rated subject (a player or an engine — see subject_kind).
-- Rating starts at 1500 the first time a subject is ever rated; there's no
-- row here at all for a subject that's never finished a rated game.
create table player_ratings (
    subject_kind text not null,
    subject_id text not null,
    rating real not null default 1500,
    games_rated integer not null default 0,
    updated_at text not null,
    primary key (subject_kind, subject_id)
);

-- One row per (subject, game) that actually moved that subject's rating —
-- powers the rating-over-time graph. A game that was skipped for rating
-- (timeout/force-resign endings, fewer than 2 resolvable seats) produces no
-- rows here at all.
create table rating_history (
    id text primary key,
    subject_kind text not null,
    subject_id text not null,
    game_id text not null,
    rating_before real not null,
    rating_after real not null,
    created_at text not null
);

create index idx_rating_history_subject_created_at
    on rating_history(subject_kind, subject_id, created_at);
