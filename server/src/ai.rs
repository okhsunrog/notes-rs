use crate::config::AiConfig;
use crate::state::UserRegistry;
use anyhow::{Context, Result, bail};
use notes_ai::agent;
use notes_ai::embed::EmbedderBackend;
use notes_ai::retrieval::RetrievalPipeline;
use notes_ai::store::{AiStore, embedding_identity_fingerprint};
use notes_core::Connection;
use notes_core::db::{self, SearchHit};
use notes_protocol::{ChatEvent, ChatTurn};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
struct UserAi {
    notes: Connection,
    store: Arc<AiStore>,
    retrieval: RetrievalPipeline,
}

#[derive(Clone)]
pub struct AiRuntime {
    users: Arc<HashMap<String, UserAi>>,
    chat: llm_relay::ClientConfig,
    extraction: llm_relay::ClientConfig,
    entity_extraction_enabled: bool,
    query_rewriting_enabled: bool,
}

impl AiRuntime {
    pub async fn open(config: &AiConfig, registry: &UserRegistry, data_dir: &Path) -> Result<Self> {
        let embedder = notes_ai::embed::make_openrouter_embedder(
            config.openrouter_api_key.clone(),
            &config.openrouter_base_url,
            config.embedding_model.clone(),
            config.embedding_dimensions,
        )?;
        let reranker = notes_ai::embed::make_openrouter_reranker(
            config.openrouter_api_key.clone(),
            &config.openrouter_base_url,
            config.rerank_model.clone(),
        )?;
        let chat = notes_ai::config::completion_config(
            notes_ai::config::CompletionProtocol::Openai,
            Some(config.openrouter_base_url.clone()),
            Some(config.openrouter_api_key.clone()),
            config.chat_model.clone(),
        )?;
        let extraction = notes_ai::config::completion_config(
            notes_ai::config::CompletionProtocol::Openai,
            Some(config.openrouter_base_url.clone()),
            Some(config.openrouter_api_key.clone()),
            config.extraction_model.clone(),
        )?;
        let identity = embedding_identity_fingerprint(
            &config.openrouter_base_url,
            &config.embedding_model,
            config.embedding_dimensions,
        );
        let mut users = HashMap::new();
        for user in registry.users() {
            let store = Arc::new(
                AiStore::open(
                    data_dir.join("users").join(&user.id).join("ai.db"),
                    identity.clone(),
                    config.embedding_dimensions,
                )
                .await
                .with_context(|| format!("opening AI store for {}", user.id))?,
            );
            let retrieval = RetrievalPipeline::new(
                user.notes.clone(),
                store.clone(),
                embedder.clone(),
                reranker.clone(),
            );
            users.insert(
                user.id.clone(),
                UserAi {
                    notes: user.notes.clone(),
                    store,
                    retrieval,
                },
            );
        }
        let runtime = Self {
            users: Arc::new(users),
            chat,
            extraction,
            entity_extraction_enabled: config.entity_extraction_enabled,
            query_rewriting_enabled: config.query_rewriting_enabled,
        };
        runtime.spawn_workers(embedder);
        Ok(runtime)
    }

    fn spawn_workers(&self, embedder: Arc<dyn EmbedderBackend>) {
        for user in self.users.values() {
            let paused = Arc::new(AtomicBool::new(false));
            notes_ai::embed::spawn_worker(
                user.notes.clone(),
                user.store.clone(),
                embedder.clone(),
                Arc::new(|| {}),
                paused.clone(),
            );
            if self.entity_extraction_enabled {
                notes_ai::extract::spawn_worker(
                    user.notes.clone(),
                    user.store.clone(),
                    Arc::new(notes_ai::extract::EntityExtractor::new(
                        self.extraction.clone(),
                    )),
                    Arc::new(|| {}),
                    Arc::new(|| {}),
                    paused,
                );
            }
        }
    }

    fn user(&self, user_id: &str) -> Result<&UserAi> {
        self.users
            .get(user_id)
            .with_context(|| format!("AI runtime is unavailable for user {user_id}"))
    }

    pub async fn search(&self, user_id: &str, query: String, limit: u32) -> Result<Vec<SearchHit>> {
        if query.trim().is_empty() || query.chars().count() > 4_096 {
            bail!("query must contain 1 to 4096 characters");
        }
        if !(1..=100).contains(&limit) {
            bail!("limit must be between 1 and 100");
        }
        self.user(user_id)?.retrieval.retrieve(query, limit).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn chat(
        &self,
        user_id: &str,
        history: Vec<ChatTurn>,
        message: String,
        allow_writes: bool,
        active_node_uuid: Option<uuid::Uuid>,
        cancelled: Arc<AtomicBool>,
        emit: impl Fn(ChatEvent) + Send + Sync + 'static,
    ) -> Result<String> {
        let user = self.user(user_id)?;
        let active_node_id = match active_node_uuid {
            Some(uuid) => db::get_node_by_uuid(&user.notes, uuid)
                .await?
                .map(|node| node.id),
            None => None,
        };
        if cancelled.load(Ordering::Acquire) {
            bail!("chat request was cancelled");
        }
        agent::run_chat_stream_with_config(
            user.retrieval.clone(),
            history,
            message,
            allow_writes,
            active_node_id,
            cancelled,
            emit,
            self.chat.clone(),
            self.query_rewriting_enabled,
        )
        .await
        .map_err(anyhow::Error::from)
    }
}
