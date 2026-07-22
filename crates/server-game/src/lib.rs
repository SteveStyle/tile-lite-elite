pub mod app;
pub mod email;
pub mod game_state;
pub mod persistence;
pub mod stats;

pub use app::{AppState, app_version, build_router};
