use crate::config::AiConfig;
use crate::state::{UserRegistry, UserState};
use anyhow::{Context, Result, bail};
use llm_relay::{ChatOptions, LlmClient};
use notes_ai::agent;
use notes_ai::embed::{EmbedderBackend, RerankBackend};
use notes_ai::retrieval::RetrievalPipeline;
use notes_ai::store::{AiControl, AiStore, GenerationStatus, embedding_identity_fingerprint};
use notes_core::Connection;
use notes_core::db::{self, SearchHit};
use notes_protocol::{
    AiGenerationState, AiIndexStatus, AiProbeCheck, AiProviderProbeResult,
    AiProviderSettingsUpdate, AiRuntimeSettings, ChatEvent, ChatTurn,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

const SETTINGS_FILE: &str = "ai-settings.json";

#[derive(Clone)]
struct UserAi {
    source: Arc<UserState>,
    notes: Connection,
    store: Arc<AiStore>,
    retrieval: RetrievalPipeline,
    embed_wake: Arc<Notify>,
    extract_wake: Arc<Notify>,
}

struct ProviderClients {
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<dyn RerankBackend>,
    chat: llm_relay::ClientConfig,
    extraction: llm_relay::ClientConfig,
}

struct ActiveAi {
    config: AiConfig,
    users: HashMap<String, UserAi>,
    embedder: Arc<dyn EmbedderBackend>,
    chat: llm_relay::ClientConfig,
    extraction: llm_relay::ClientConfig,
    shutdown: CancellationToken,
}

pub struct AiRuntime {
    registry: UserRegistry,
    data_dir: PathBuf,
    settings_path: PathBuf,
    shutdown: CancellationToken,
    active: RwLock<Arc<ActiveAi>>,
    reconfigure: Mutex<()>,
}

impl AiRuntime {
    pub async fn open(
        bootstrap: &AiConfig,
        registry: &UserRegistry,
        data_dir: &Path,
        shutdown: CancellationToken,
    ) -> Result<Self> {
        let settings_path = data_dir.join(SETTINGS_FILE);
        let config = load_or_initialize_config(&settings_path, bootstrap)?;
        let active = build_active(config, registry, data_dir, shutdown.child_token()).await?;
        active.spawn_workers();
        Ok(Self {
            registry: registry.clone(),
            data_dir: data_dir.to_path_buf(),
            settings_path,
            shutdown,
            active: RwLock::new(active),
            reconfigure: Mutex::new(()),
        })
    }

    fn active(&self) -> Arc<ActiveAi> {
        self.active
            .read()
            .expect("AI runtime lock poisoned")
            .clone()
    }

    pub async fn search(&self, user_id: &str, query: String, limit: u32) -> Result<Vec<SearchHit>> {
        if query.trim().is_empty() || query.chars().count() > 4_096 {
            bail!("query must contain 1 to 4096 characters");
        }
        if !(1..=100).contains(&limit) {
            bail!("limit must be between 1 and 100");
        }
        let active = self.active();
        active.user(user_id)?.retrieval.retrieve(query, limit).await
    }

    pub async fn status(&self, user_id: &str) -> Result<AiIndexStatus> {
        let active = self.active();
        status_for(&active, user_id).await
    }

    pub async fn update_settings(
        &self,
        user_id: &str,
        settings: AiRuntimeSettings,
    ) -> Result<AiIndexStatus> {
        let active = self.active();
        let user = active.user(user_id)?;
        user.store
            .set_control(AiControl {
                automatic_embeddings: settings.automatic_embeddings,
                entity_extraction: settings.entity_extraction,
                query_rewriting: settings.query_rewriting,
            })
            .await?;
        user.embed_wake.notify_one();
        user.extract_wake.notify_one();
        status_for(&active, user_id).await
    }

    pub async fn update_provider(
        &self,
        user_id: &str,
        update: AiProviderSettingsUpdate,
    ) -> Result<AiIndexStatus> {
        let _guard = self.reconfigure.lock().await;
        let next_config = self.active().config.applying(update);
        next_config.validate()?;
        let next = build_active(
            next_config.clone(),
            &self.registry,
            &self.data_dir,
            self.shutdown.child_token(),
        )
        .await?;
        persist_config(&self.settings_path, &next_config)?;
        let previous = {
            let mut active = self.active.write().expect("AI runtime lock poisoned");
            std::mem::replace(&mut *active, next.clone())
        };
        previous.shutdown.cancel();
        next.spawn_workers();
        status_for(&next, user_id).await
    }

    pub fn validate_provider_update(&self, update: &AiProviderSettingsUpdate) -> Result<()> {
        let candidate = self.active().config.applying(update.clone());
        candidate.validate()?;
        build_provider_clients(&candidate)?;
        Ok(())
    }

    pub async fn probe_provider(
        &self,
        update: AiProviderSettingsUpdate,
    ) -> Result<AiProviderProbeResult> {
        let candidate = self.active().config.applying(update);
        candidate.validate()?;
        let clients = build_provider_clients(&candidate)?;

        let embeddings = match clients
            .embedder
            .embed_query("notes-rs provider probe".into())
            .await
        {
            Ok(vector) if vector.len() == candidate.embedding_dimensions => {
                probe_ok(format!("returned {} dimensions", vector.len()))
            }
            Ok(vector) => probe_error(format!(
                "returned {} dimensions, expected {}",
                vector.len(),
                candidate.embedding_dimensions
            )),
            Err(error) => probe_error(format!("{error:#}")),
        };
        let reranking = match clients
            .reranker
            .rerank(
                "notes-rs provider probe".into(),
                vec!["relevant note".into(), "unrelated note".into()],
            )
            .await
        {
            Ok(scores) if !scores.is_empty() => probe_ok("returned ranked documents"),
            Ok(_) => probe_error("returned no ranked documents"),
            Err(error) => probe_error(format!("{error:#}")),
        };
        let chat = probe_completion(clients.chat, "chat").await;
        let extraction = probe_completion(clients.extraction, "extraction").await;
        Ok(AiProviderProbeResult {
            embeddings,
            reranking,
            chat,
            extraction,
        })
    }

    pub async fn reindex(&self, user_id: &str) -> Result<AiIndexStatus> {
        let active = self.active();
        let user = active.user(user_id)?;
        user.store.reset_index().await?;
        user.embed_wake.notify_one();
        user.extract_wake.notify_one();
        status_for(&active, user_id).await
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
        let active = self.active();
        let user = active.user(user_id)?;
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
            active.chat.clone(),
            query_rewriting,
        )
        .await
        .map_err(anyhow::Error::from)
    }
}

impl ActiveAi {
    fn user(&self, user_id: &str) -> Result<&UserAi> {
        self.users
            .get(user_id)
            .with_context(|| format!("AI runtime is unavailable for user {user_id}"))
    }

    fn spawn_workers(self: &Arc<Self>) {
        for user in self.users.values() {
            notes_ai::embed::spawn_worker(
                user.notes.clone(),
                user.store.clone(),
                self.embedder.clone(),
                Arc::new(|| {}),
                user.embed_wake.clone(),
                self.shutdown.child_token(),
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
                self.shutdown.child_token(),
            );
            let mut changes = user.source.subscribe();
            let embed_wake = user.embed_wake.clone();
            let extract_wake = user.extract_wake.clone();
            let shutdown = self.shutdown.child_token();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = shutdown.cancelled() => return,
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
    }
}

async fn build_active(
    config: AiConfig,
    registry: &UserRegistry,
    data_dir: &Path,
    shutdown: CancellationToken,
) -> Result<Arc<ActiveAi>> {
    config.validate()?;
    let clients = build_provider_clients(&config)?;
    let identity = embedding_identity_fingerprint(
        &config.retrieval_base_url,
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
            clients.embedder.clone(),
            clients.reranker.clone(),
        );
        users.insert(
            user.id.clone(),
            UserAi {
                source: user.clone(),
                notes: user.notes.clone(),
                store,
                retrieval,
                embed_wake,
                extract_wake,
            },
        );
    }
    Ok(Arc::new(ActiveAi {
        config,
        users,
        embedder: clients.embedder,
        chat: clients.chat,
        extraction: clients.extraction,
        shutdown,
    }))
}

fn build_provider_clients(config: &AiConfig) -> Result<ProviderClients> {
    let embedder = notes_ai::embed::make_openrouter_embedder(
        config.retrieval_api_key.clone(),
        &config.retrieval_base_url,
        config.embedding_model.clone(),
        config.embedding_dimensions,
    )?;
    let reranker = notes_ai::embed::make_openrouter_reranker(
        config.retrieval_api_key.clone(),
        &config.retrieval_base_url,
        config.rerank_model.clone(),
    )?;
    let chat = notes_ai::config::completion_config(
        config.completion_protocol,
        Some(config.completion_base_url.clone()),
        Some(config.completion_api_key.clone()),
        config.chat_model.clone(),
    )?;
    let extraction = notes_ai::config::completion_config(
        config.completion_protocol,
        Some(config.completion_base_url.clone()),
        Some(config.completion_api_key.clone()),
        config.extraction_model.clone(),
    )?;
    Ok(ProviderClients {
        embedder,
        reranker,
        chat,
        extraction,
    })
}

async fn status_for(active: &ActiveAi, user_id: &str) -> Result<AiIndexStatus> {
    let user = active.user(user_id)?;
    let source_nodes = user.store.source_node_count(&user.notes).await?;
    let status = user.store.status(source_nodes).await?;
    Ok(AiIndexStatus {
        provider: active.config.public_settings(),
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

fn load_or_initialize_config(path: &Path, bootstrap: &AiConfig) -> Result<AiConfig> {
    if path.exists() {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading AI settings {}", path.display()))?;
        let config: AiConfig = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing AI settings {}", path.display()))?;
        config.validate()?;
        return Ok(config);
    }
    bootstrap.validate()?;
    persist_config(path, bootstrap)?;
    Ok(bootstrap.clone())
}

fn persist_config(path: &Path, config: &AiConfig) -> Result<()> {
    let parent = path.parent().context("AI settings path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{SETTINGS_FILE}.{}.tmp", uuid::Uuid::now_v7()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(config)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}

async fn probe_completion(config: llm_relay::ClientConfig, label: &str) -> AiProbeCheck {
    let result = async {
        let client = LlmClient::new(config.max_tokens(16))?;
        let response = client
            .complete(
                "Reply with exactly OK.",
                ChatOptions {
                    temperature: Some(0.0),
                    ..ChatOptions::default()
                },
            )
            .await?;
        if response.text().trim().is_empty() {
            bail!("provider returned an empty response");
        }
        Result::<()>::Ok(())
    }
    .await;
    match result {
        Ok(()) => probe_ok(format!("{label} model responded")),
        Err(error) => probe_error(format!("{error:#}")),
    }
}

fn probe_ok(message: impl Into<String>) -> AiProbeCheck {
    AiProbeCheck {
        ok: true,
        message: message.into(),
    }
}

fn probe_error(message: impl Into<String>) -> AiProbeCheck {
    AiProbeCheck {
        ok: false,
        message: message.into(),
    }
}
