mod agent;
mod commands;
mod db;
mod embed;
mod extract;
mod stem;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = app.path().app_data_dir().expect("resolving app data dir");
            std::fs::create_dir_all(&data_dir).expect("creating data dir");

            // Desktop launches don't inherit shell env. Load .env from the app
            // data dir if present so API keys configured by the user survive
            // double-click launches. Missing file is fine.
            let env_path = data_dir.join(".env");
            match dotenvy::from_path(&env_path) {
                Ok(()) => tracing::info!(?env_path, "loaded .env"),
                Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(?env_path, error = %e, "failed to load .env"),
            }

            let db_path = data_dir.join("notes.db");

            // Register the readiness flag immediately so the frontend can
            // poll/listen instead of invoking commands that would otherwise
            // fail with an opaque "state not managed" error during the
            // (slow, first-run) embedder model download.
            let ready = Arc::new(AtomicBool::new(false));
            handle.manage(commands::Startup {
                ready: ready.clone(),
            });

            tauri::async_runtime::spawn(async move {
                let embedder = embed::make_embedder().expect("loading embedder");
                let id = embedder.id();
                let ndims = embedder.ndims();
                tracing::info!(embedder = %id, ndims, "embedder loaded");
                let conn = db::open(&db_path, &id, ndims).await.expect("opening db");
                let reranker = Arc::new(embed::Reranker::new().expect("loading reranker"));
                embed::spawn_worker(conn.clone(), embedder.clone());
                let extractor = Arc::new(extract::EntityExtractor::new());
                extract::spawn_worker(conn.clone(), extractor, handle.clone());
                handle.manage(commands::AppState {
                    conn,
                    embedder,
                    reranker,
                });
                ready.store(true, Ordering::Release);
                let _ = handle.emit("app:ready", ());
                tracing::info!(?db_path, "notes-rs ready");
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::is_ready,
            commands::create_node,
            commands::update_node,
            commands::link_nodes,
            commands::get_node,
            commands::neighbors,
            commands::search_fts,
            commands::search_vec,
            commands::search_hybrid,
            commands::search_agentic,
            commands::rerank,
            commands::chat,
            commands::chat_stream,
            commands::list_entities,
            commands::list_pages,
            commands::create_page,
            commands::list_block_children,
            commands::create_block,
            commands::move_block,
            commands::delete_block,
            commands::replace_block_refs,
            commands::get_or_create_page_by_title,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
