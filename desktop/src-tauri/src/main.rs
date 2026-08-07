//! penguin-desktop — Tauri shell wrapping penguin-desktop-core
//!
//! This app manages authentication via the desktop core and proxies API calls
//! to the hub through penguind. All token handling is behind the Rust layer;
//! the frontend never sees credentials or tokens.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod approval;
mod commands;
mod error;

use commands::AppState;
use penguin_desktop_core::Session;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

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

    // Initialize approval pending map for the approval prompt
    let approval_pending = Arc::new(RwLock::new(std::collections::HashMap::new()));

    // Initialize poll loop handle as None (will be set on login)
    let poll_loop = Arc::new(Mutex::new(None));

    let app_state = AppState {
        session,
        hub_url,
        approval_pending,
        poll_loop,
    };

    info!("[penguin-desktop] Initializing Tauri app");

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::login,
            commands::logout,
            commands::api_request,
            commands::oauth_start,
            commands::oauth_complete,
            commands::respond_to_approval,
        ])
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            info!("[penguin-desktop] Tauri setup complete");

            // Check if a session already exists and auto-resume the poll loop
            let app_handle = app.handle().clone();
            let state = app.state::<AppState>();

            let session_arc = state.session.clone();
            let approval_pending_arc = state.approval_pending.clone();
            let poll_loop_arc = state.poll_loop.clone();

            tokio::spawn(async move {
                let session = session_arc.lock().await;
                if session.has_existing_session().await {
                    info!("[setup] Existing session found, resuming poll loop");
                    drop(session);

                    let mut poll_loop_guard = poll_loop_arc.lock().await;
                    if let Err(e) = commands::spawn_poll_loop(
                        &app_handle,
                        session_arc.clone(),
                        approval_pending_arc.clone(),
                        &mut poll_loop_guard,
                    ).await {
                        warn!("[setup] Failed to resume poll loop: {}", e);
                    } else {
                        info!("[setup] Poll loop resumed successfully");
                    }
                } else {
                    debug!("[setup] No existing session in keychain");
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
