use crate::config::AiConfig;
use crate::state::UserRegistry;
use anyhow::{Context, Result, bail};
use notes_ai::agent;
use notes_ai::embed::EmbedderBackend;
use notes_ai::retrieval::RetrievalPipeline;
use notes_ai::store::{AiControl, AiStore, GenerationStatus, embedding_identity_fingerprint};
use notes_core::Connection;
use notes_core::db::{self, SearchHit};
use notes_protocol::{AiGenerationState, AiIndexStatus, AiRuntimeSettings, ChatEvent, ChatTurn};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct UserAi {
    source: Arc<crate::state::UserState>,
    notes: Connection,
    store: Arc<AiStore>,
    retrieval: RetrievalPipeline,
    embed_wake: Arc<Notify>,
    extract_wake: Arc<Notify>,
}

#[derive(Clone)]
pub struct AiRuntime {
    users: Arc<HashMap<String, UserAi>>,
    chat: llm_relay::ClientConfig,
    extraction: llm_relay::ClientConfig,
    embedding_provider_id: String,
    embedding_model: String,
    embedding_dimensions: usize,
    rerank_model: String,
    chat_model: String,
    extraction_model: String,
}

impl AiRuntime {
    pub async fn open(
        config: &AiConfig,
        registry: &UserRegistry,
        data_dir: &Path,
        shutdown: CancellationToken,
    ) -> Result<Self> {
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
            let embed_wake = Arc::new(Notify::new());
            let extract_wake = Arc::new(Notify::new());
            let store = Arc::new(
                AiStore::open_with_control(
                    data_dir.join("users").join(&user.id).join("ai.db"),
                    identity.clone(),
                    config.embedding_dimensions,
                    AiControl {
                        automatic_embeddings: config.automatic_embeddings,
                        entity_extraction: config.entity_extraction_enabled,
                        query_rewriting: config.query_rewriting_enabled,
                    },
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
                    source: user.clone(),
                    notes: user.notes.clone(),
                    store,
                    retrieval,
                    embed_wake: embed_wake.clone(),
                    extract_wake: extract_wake.clone(),
                },
            );
            let mut changes = user.subscribe();
            let change_shutdown = shutdown.child_token();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = change_shutdown.cancelled() => return,
                        result = changes.recv() => match result {
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                embed_wake.notify_one();
                                extract_wake.notify_one();
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        },
                    }
                }
            });
        }
        let runtime = Self {
            users: Arc::new(users),
            chat,
            extraction,
            embedding_provider_id: format!("openrouter:{}", config.embedding_model),
            embedding_model: config.embedding_model.clone(),
            embedding_dimensions: config.embedding_dimensions,
            rerank_model: config.rerank_model.clone(),
            chat_model: config.chat_model.clone(),
            extraction_model: config.extraction_model.clone(),
        };
        runtime.spawn_workers(embedder, shutdown);
        Ok(runtime)
    }

    fn spawn_workers(&self, embedder: Arc<dyn EmbedderBackend>, shutdown: CancellationToken) {
        for user in self.users.values() {
            notes_ai::embed::spawn_worker(
                user.notes.clone(),
                user.store.clone(),
                embedder.clone(),
                Arc::new(|| {}),
                user.embed_wake.clone(),
                shutdown.child_token(),
            );
            let entity_source = user.source.clone();
            let entity_embed_wake = user.embed_wake.clone();
            let entity_extract_wake = user.extract_wake.clone();
            notes_ai::extract::spawn_worker(
                user.notes.clone(),
                user.store.clone(),
                Arc::new(notes_ai::extract::EntityExtractor::new(
                    self.extraction.clone(),
                )),
                Arc::new(move || {
                    entity_source.notify_outbox();
                    entity_embed_wake.notify_one();
                    entity_extract_wake.notify_one();
                }),
                Arc::new(|| {}),
                user.extract_wake.clone(),
                shutdown.child_token(),
            );
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

    pub async fn status(&self, user_id: &str) -> Result<AiIndexStatus> {
        let user = self.user(user_id)?;
        let source_nodes = user.store.source_node_count(&user.notes).await?;
        let status = user.store.status(source_nodes).await?;
        Ok(AiIndexStatus {
            embedding_provider_id: self.embedding_provider_id.clone(),
            embedding_model: self.embedding_model.clone(),
            embedding_dimensions: self.embedding_dimensions,
            rerank_model: self.rerank_model.clone(),
            chat_model: self.chat_model.clone(),
            extraction_model: self.extraction_model.clone(),
            generation_id: status.generation_id,
            generation_state: match status.generation_status {
                GenerationStatus::Building => AiGenerationState::Building,
                GenerationStatus::Active => AiGenerationState::Active,
                GenerationStatus::Retired => AiGenerationState::Retired,
            },
            settings: AiRuntimeSettings {
                automatic_embeddings: status.control.automatic_embeddings,
                entity_extraction: status.control.entity_extraction,
                query_rewriting: status.control.query_rewriting,
            },
            pending_embeddings: status.pending,
            failed_embeddings: status.failed,
            indexed_nodes: status.indexed,
            source_nodes: status.source_nodes,
            pending_extractions: status.extraction_pending,
            failed_extractions: status.extraction_failed,
        })
    }

    pub async fn update_settings(
        &self,
        user_id: &str,
        settings: AiRuntimeSettings,
    ) -> Result<AiIndexStatus> {
        let user = self.user(user_id)?;
        user.store
            .set_control(AiControl {
                automatic_embeddings: settings.automatic_embeddings,
                entity_extraction: settings.entity_extraction,
                query_rewriting: settings.query_rewriting,
            })
            .await?;
        user.embed_wake.notify_one();
        user.extract_wake.notify_one();
        self.status(user_id).await
    }

    pub async fn reindex(&self, user_id: &str) -> Result<AiIndexStatus> {
        let user = self.user(user_id)?;
        user.store.reset_index().await?;
        user.embed_wake.notify_one();
        user.extract_wake.notify_one();
        self.status(user_id).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn chat(
        &self,
        user_id: &str,
        history: Vec<ChatTurn>,
        message: String,
        allow_writes: bool,
        active_node_uuid: Option<uuid::Uuid>,
        cancelled: CancellationToken,
        emit: impl Fn(ChatEvent) + Send + Sync + 'static,
    ) -> Result<String> {
        let user = self.user(user_id)?;
        let query_rewriting = user.store.control().await?.query_rewriting;
        let active_node_id = match active_node_uuid {
            Some(uuid) => db::get_node_by_uuid(&user.notes, uuid)
                .await?
                .map(|node| node.id),
            None => None,
        };
        if cancelled.is_cancelled() {
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
            query_rewriting,
        )
        .await
        .map_err(anyhow::Error::from)
    }
}
