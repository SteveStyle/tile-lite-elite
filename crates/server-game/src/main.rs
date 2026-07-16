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

    let database_url = std::env::var("TILE_LITE_ELITE_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://data/tile-lite-elite.sqlite3".to_string());
    let bind = std::env::var("TILE_LITE_ELITE_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let state = AppState::new(&database_url).await?;
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

/// The `Major.Minor.Patch` release version from Cargo.toml, plus an
/// optional build identifier appended as SemVer build metadata (`+<id>`)
/// when `TILE_LITE_ELITE_BUILD_ID` is set at compile time — e.g. a git short
/// SHA or CI run number, for telling internal/test builds apart. A
/// production release simply doesn't set that var, so it shows only the
/// three numbers. Distinct from `api::API_VERSION`: this is the build
/// identity, not the wire-contract version clients check on connect.
fn app_version() -> String {
    format_app_version(env!("CARGO_PKG_VERSION"), option_env!("TILE_LITE_ELITE_BUILD_ID"))
}

fn format_app_version(pkg_version: &str, build_id: Option<&str>) -> String {
    match build_id {
        Some(id) if !id.is_empty() => format!("{pkg_version}+{id}"),
        _ => pkg_version.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_build_id_is_three_numbers_only() {
        assert_eq!(format_app_version("0.1.0", None), "0.1.0");
    }

    #[test]
    fn empty_build_id_is_treated_as_absent() {
        assert_eq!(format_app_version("0.1.0", Some("")), "0.1.0");
    }

    #[test]
    fn build_id_appends_as_semver_build_metadata() {
        assert_eq!(
            format_app_version("0.1.0", Some("a1c9f02")),
            "0.1.0+a1c9f02"
        );
    }
}
