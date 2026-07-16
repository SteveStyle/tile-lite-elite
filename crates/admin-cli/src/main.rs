//! Operator tooling for a tile-lite-elite server: list/delete users, reset
//! passwords, list/delete/force-end games. Talks to the server's `/admin/*`
//! endpoints over plain HTTP rather than touching the database directly, so
//! it can't drift from the cascading-delete/password-hashing logic the
//! server already has to get right for its own sake.
//!
//! The server only accepts these endpoints from loopback callers — running
//! this CLI IS the authentication, in the sense that you need to be on the
//! same machine as the server to reach them at all. Point `--server` at
//! anything other than the server's own loopback address and every request
//! will be rejected with 403, by design (see `require_loopback` in
//! `server-game`).

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tile-lite-elite-admin",
    about = "Administer a tile-lite-elite server (users, games). Must run on the same machine as the server."
)]
struct Cli {
    /// Base URL of the server's HTTP API. Must resolve to loopback from the
    /// server's point of view, or every request will 403.
    #[arg(long, env = "TILE_LITE_ELITE_API_BASE_URL", default_value = "http://127.0.0.1:3000")]
    server: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage player accounts.
    Users {
        #[command(subcommand)]
        action: UsersAction,
    },
    /// Manage games.
    Games {
        #[command(subcommand)]
        action: GamesAction,
    },
}

#[derive(Subcommand)]
enum UsersAction {
    /// List all registered users.
    List,
    /// Delete a user. Their past games are kept, with the seat unclaimed
    /// rather than deleted, so game history and other players' records
    /// survive.
    Delete { player_id: String },
    /// Reset a user's password. Prints the new password if you don't
    /// supply one — there's no email flow to deliver it any other way.
    ResetPassword {
        player_id: String,
        #[arg(long)]
        password: Option<String>,
    },
}

#[derive(Subcommand)]
enum GamesAction {
    /// List games, optionally filtered by status and/or age.
    List {
        /// waiting | active | finished
        #[arg(long)]
        status: Option<String>,
        /// Only games created at least this many days ago.
        #[arg(long)]
        older_than_days: Option<i64>,
    },
    /// Delete a game and all its moves/participants/invitations.
    Delete { game_id: String },
    /// Mark a stuck or abandoned game Finished without going through
    /// per-seat resignation. Doesn't touch scores.
    ForceEnd { game_id: String },
}

fn main() {
    let cli = Cli::parse();
    let client = reqwest::blocking::Client::new();

    let result = match cli.command {
        Command::Users { action } => run_users(&client, &cli.server, action),
        Command::Games { action } => run_games(&client, &cli.server, action),
    };

    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run_users(client: &reqwest::blocking::Client, server: &str, action: UsersAction) -> Result<(), String> {
    match action {
        UsersAction::List => {
            let users: Vec<api::PlayerDto> =
                check_response(client.get(format!("{server}/admin/users")).send().map_err(fmt_err)?)?
                    .json()
                    .map_err(fmt_err)?;
            if users.is_empty() {
                println!("No users.");
                return Ok(());
            }
            for user in users {
                println!(
                    "{}  {:<20}  {:<30}  last seen: {}",
                    user.id,
                    user.display_name,
                    user.email,
                    user.last_seen_at.as_deref().unwrap_or("never")
                );
            }
        }
        UsersAction::Delete { player_id } => {
            check_response(
                client
                    .delete(format!("{server}/admin/users/{player_id}"))
                    .send()
                    .map_err(fmt_err)?,
            )?;
            println!("Deleted user {player_id}.");
        }
        UsersAction::ResetPassword { player_id, password } => {
            let new_password = password.unwrap_or_else(generate_password);
            check_response(
                client
                    .post(format!("{server}/admin/users/{player_id}/reset-password"))
                    .json(&api::AdminResetPasswordRequest {
                        new_password: new_password.clone(),
                    })
                    .send()
                    .map_err(fmt_err)?,
            )?;
            println!("Password reset for {player_id}.");
            println!("New password: {new_password}");
        }
    }
    Ok(())
}

fn run_games(client: &reqwest::blocking::Client, server: &str, action: GamesAction) -> Result<(), String> {
    match action {
        GamesAction::List { status, older_than_days } => {
            let mut request = client.get(format!("{server}/admin/games"));
            let mut query = Vec::new();
            if let Some(status) = &status {
                query.push(("status", status.clone()));
            }
            if let Some(days) = older_than_days {
                query.push(("older_than_days", days.to_string()));
            }
            request = request.query(&query);

            let games: Vec<api::AdminGameSummaryDto> =
                check_response(request.send().map_err(fmt_err)?)?.json().map_err(fmt_err)?;
            if games.is_empty() {
                println!("No games match.");
                return Ok(());
            }
            for game in games {
                let players = game
                    .participants
                    .iter()
                    .map(|participant| participant.display_name.as_str())
                    .collect::<Vec<_>>()
                    .join(" vs ");
                println!(
                    "{}  {:<9}  created: {:<12}  last activity: {:<12}  {}",
                    game.id,
                    format!("{:?}", game.status).to_lowercase(),
                    game.created_at,
                    game.last_activity_at,
                    players
                );
            }
        }
        GamesAction::Delete { game_id } => {
            check_response(
                client
                    .delete(format!("{server}/admin/games/{game_id}"))
                    .send()
                    .map_err(fmt_err)?,
            )?;
            println!("Deleted game {game_id}.");
        }
        GamesAction::ForceEnd { game_id } => {
            check_response(
                client
                    .post(format!("{server}/admin/games/{game_id}/force-end"))
                    .send()
                    .map_err(fmt_err)?,
            )?;
            println!("Game {game_id} marked finished.");
        }
    }
    Ok(())
}

fn fmt_err(error: reqwest::Error) -> String {
    format!("could not reach {}: {error}", error.url().map(|u| u.as_str()).unwrap_or("server"))
}

fn check_response(response: reqwest::blocking::Response) -> Result<reqwest::blocking::Response, String> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let message = response
        .json::<api::ApiError>()
        .map(|error| error.message)
        .unwrap_or_else(|_| status.to_string());
    Err(format!("{status}: {message}"))
}

/// A 16-character password from a charset with visually-ambiguous
/// characters (0/O, 1/l/I) removed, since a human has to read this off a
/// terminal and type it somewhere.
fn generate_password() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}
