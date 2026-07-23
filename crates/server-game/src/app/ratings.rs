use super::*;

/// Same openness level as `search_players`: gated on being signed in at
/// all, not on being the subject in question — this app has no public/
/// private stats distinction. Never 404s; an unrated player just comes
/// back as rating 1500 with every counter at 0 (see
/// `stats::get_subject_stats`).
pub(crate) async fn get_player_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(player_id): Path<String>,
) -> Result<Json<api::PlayerStatsDto>, ApiProblem> {
    authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to view stats"))?;
    let stats = stats::get_subject_stats(&state.db, "player", &player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    Ok(Json(stats))
}

pub(crate) async fn get_player_rating_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(player_id): Path<String>,
) -> Result<Json<Vec<api::RatingPointDto>>, ApiProblem> {
    authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to view rating history"))?;
    let history = stats::get_rating_history(&state.db, "player", &player_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    Ok(Json(history))
}

/// The engine (bot) counterpart to `get_player_stats` — a bot is a rating
/// subject too (see `stats::settle_ratings`'s doc comment), so it gets the
/// same read surface.
pub(crate) async fn get_engine_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(engine_id): Path<String>,
) -> Result<Json<api::PlayerStatsDto>, ApiProblem> {
    authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to view stats"))?;
    let stats = stats::get_subject_stats(&state.db, "engine", &engine_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    Ok(Json(stats))
}

pub(crate) async fn get_engine_rating_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(engine_id): Path<String>,
) -> Result<Json<Vec<api::RatingPointDto>>, ApiProblem> {
    authenticated_player_id(&state, &headers)
        .await
        .ok_or_else(|| ApiProblem::unauthorized("Sign in to view rating history"))?;
    let history = stats::get_rating_history(&state.db, "engine", &engine_id)
        .await
        .map_err(ApiProblem::from_sqlx)?;
    Ok(Json(history))
}
