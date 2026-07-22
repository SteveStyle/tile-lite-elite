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

use std::time::{Duration, Instant};

use api::{GameStatus, SeatKind};
use rules_shared::{Rack, VariantRules};
use server_game::game_state::{EngineRegistry, GameSession, ParticipantState};

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
    println!("min:          {:>8.2} ms", samples_ms[0]);
    println!("Q1 (25th):    {:>8.2} ms", percentile(&samples_ms, 0.25));
    println!("median (50th):{:>8.2} ms", percentile(&samples_ms, 0.50));
    println!("mean:         {:>8.2} ms", mean);
    println!("Q3 (75th):    {:>8.2} ms", percentile(&samples_ms, 0.75));
    println!("p95:          {:>8.2} ms", percentile(&samples_ms, 0.95));
    println!("p99:          {:>8.2} ms", percentile(&samples_ms, 0.99));
    println!("max:          {:>8.2} ms", samples_ms[n - 1]);
}
