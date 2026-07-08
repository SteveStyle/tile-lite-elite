use std::net::SocketAddr;

use server_game::{AppState, build_router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("SCRABBLE_PX_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://data/scrabble-px.sqlite3".to_string());
    let bind = std::env::var("SCRABBLE_PX_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let state = AppState::new(&database_url).await?;
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(bind.parse::<SocketAddr>()?).await?;

    axum::serve(listener, app).await?;
    Ok(())
}
