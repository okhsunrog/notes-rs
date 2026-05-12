mod agent;
mod commands;
mod db;
mod embed;
mod extract;
mod stem;

use std::sync::Arc;
use tauri::Manager;

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
            let db_path = data_dir.join("notes.db");

            tauri::async_runtime::spawn(async move {
                let embedder = embed::make_embedder().expect("loading embedder");
                let id = embedder.id();
                let ndims = embedder.ndims();
                tracing::info!(embedder = %id, ndims, "embedder loaded");
                let conn = db::open(&db_path, &id, ndims).await.expect("opening db");
                let reranker = Arc::new(embed::Reranker::new().expect("loading reranker"));
                embed::spawn_worker(conn.clone(), embedder.clone());
                let extractor = Arc::new(extract::EntityExtractor::new());
                extract::spawn_worker(conn.clone(), extractor);
                handle.manage(commands::AppState {
                    conn,
                    embedder,
                    reranker,
                });
                tracing::info!(?db_path, "notes-rs ready");
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
