//! penguin-desktop — Tauri shell wrapping penguin-desktop-core
//!
//! This app manages authentication via the desktop core and proxies API calls
//! to the hub through penguind. All token handling is behind the Rust layer;
//! the frontend never sees credentials or tokens.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod error;

use commands::AppState;
use penguin_desktop_core::Session;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

#[tokio::main]
async fn main() {
    // Initialize tracing for structured logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::stdout)
        .init();

    // Initialize Session with the default data directory and IPC socket
    // Use home directory / .penguin-desktop as the default data dir
    let data_dir = dirs::home_dir()
        .map(|h| h.join(".penguin-desktop"))
        .unwrap_or_else(|| PathBuf::from("~/.penguin-desktop"));

    let session = match Session::new(data_dir.clone()).await {
        Ok(s) => {
            info!("[penguin-desktop] Session initialized (data_dir: {})", data_dir.display());
            Arc::new(Mutex::new(s))
        }
        Err(e) => {
            warn!("[penguin-desktop] Failed to initialize session: {}", e);
            std::process::exit(1);
        }
    };

    // Hub URL will be set at login; for now, use a placeholder
    let hub_url = "https://hub.penguintech.cloud".to_string();

    let app_state = AppState { session, hub_url };

    info!("[penguin-desktop] Initializing Tauri app");

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::login,
            commands::logout,
            commands::api_request,
            commands::oauth_start,
            commands::oauth_complete,
        ])
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            info!("[penguin-desktop] Tauri setup complete");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
