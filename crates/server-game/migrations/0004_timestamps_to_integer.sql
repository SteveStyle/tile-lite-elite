-- Convert every timestamp column from TEXT (unix seconds stored as text) to
-- INTEGER (unix seconds as a real i64), matching the Rust/API switch to `i64`.
--
-- SQLite can't ALTER a column's type, and a TEXT-affinity column coerces any
-- integer written to it back into text — so each table with a timestamp column
-- is rebuilt (new table with INTEGER columns -> copy, casting the old text
-- values -> drop -> rename). The copy is a no-op on the wiped databases this
-- ships against, but is written the safe, data-preserving way regardless.
--
-- Rebuilding a table drops its indexes, so every index on a rebuilt table is
-- recreated at the end (plus a new idx_sessions_last_seen_at, since the session
-- idle-cleanup now filters on that column). There are no foreign-key
-- constraints in this schema, so the drop/rename dance needs no fk handling.

-- players
create table players_new (
    id text primary key,
    display_name text not null unique,
    email text not null,
    email_verification_code_hash text,
    email_verification_sent_at integer,
    email_verified_at integer,
    password_hash text not null,
    created_at integer not null,
    updated_at integer not null,
    last_seen_at integer,
    preferences_json text
);
insert into players_new
    select id, display_name, email, email_verification_code_hash,
        cast(email_verification_sent_at as integer), cast(email_verified_at as integer),
        password_hash, cast(created_at as integer), cast(updated_at as integer),
        cast(last_seen_at as integer), preferences_json
    from players;
drop table players;
alter table players_new rename to players;

-- engine_profiles
create table engine_profiles_new (
    id text primary key,
    name text not null,
    version text not null,
    author text,
    description text,
    capabilities_json text not null,
    created_at integer not null,
    updated_at integer not null
);
insert into engine_profiles_new
    select id, name, version, author, description, capabilities_json,
        cast(created_at as integer), cast(updated_at as integer)
    from engine_profiles;
drop table engine_profiles;
alter table engine_profiles_new rename to engine_profiles;

-- games
create table games_new (
    id text primary key,
    created_at integer not null,
    started_at integer,
    ended_at integer,
    status text not null,
    variant text not null,
    language text not null,
    board_layout text not null,
    turn_number integer not null,
    current_seat integer not null,
    winner_seat integer,
    random_seed integer,
    snapshot_json text not null,
    stats_settled_at integer
);
insert into games_new
    select id, cast(created_at as integer), cast(started_at as integer),
        cast(ended_at as integer), status, variant, language, board_layout,
        turn_number, current_seat, winner_seat, random_seed, snapshot_json,
        cast(stats_settled_at as integer)
    from games;
drop table games;
alter table games_new rename to games;

-- game_participants
create table game_participants_new (
    id text primary key,
    game_id text not null,
    seat_number integer not null,
    kind text not null,
    display_name text not null,
    player_id text,
    engine_id text,
    score integer not null default 0,
    joined_at integer not null,
    outcome text,
    bingo_count integer not null default 0,
    unique(game_id, seat_number)
);
insert into game_participants_new
    select id, game_id, seat_number, kind, display_name, player_id, engine_id,
        score, cast(joined_at as integer), outcome, bingo_count
    from game_participants;
drop table game_participants;
alter table game_participants_new rename to game_participants;

-- game_moves
create table game_moves_new (
    id text primary key,
    game_id text not null,
    move_number integer not null,
    seat_number integer not null,
    move_type text not null,
    payload_json text not null,
    score_delta integer not null default 0,
    created_at integer not null,
    unique(game_id, move_number)
);
insert into game_moves_new
    select id, game_id, move_number, seat_number, move_type, payload_json,
        score_delta, cast(created_at as integer)
    from game_moves;
drop table game_moves;
alter table game_moves_new rename to game_moves;

-- game_messages
create table game_messages_new (
    id text primary key,
    game_id text not null,
    player_id text not null,
    display_name text not null,
    body text not null,
    created_at integer not null
);
insert into game_messages_new
    select id, game_id, player_id, display_name, body, cast(created_at as integer)
    from game_messages;
drop table game_messages;
alter table game_messages_new rename to game_messages;

-- sessions
create table sessions_new (
    id text primary key,
    player_id text not null,
    token_hash text,
    created_at integer not null,
    last_seen_at integer not null,
    expires_at integer,
    stay_logged_in integer not null default 0
);
insert into sessions_new
    select id, player_id, token_hash, cast(created_at as integer),
        cast(last_seen_at as integer), cast(expires_at as integer), stay_logged_in
    from sessions;
drop table sessions;
alter table sessions_new rename to sessions;

-- game_invitations
create table game_invitations_new (
    id text primary key,
    game_id text not null,
    invited_player_id text,
    inviting_player_id text not null,
    seat_number integer not null,
    status text not null,
    created_at integer not null,
    responded_at integer,
    invited_email text
);
insert into game_invitations_new
    select id, game_id, invited_player_id, inviting_player_id, seat_number,
        status, cast(created_at as integer), cast(responded_at as integer), invited_email
    from game_invitations;
drop table game_invitations;
alter table game_invitations_new rename to game_invitations;

-- password_reset_tokens
create table password_reset_tokens_new (
    id text primary key,
    player_id text not null,
    token_hash text not null,
    created_at integer not null,
    expires_at integer not null,
    consumed_at integer
);
insert into password_reset_tokens_new
    select id, player_id, token_hash, cast(created_at as integer),
        cast(expires_at as integer), cast(consumed_at as integer)
    from password_reset_tokens;
drop table password_reset_tokens;
alter table password_reset_tokens_new rename to password_reset_tokens;

-- player_ratings
create table player_ratings_new (
    subject_kind text not null,
    subject_id text not null,
    rating real not null default 1500,
    games_rated integer not null default 0,
    updated_at integer not null,
    primary key (subject_kind, subject_id)
);
insert into player_ratings_new
    select subject_kind, subject_id, rating, games_rated, cast(updated_at as integer)
    from player_ratings;
drop table player_ratings;
alter table player_ratings_new rename to player_ratings;

-- rating_history
create table rating_history_new (
    id text primary key,
    subject_kind text not null,
    subject_id text not null,
    game_id text not null,
    rating_before real not null,
    rating_after real not null,
    created_at integer not null
);
insert into rating_history_new
    select id, subject_kind, subject_id, game_id, rating_before, rating_after,
        cast(created_at as integer)
    from rating_history;
drop table rating_history;
alter table rating_history_new rename to rating_history;

-- Recreate every index dropped with a rebuilt table, plus the new last_seen one.
create index idx_sessions_token_hash on sessions(token_hash);
create index idx_sessions_expires_at on sessions(expires_at);
create index idx_sessions_last_seen_at on sessions(last_seen_at);
create index idx_games_status_ended_at on games(status, ended_at);
create index idx_game_invitations_game_id on game_invitations(game_id);
create index idx_game_invitations_invited_player_id on game_invitations(invited_player_id);
create index idx_game_messages_game_id on game_messages(game_id);
create index idx_rating_history_subject_created_at
    on rating_history(subject_kind, subject_id, created_at);
