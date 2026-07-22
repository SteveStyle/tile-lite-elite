//! Dev-only benchmark: how long `GreedyEngine::choose_action` actually
//! takes per move, measured across real games (not synthetic positions) —
//! plays engine-vs-engine games start to finish via the exact same
//! `GameSession::maybe_run_engine_turn` production Bot Showdown games use,
//! timing each call. Never built or run as part of the shipped
//! server/image (examples aren't part of the release binary) — this is a
//! `cargo run --example` tool only.
//!
//! Usage: `cargo run --release --example engine_timing_bench [num_games]`
//! (release build matters here — move generation is meaningfully slower
//! under a debug build, and would skew the numbers away from what a real
//! deployed server actually experiences).
//!
//! Every run appends one row to `engine_timing_results.csv` (alongside
//! this file, found via `CARGO_MANIFEST_DIR` so it works regardless of
//! the directory `cargo run` was invoked from) — a running, git-committed
//! log of every benchmark run, so later runs (e.g. after an engine change)
//! can be compared against earlier ones rather than only ever seeing the
//! latest numbers.

use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command;
use std::time::{Duration, Instant};

use api::{GameStatus, SeatKind};
use rules_shared::{Rack, VariantRules};
use server_game::game_state::{EngineRegistry, GameSession, ParticipantState};

const RESULTS_CSV_HEADER: &str = "timestamp_unix_seconds,git_commit,num_games,games_completed,samples,min_ms,q1_ms,median_ms,mean_ms,q3_ms,p95_ms,p99_ms,max_ms\n";

/// The short commit hash `HEAD` is on, with a `-dirty` suffix if the working
/// tree has uncommitted changes — unlike a real deploy (which refuses a
/// dirty tree), benchmarking mid-change is completely normal here, so this
/// records rather than blocks it. `"unknown"` if `git` isn't available at
/// all (e.g. run from outside a checkout).
fn git_commit_label() -> String {
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .is_some_and(|output| output.status.success() && !output.stdout.is_empty());
    if dirty { format!("{hash}-dirty") } else { hash }
}

fn append_result_row(row: &str) -> std::io::Result<std::path::PathBuf> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/engine_timing_results.csv");
    let is_new_file = !path.exists();
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    if is_new_file {
        file.write_all(RESULTS_CSV_HEADER.as_bytes())?;
    }
    file.write_all(row.as_bytes())?;
    Ok(path)
}

const ENGINE_ID: &str = "greedy-v1";
const ENGINE_TURN_TIMEOUT: Duration = Duration::from_secs(5);
// Defensive cap matching production's own MAX_ENGINE_TURNS_PER_TRIGGER, in
// case a future rules bug ever made a game fail to terminate.
const MAX_TURNS_PER_GAME: usize = 500;

fn engine_participant(seat_number: u8, display_name: &str) -> ParticipantState {
    ParticipantState {
        seat_number,
        kind: SeatKind::Engine,
        display_name: display_name.to_string(),
        player_id: None,
        engine_id: Some(ENGINE_ID.to_string()),
        score: 0,
        rack: Rack::default(),
        resigned: false,
        removed_by_player: false,
        invited_email: None,
    }
}

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let idx = (p * (sorted_ms.len() as f64 - 1.0)).round() as usize;
    sorted_ms[idx.min(sorted_ms.len() - 1)]
}

#[tokio::main]
async fn main() {
    let num_games: usize = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(30);

    let engines = EngineRegistry::default();
    let mut samples_ms: Vec<f64> = Vec::new();
    let mut games_completed = 0usize;

    for game_index in 0..num_games {
        let rules = VariantRules::official();
        let participants = vec![
            engine_participant(0, "Greedy A"),
            engine_participant(1, "Greedy B"),
        ];
        let mut game = GameSession::new(
            format!("bench-{game_index}"),
            participants,
            None,
            game_index as u64,
            rules,
            72 * 60 * 60,
        );
        game.start();

        for _ in 0..MAX_TURNS_PER_GAME {
            let before = Instant::now();
            let advanced = game
                .maybe_run_engine_turn(&engines, ENGINE_TURN_TIMEOUT)
                .await
                .expect("engine turn should not error in a clean engine-vs-engine game");
            samples_ms.push(before.elapsed().as_secs_f64() * 1000.0);
            if !advanced || game.status != GameStatus::Active {
                break;
            }
        }
        if game.status == GameStatus::Finished {
            games_completed += 1;
        }
    }

    samples_ms.sort_by(|a, b| a.partial_cmp(b).expect("no NaNs in timing data"));
    let n = samples_ms.len();
    let mean = samples_ms.iter().sum::<f64>() / n as f64;

    println!("games played: {num_games} ({games_completed} reached Finished)");
    println!("move-timing samples: {n}");
    println!();
    let min = samples_ms[0];
    let q1 = percentile(&samples_ms, 0.25);
    let median = percentile(&samples_ms, 0.50);
    let q3 = percentile(&samples_ms, 0.75);
    let p95 = percentile(&samples_ms, 0.95);
    let p99 = percentile(&samples_ms, 0.99);
    let max = samples_ms[n - 1];

    println!("min:          {min:>8.2} ms");
    println!("Q1 (25th):    {q1:>8.2} ms");
    println!("median (50th):{median:>8.2} ms");
    println!("mean:         {mean:>8.2} ms");
    println!("Q3 (75th):    {q3:>8.2} ms");
    println!("p95:          {p95:>8.2} ms");
    println!("p99:          {p99:>8.2} ms");
    println!("max:          {max:>8.2} ms");

    let timestamp_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs();
    let row = format!(
        "{timestamp_unix_seconds},{},{num_games},{games_completed},{n},{min:.2},{q1:.2},{median:.2},{mean:.2},{q3:.2},{p95:.2},{p99:.2},{max:.2}\n",
        git_commit_label(),
    );
    match append_result_row(&row) {
        Ok(path) => println!("\nrecorded to {}", path.display()),
        Err(error) => eprintln!("\nfailed to record results to engine_timing_results.csv: {error}"),
    }
}
