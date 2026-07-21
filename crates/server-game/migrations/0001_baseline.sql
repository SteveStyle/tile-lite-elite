-- Baseline schema, captured as-is from the ad-hoc `create table if not
-- exists` / `create index if not exists` statements that used to live in
-- persistence::migrate(). Written this way (idempotent DDL) specifically so
-- this file is a safe no-op against every database that already has this
-- exact shape from the old code path (production, and any local dev DB) --
-- NOT because "if not exists" is generally an acceptable pattern for
-- migrations going forward. See migrations/README.md: every migration after
-- this one must use real, non-silently-skippable DDL (alter table, create
-- table without "if not exists", etc.), and THIS FILE MUST NEVER BE EDITED
-- AGAIN once it has shipped anywhere (local dev or production) — sqlx
-- checksums applied migrations and refuses to start if one's content
-- changes after the fact. Add a new numbered migration instead.

-- Dead scaffolding from the old migrate() — created every startup but never
-- inserted into or read from anywhere. Superseded by sqlx's own migration
-- tracking table (`_sqlx_migrations`, created automatically). Safe to drop
-- unconditionally: a no-op on any database that never had it.
drop table if exists schema_migrations;

create table if not exists players (
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
);

create table if not exists engine_profiles (
    id text primary key,
    name text not null,
    version text not null,
    author text,
    description text,
    capabilities_json text not null,
    created_at text not null,
    updated_at text not null
);

create table if not exists games (
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
);

create table if not exists game_participants (
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
);

create table if not exists game_moves (
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
);

create table if not exists game_messages (
    id text primary key,
    game_id text not null,
    player_id text not null,
    display_name text not null,
    body text not null,
    created_at text not null
);

create table if not exists sessions (
    id text primary key,
    player_id text not null,
    game_id text,
    token_hash text,
    created_at text not null,
    last_seen_at text not null,
    expires_at text,
    stay_logged_in integer not null default 0
);

create table if not exists game_invitations (
    id text primary key,
    game_id text not null,
    invited_player_id text,
    inviting_player_id text not null,
    seat_number integer not null,
    status text not null,
    created_at text not null,
    responded_at text,
    invited_email text
);

create table if not exists password_reset_tokens (
    id text primary key,
    player_id text not null,
    token_hash text not null,
    created_at text not null,
    expires_at text not null,
    consumed_at text
);

create index if not exists idx_sessions_token_hash on sessions(token_hash);
create index if not exists idx_games_status_ended_at on games(status, ended_at);
create index if not exists idx_sessions_expires_at on sessions(expires_at);
create index if not exists idx_game_invitations_game_id on game_invitations(game_id);
create index if not exists idx_game_invitations_invited_player_id on game_invitations(invited_player_id);
create index if not exists idx_game_messages_game_id on game_messages(game_id);
