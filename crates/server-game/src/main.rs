use std::net::SocketAddr;

use server_game::{AppState, build_router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // RUST_LOG controls verbosity (e.g. `RUST_LOG=debug`, or
    // `RUST_LOG=server_game=debug,tower_http=debug` to scope it) — defaults
    // to `info` for this crate and `warn` for everything else so a plain
    // `docker compose logs` / journal isn't dominated by dependency noise.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "server_game=info,tower_http=info,warn".into()),
        )
        .init();

    let database_url = std::env::var("SCRABBLE_PX_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://data/scrabble-px.sqlite3".to_string());
    let bind = std::env::var("SCRABBLE_PX_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let state = AppState::new(&database_url).await?;
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(bind.parse::<SocketAddr>()?).await?;

    tracing::info!(%bind, database_url, "server-game starting");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
