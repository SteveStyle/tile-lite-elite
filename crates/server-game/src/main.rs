use std::net::SocketAddr;

use server_game::email::EmailConfig;
use server_game::{AppState, app_version, build_router};

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

    let database_url = std::env::var("TILE_LITE_ELITE_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://data/tile-lite-elite.sqlite3".to_string());
    let bind =
        std::env::var("TILE_LITE_ELITE_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    // Only used to build the link inside a password-reset email — see
    // `AppState::public_base_url`'s doc comment. Defaults to the local web
    // dev server so the flow works out of the box in dev without this var
    // set; production sets it explicitly (docker-compose.yml).
    let public_base_url = std::env::var("TILE_LITE_ELITE_PUBLIC_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    // Unset in local dev by default — every email-triggering flow still
    // works without it, just logging the message instead of sending it
    // (see EmailConfig's doc comment). Production sets both explicitly.
    let email_api_key = std::env::var("RESEND_API_KEY")
        .ok()
        .filter(|key| !key.is_empty());
    let email_from_address = std::env::var("RESEND_FROM_ADDRESS")
        .unwrap_or_else(|_| "Tile Lite Elite <noreply@mail.tileliteelite.com>".to_string());
    let email_config = EmailConfig::new(email_api_key, email_from_address);
    let state = AppState::new(&database_url, public_base_url, email_config).await?;
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(bind.parse::<SocketAddr>()?).await?;

    tracing::info!(
        %bind,
        database_url,
        app_version = %app_version(),
        api_version = %api::API_VERSION,
        "server-game starting"
    );

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
