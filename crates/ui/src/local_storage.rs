//! Persists a small amount of state across app restarts:
//! - `StoredAuth`: `remembered_name` (pure convenience, just pre-fills the
//!   display-name field next time — no security weight) and `session_token`
//!   (the actual bearer token, kept only if the person checked "Stay
//!   logged in").
//! - `StoredChatWatermarks`: per-game "last seen chat message" markers, so
//!   an unread-messages indicator survives a reload. There's no server-side
//!   read-receipt concept — this is purely local to the device/browser.
//!
//! Storage differs by platform since there's no browser localStorage on
//! desktop: web uses `localStorage`, native writes a small JSON file
//! under the OS config directory.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(target_arch = "wasm32")]
use gloo_storage::Storage;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredAuth {
    pub remembered_name: Option<String>,
    pub session_token: Option<String>,
}

#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "tile_lite_elite_auth";

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
    dir.push("tile-lite-elite");
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
    let path =
        config_file_path().ok_or_else(|| "Could not resolve config directory".to_string())?;
    let contents = serde_json::to_string(auth).map_err(|error| error.to_string())?;
    std::fs::write(path, contents).map_err(|error| error.to_string())
}

/// game_id -> the `created_at` of the last chat message this device has
/// seen for that game. A game with no entry (or an entry that doesn't
/// match the latest message) has unread chat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredChatWatermarks {
    pub last_seen: HashMap<String, i64>,
}

#[cfg(target_arch = "wasm32")]
const CHAT_STORAGE_KEY: &str = "tile_lite_elite_chat_seen";

pub fn load_chat_watermarks() -> StoredChatWatermarks {
    load_chat_watermarks_impl().unwrap_or_default()
}

pub fn save_chat_watermarks(watermarks: &StoredChatWatermarks) {
    let _ = save_chat_watermarks_impl(watermarks);
}

#[cfg(target_arch = "wasm32")]
fn load_chat_watermarks_impl() -> Option<StoredChatWatermarks> {
    gloo_storage::LocalStorage::get(CHAT_STORAGE_KEY).ok()
}

#[cfg(target_arch = "wasm32")]
fn save_chat_watermarks_impl(watermarks: &StoredChatWatermarks) -> Result<(), String> {
    gloo_storage::LocalStorage::set(CHAT_STORAGE_KEY, watermarks).map_err(|error| error.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn chat_watermarks_file_path() -> Option<std::path::PathBuf> {
    let mut dir = dirs::config_dir()?;
    dir.push("tile-lite-elite");
    std::fs::create_dir_all(&dir).ok()?;
    dir.push("chat_watermarks.json");
    Some(dir)
}

#[cfg(not(target_arch = "wasm32"))]
fn load_chat_watermarks_impl() -> Option<StoredChatWatermarks> {
    let path = chat_watermarks_file_path()?;
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn save_chat_watermarks_impl(watermarks: &StoredChatWatermarks) -> Result<(), String> {
    let path = chat_watermarks_file_path()
        .ok_or_else(|| "Could not resolve config directory".to_string())?;
    let contents = serde_json::to_string(watermarks).map_err(|error| error.to_string())?;
    std::fs::write(path, contents).map_err(|error| error.to_string())
}
