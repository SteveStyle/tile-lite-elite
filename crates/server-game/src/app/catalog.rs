use super::*;

pub(crate) async fn health() -> Json<api::HealthDto> {
    Json(api::HealthDto {
        status: "ok".to_string(),
        api_version: api::API_VERSION,
        app_version: app_version(),
    })
}

pub(crate) async fn list_engines(
    State(state): State<AppState>,
) -> Json<Vec<api::EngineProfileDto>> {
    Json(state.engines.metadata())
}

/// Serves a dictionary's raw word-list text on request, for clients (the
/// wasm/web build specifically) that fetch it at runtime rather than
/// embedding it at compile time — the server already has this exact text
/// compiled in (`rules_shared::sowpods_word_list`), so this is just
/// re-serving it, not a second copy of the file anywhere. Unauthenticated,
/// same as `/health`/`/engines` — a word list isn't sensitive, and every
/// signed-in player's client needs it regardless of which game they're in.
pub(crate) async fn get_dictionary(Path(name): Path<String>) -> Result<String, ApiProblem> {
    match name.as_str() {
        "sowpods" => Ok(rules_shared::sowpods_word_list().to_string()),
        "enable2k" => Ok(rules_shared::enable2k_word_list().to_string()),
        "german" => Ok(rules_shared::german_word_list().to_string()),
        "spanish" => Ok(rules_shared::spanish_word_list().to_string()),
        _ => Err(ApiProblem::not_found(format!(
            "Unknown dictionary '{name}'"
        ))),
    }
}
