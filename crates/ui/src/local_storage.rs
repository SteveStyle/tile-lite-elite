//! Persists a small amount of auth state across app restarts:
//! - `remembered_name`: pure convenience, just pre-fills the display-name
//!   field next time ("Remember me"). No security weight.
//! - `session_token`: the actual bearer token, kept only if the person
//!   checked "Stay logged in".
//!
//! Storage differs by platform since there's no browser localStorage on
//! desktop: web uses `localStorage`, native writes a small JSON file
//! under the OS config directory.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredAuth {
    pub remembered_name: Option<String>,
    pub session_token: Option<String>,
}

const STORAGE_KEY: &str = "scrabble_px_auth";

pub fn load() -> StoredAuth {
    load_impl().unwrap_or_default()
}

pub fn save(auth: &StoredAuth) {
    let _ = save_impl(auth);
}

#[cfg(target_arch = "wasm32")]
fn load_impl() -> Option<StoredAuth> {
    gloo_storage::LocalStorage::get(STORAGE_KEY).ok()
}

#[cfg(target_arch = "wasm32")]
fn save_impl(auth: &StoredAuth) -> Result<(), String> {
    gloo_storage::LocalStorage::set(STORAGE_KEY, auth).map_err(|error| error.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn config_file_path() -> Option<std::path::PathBuf> {
    let mut dir = dirs::config_dir()?;
    dir.push("scrabble-px");
    std::fs::create_dir_all(&dir).ok()?;
    dir.push("auth.json");
    Some(dir)
}

#[cfg(not(target_arch = "wasm32"))]
fn load_impl() -> Option<StoredAuth> {
    let path = config_file_path()?;
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn save_impl(auth: &StoredAuth) -> Result<(), String> {
    let path = config_file_path().ok_or_else(|| "Could not resolve config directory".to_string())?;
    let contents = serde_json::to_string(auth).map_err(|error| error.to_string())?;
    std::fs::write(path, contents).map_err(|error| error.to_string())
}
