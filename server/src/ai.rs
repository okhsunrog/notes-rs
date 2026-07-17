use crate::config::AiConfig;
use anyhow::{Context, Result, bail};
use notes_ai::agent;
use notes_ai::embed::{EmbedderBackend, RerankBackend};
use notes_core::Connection;
use notes_core::db::{self, SearchHit};
use notes_protocol::{ChatEvent, ChatTurn};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub struct AiRuntime {
    pub embedder: Arc<dyn EmbedderBackend>,
    pub reranker: Arc<dyn RerankBackend>,
    chat: llm_relay::ClientConfig,
    extraction: llm_relay::ClientConfig,
    entity_extraction_enabled: bool,
    query_rewriting_enabled: bool,
}

impl AiRuntime {
    pub fn new(config: &AiConfig) -> Result<Self> {
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
        Ok(Self {
            embedder,
            reranker,
            chat,
            extraction,
            entity_extraction_enabled: config.entity_extraction_enabled,
            query_rewriting_enabled: config.query_rewriting_enabled,
        })
    }

    pub fn spawn_workers(&self, connection: Connection) {
        let paused = Arc::new(AtomicBool::new(false));
        notes_ai::embed::spawn_worker(
            connection.clone(),
            self.embedder.clone(),
            Arc::new(|| {}),
            paused.clone(),
        );
        if self.entity_extraction_enabled {
            notes_ai::extract::spawn_worker(
                connection,
                Arc::new(notes_ai::extract::EntityExtractor::new(
                    self.extraction.clone(),
                )),
                Arc::new(|| {}),
                Arc::new(|| {}),
                paused,
            );
        }
    }

    pub async fn search(
        &self,
        connection: &Connection,
        query: String,
        limit: u32,
    ) -> Result<Vec<SearchHit>> {
        if query.trim().is_empty() || query.chars().count() > 4_096 {
            bail!("query must contain 1 to 4096 characters");
        }
        if !(1..=100).contains(&limit) {
            bail!("limit must be between 1 and 100");
        }
        let embedding = self
            .embedder
            .embed_query(query.clone())
            .await
            .context("embedding search query")?;
        let pool = limit.saturating_mul(4).max(32);
        let candidates = db::search_hybrid(connection, query.clone(), embedding, pool).await?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let documents = candidates
            .iter()
            .map(|hit| {
                format!(
                    "{}\n{}",
                    hit.node.title.as_deref().unwrap_or_default(),
                    hit.node.content
                )
            })
            .collect();
        let ranked = self.reranker.rerank(query, documents).await?;
        Ok(ranked
            .into_iter()
            .take(limit as usize)
            .filter_map(|(index, score)| {
                candidates.get(index).map(|candidate| SearchHit {
                    node: candidate.node.clone(),
                    score: f64::from(score),
                })
            })
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn chat(
        &self,
        connection: Connection,
        history: Vec<ChatTurn>,
        message: String,
        allow_writes: bool,
        active_node_uuid: Option<uuid::Uuid>,
        cancelled: Arc<AtomicBool>,
        emit: impl Fn(ChatEvent) + Send + Sync + 'static,
    ) -> Result<String> {
        let active_node_id = match active_node_uuid {
            Some(uuid) => db::get_node_by_uuid(&connection, uuid)
                .await?
                .map(|node| node.id),
            None => None,
        };
        if cancelled.load(Ordering::Acquire) {
            bail!("chat request was cancelled");
        }
        agent::run_chat_stream_with_config(
            connection,
            self.embedder.clone(),
            self.reranker.clone(),
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
