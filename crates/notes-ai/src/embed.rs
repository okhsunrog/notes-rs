use crate::chunking::{TextChunk, split_for_embedding};
use crate::config::{EmbeddingProvider, RerankProvider};
use crate::store::{AiStore, EmbeddedChunk, EmbeddedDocument, IndexJob};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
#[cfg(feature = "local-models")]
use fastembed::{
    EmbeddingModel as FeModel, InitOptions, RerankInitOptions, RerankerModel, TextEmbedding,
    TextRerank,
};
use notes_core::Connection;
use rig::embeddings::EmbeddingModel;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[cfg(feature = "local-models")]
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;

// ───────────────────────── embedder: trait + factory ─────────────────────────

#[async_trait]
pub trait EmbedderBackend: Send + Sync {
    fn ndims(&self) -> usize;
    /// Display id like "openrouter:qwen/qwen3-embedding-8b" or
    /// "local:bge-m3". Used to detect provider drift and re-embed.
    fn id(&self) -> String;
    async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;
    async fn embed_query(&self, text: String) -> Result<Vec<f32>>;
}

#[derive(Debug, Clone)]
pub struct EmbedderConfig {
    pub provider: EmbeddingProvider,
    pub model: String,
    pub ndims: Option<usize>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub local_only: bool,
}

/// Constructs an embedder exclusively from host-owned typed settings.
pub fn make_embedder(config: &EmbedderConfig) -> Result<Arc<dyn EmbedderBackend>> {
    if config.local_only && config.provider != EmbeddingProvider::Local {
        bail!("local-only mode requires the local embedding provider");
    }

    match config.provider {
        EmbeddingProvider::Local => {
            #[cfg(feature = "local-models")]
            {
                Ok(Arc::new(LocalBgeM3::new()?))
            }
            #[cfg(not(feature = "local-models"))]
            {
                bail!(
                    "local embedding requested but this binary was built without the \
                     'local-models' feature. Rebuild with `cargo build --features local-models`, \
                     or pick a cloud provider."
                );
            }
        }
        EmbeddingProvider::Openai => {
            let model = config.model.clone();
            let ndims = config
                .ndims
                .or_else(|| default_ndims_for_model(&model))
                .with_context(|| format!("embedding dimensions are required for {model}"))?;
            let base_url = config
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".into());
            let api_key = config.api_key.clone().unwrap_or_default();
            let mut config =
                llm_relay::EmbeddingsConfig::openai_compatible(base_url, api_key, model.clone())
                    .dimensions(ndims as u32);
            if config.api_key.is_empty() {
                config = config.without_auth();
            }
            Ok(Arc::new(RelayEmbedder {
                client: llm_relay::EmbeddingsClient::new(config)?,
                id: format!("openai:{model}"),
                ndims,
            }))
        }
        EmbeddingProvider::Openrouter => {
            use rig::providers::openrouter;
            let model = config.model.clone();
            let ndims = config
                .ndims
                .or_else(|| default_ndims_for_model(&model))
                .with_context(|| {
                    format!("embedding dimensions are required for OpenRouter model {model}")
                })?;
            let api_key = required_key(config.api_key.as_deref(), "OpenRouter")?;
            let base_url = config
                .base_url
                .as_deref()
                .unwrap_or(crate::config::DEFAULT_OPENROUTER_BASE_URL);
            let client = crate::config::openrouter_client_from(api_key, base_url)?;
            let m = <openrouter::EmbeddingModel as EmbeddingModel>::make(
                &client,
                model.clone(),
                Some(ndims),
            );
            Ok(Arc::new(RigEmbedder {
                model: m,
                id: format!("openrouter:{model}"),
            }))
        }
        EmbeddingProvider::Cohere => {
            use rig::providers::cohere;
            let model = config.model.clone();
            let api_key = required_key(config.api_key.as_deref(), "Cohere")?;
            let client = cohere::Client::new(api_key).context("building Cohere client")?;
            let passages = client.embedding_model(&model, "search_document");
            let query = client.embedding_model(&model, "search_query");
            Ok(Arc::new(AsymmetricRigEmbedder {
                passages,
                query,
                id: format!("cohere:{model}"),
            }))
        }
        EmbeddingProvider::Voyageai => {
            use rig::providers::voyageai;
            let model = config.model.clone();
            let api_key = required_key(config.api_key.as_deref(), "Voyage AI")?;
            let ndims = config
                .ndims
                .or_else(|| voyageai::model_dimensions_from_identifier(&model))
                .with_context(|| format!("embedding dimensions are required for {model}"))?;
            Ok(Arc::new(VoyageEmbedder::new(api_key, model, ndims)?))
        }
        EmbeddingProvider::Gemini => {
            use rig::providers::gemini;
            let model = config.model.clone();
            let api_key = required_key(config.api_key.as_deref(), "Gemini")?;
            let client = gemini::Client::new(api_key).context("building Gemini client")?;
            let m = <gemini::embedding::EmbeddingModel as EmbeddingModel>::make(
                &client,
                model.clone(),
                config.ndims,
            );
            Ok(Arc::new(RigEmbedder {
                model: m,
                id: format!("gemini:{model}"),
            }))
        }
    }
}

fn required_key(value: Option<&str>, provider: &str) -> Result<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .with_context(|| format!("{provider} API key is not configured"))
}

/// Builds an OpenRouter embedder from an explicit host-owned configuration.
/// Server hosts use this instead of mutating process environment variables.
pub fn make_openrouter_embedder(
    api_key: String,
    base_url: &str,
    model: String,
    ndims: usize,
) -> Result<Arc<dyn EmbedderBackend>> {
    make_embedder(&EmbedderConfig {
        provider: EmbeddingProvider::Openrouter,
        model,
        ndims: Some(ndims),
        base_url: Some(base_url.into()),
        api_key: Some(api_key),
        local_only: false,
    })
}

/// Built-in ndims for well-known embedding models so users don't need to
/// guess them. Unknown models still require `EMBED_NDIMS`.
fn default_ndims_for_model(model: &str) -> Option<usize> {
    match model {
        "qwen/qwen3-embedding-8b" => Some(4096),
        "qwen/qwen3-embedding-4b" => Some(2560),
        "qwen/qwen3-embedding-0.6b" => Some(1024),
        "openai/text-embedding-3-small" => Some(1536),
        "openai/text-embedding-3-large" => Some(3072),
        "openai/text-embedding-ada-002" => Some(1536),
        _ => None,
    }
}

// ───────────────────────── local fastembed (BGE-M3) ─────────────────────────

/// 1024-dim dense embeddings, multilingual. Gated behind `local-models`
/// because it pulls fastembed + ort + a ~2.3 GB model download.
#[cfg(feature = "local-models")]
pub struct LocalBgeM3 {
    inner: Mutex<TextEmbedding>,
}

#[cfg(feature = "local-models")]
impl LocalBgeM3 {
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(FeModel::BGEM3).with_show_download_progress(true),
        )
        .context("loading bge-m3 model")?;
        Ok(Self {
            inner: Mutex::new(model),
        })
    }
}

#[cfg(feature = "local-models")]
#[async_trait]
impl EmbedderBackend for LocalBgeM3 {
    fn ndims(&self) -> usize {
        1024
    }
    fn id(&self) -> String {
        "local:bge-m3".into()
    }
    async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let mut model = self.inner.lock().await;
        Ok(model.embed(texts, None)?)
    }
    async fn embed_query(&self, text: String) -> Result<Vec<f32>> {
        let mut model = self.inner.lock().await;
        let mut out = model.embed(vec![text], None)?;
        Ok(out.pop().expect("one embedding"))
    }
}

// ───────────────────────── rig adapter ─────────────────────────

pub struct RigEmbedder<M: EmbeddingModel> {
    model: M,
    id: String,
}

pub struct RelayEmbedder {
    client: llm_relay::EmbeddingsClient,
    id: String,
    ndims: usize,
}

#[async_trait]
impl EmbedderBackend for RelayEmbedder {
    fn ndims(&self) -> usize {
        self.ndims
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.client
            .create_embeddings(&texts)
            .await
            .context("llm-relay passage embeddings")
    }

    async fn embed_query(&self, text: String) -> Result<Vec<f32>> {
        self.client
            .create_embedding(&text)
            .await
            .context("llm-relay query embedding")
    }
}

pub struct AsymmetricRigEmbedder<M: EmbeddingModel> {
    passages: M,
    query: M,
    id: String,
}

#[async_trait]
impl<M> EmbedderBackend for AsymmetricRigEmbedder<M>
where
    M: EmbeddingModel + Send + Sync + 'static,
{
    fn ndims(&self) -> usize {
        self.passages.ndims()
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        embeddings_to_f32(
            self.passages
                .embed_texts(texts)
                .await
                .context("rig embed_texts")?,
        )
    }

    async fn embed_query(&self, text: String) -> Result<Vec<f32>> {
        let embedding = self
            .query
            .embed_text(&text)
            .await
            .context("rig embed_text")?;
        Ok(embedding
            .vec
            .into_iter()
            .map(|value| value as f32)
            .collect())
    }
}

fn embeddings_to_f32(embeddings: Vec<rig::embeddings::Embedding>) -> Result<Vec<Vec<f32>>> {
    Ok(embeddings
        .into_iter()
        .map(|embedding| {
            embedding
                .vec
                .into_iter()
                .map(|value| value as f32)
                .collect()
        })
        .collect())
}

pub struct VoyageEmbedder {
    client: reqwest::Client,
    api_key: String,
    model: String,
    ndims: usize,
}

impl VoyageEmbedder {
    fn new(api_key: String, model: String, ndims: usize) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .context("building Voyage HTTP client")?,
            api_key,
            model,
            ndims,
        })
    }

    async fn embed(&self, texts: Vec<String>, input_type: &str) -> Result<Vec<Vec<f32>>> {
        #[derive(Deserialize)]
        struct Item {
            embedding: Vec<f32>,
            index: usize,
        }
        #[derive(Deserialize)]
        struct Response {
            data: Vec<Item>,
        }

        let expected = texts.len();
        let response = self
            .client
            .post("https://api.voyageai.com/v1/embeddings")
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "input": texts,
                "model": self.model,
                "input_type": input_type,
                "output_dimension": self.ndims,
                "truncation": true,
            }))
            .send()
            .await
            .context("Voyage embedding request failed")?
            .error_for_status()
            .context("Voyage embeddings returned non-2xx")?
            .json::<Response>()
            .await
            .context("decoding Voyage embedding response")?;
        if response.data.len() != expected {
            bail!(
                "Voyage returned {} embeddings for {expected} inputs",
                response.data.len()
            );
        }
        let mut ordered: Vec<Option<Vec<f32>>> = vec![None; expected];
        for item in response.data {
            if item.index >= expected || ordered[item.index].is_some() {
                bail!("Voyage returned an invalid or duplicate embedding index");
            }
            if item.embedding.len() != self.ndims {
                bail!("Voyage returned an unexpected embedding dimension");
            }
            ordered[item.index] = Some(item.embedding);
        }
        ordered
            .into_iter()
            .map(|item| item.context("Voyage omitted an embedding index"))
            .collect()
    }
}

#[async_trait]
impl EmbedderBackend for VoyageEmbedder {
    fn ndims(&self) -> usize {
        self.ndims
    }

    fn id(&self) -> String {
        format!("voyageai:{}", self.model)
    }

    async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.embed(texts, "document").await
    }

    async fn embed_query(&self, text: String) -> Result<Vec<f32>> {
        self.embed(vec![text], "query")
            .await?
            .pop()
            .context("Voyage returned no query embedding")
    }
}

#[async_trait]
impl<M> EmbedderBackend for RigEmbedder<M>
where
    M: EmbeddingModel + Send + Sync + 'static,
{
    fn ndims(&self) -> usize {
        self.model.ndims()
    }
    fn id(&self) -> String {
        self.id.clone()
    }
    async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let out = self
            .model
            .embed_texts(texts)
            .await
            .context("rig embed_texts")?;
        Ok(out
            .into_iter()
            .map(|e| e.vec.into_iter().map(|x| x as f32).collect())
            .collect())
    }
    async fn embed_query(&self, text: String) -> Result<Vec<f32>> {
        let out = self
            .model
            .embed_text(&text)
            .await
            .context("rig embed_text")?;
        Ok(out.vec.into_iter().map(|x| x as f32).collect())
    }
}

// ───────────────────────── reranker: trait + factory ─────────────────────────

#[async_trait]
pub trait RerankBackend: Send + Sync {
    /// Returns `(original_index, score)` pairs sorted by score descending.
    async fn rerank(&self, query: String, docs: Vec<String>) -> Result<Vec<(usize, f32)>>;
}

#[derive(Debug, Clone)]
pub struct RerankerConfig {
    pub provider: RerankProvider,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub local_only: bool,
}

pub fn make_reranker(config: &RerankerConfig) -> Result<Arc<dyn RerankBackend>> {
    if config.local_only && config.provider != RerankProvider::Local {
        bail!("local-only mode requires the local reranking provider");
    }

    match config.provider {
        RerankProvider::Local => {
            #[cfg(feature = "local-models")]
            {
                Ok(Arc::new(LocalReranker::new()?))
            }
            #[cfg(not(feature = "local-models"))]
            {
                bail!(
                    "local reranking requires the 'local-models' Cargo feature; \
                     rebuild with `--features local-models` or use a cloud provider."
                );
            }
        }
        RerankProvider::Openrouter => {
            let api_key = required_key(config.api_key.as_deref(), "OpenRouter")?;
            let base_url = config
                .base_url
                .as_deref()
                .unwrap_or(crate::config::DEFAULT_OPENROUTER_BASE_URL);
            Ok(Arc::new(OpenRouterReranker::from_config(
                api_key,
                base_url,
                config.model.clone(),
            )?))
        }
    }
}

// ───────────────────────── local reranker (fastembed BGE-Reranker-v2-m3) ─────────────────────────

#[cfg(feature = "local-models")]
pub struct LocalReranker {
    inner: Mutex<TextRerank>,
}

#[cfg(feature = "local-models")]
impl LocalReranker {
    pub fn new() -> Result<Self> {
        let model = TextRerank::try_new(
            RerankInitOptions::new(RerankerModel::BGERerankerV2M3)
                .with_show_download_progress(true),
        )
        .context("loading bge-reranker-v2-m3 model")?;
        Ok(Self {
            inner: Mutex::new(model),
        })
    }
}

#[cfg(feature = "local-models")]
#[async_trait]
impl RerankBackend for LocalReranker {
    async fn rerank(&self, query: String, docs: Vec<String>) -> Result<Vec<(usize, f32)>> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let mut model = self.inner.lock().await;
        let out = model.rerank(query, &docs, false, None)?;
        let mut scored: Vec<(usize, f32)> = out.into_iter().map(|r| (r.index, r.score)).collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        Ok(scored)
    }
}

// ───────────────────────── OpenRouter reranker ─────────────────────────

/// Calls OpenRouter's `/v1/rerank` endpoint directly via reqwest. rig doesn't
/// expose a rerank trait, and OpenRouter proxies Cohere's rerank API shape,
/// so this is a thin HTTP client wrapping that contract.
pub struct OpenRouterReranker {
    client: reqwest::Client,
    api_key: String,
    model: String,
    url: String,
}

impl OpenRouterReranker {
    pub fn from_config(api_key: String, base_url: &str, model: String) -> Result<Self> {
        if api_key.trim().is_empty() {
            bail!("OpenRouter API key cannot be empty");
        }
        crate::config::validate_http_base_url(base_url)?;
        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            url: format!("{}/rerank", base_url.trim_end_matches('/')),
        })
    }
}

pub fn make_openrouter_reranker(
    api_key: String,
    base_url: &str,
    model: String,
) -> Result<Arc<dyn RerankBackend>> {
    make_reranker(&RerankerConfig {
        provider: RerankProvider::Openrouter,
        model,
        base_url: Some(base_url.into()),
        api_key: Some(api_key),
        local_only: false,
    })
}

#[derive(Debug, Deserialize)]
struct CohereRerankResult {
    index: usize,
    relevance_score: f32,
}

#[derive(Deserialize)]
struct CohereRerankResponse {
    results: Vec<CohereRerankResult>,
}

fn validate_rerank_results(
    results: Vec<CohereRerankResult>,
    document_count: usize,
) -> Result<Vec<(usize, f32)>> {
    let mut seen = std::collections::HashSet::new();
    let mut scored = Vec::with_capacity(results.len());
    for result in results {
        if result.index >= document_count {
            bail!(
                "reranker returned index {} for {document_count} documents",
                result.index
            );
        }
        if !result.relevance_score.is_finite() {
            bail!("reranker returned a non-finite score");
        }
        if !seen.insert(result.index) {
            bail!("reranker returned duplicate index {}", result.index);
        }
        scored.push((result.index, result.relevance_score));
    }
    scored.sort_by(|left, right| right.1.total_cmp(&left.1));
    Ok(scored)
}

#[async_trait]
impl RerankBackend for OpenRouterReranker {
    async fn rerank(&self, query: String, docs: Vec<String>) -> Result<Vec<(usize, f32)>> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let body = serde_json::json!({
            "model": self.model,
            "query": query,
            "documents": docs,
        });
        let resp = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("openrouter rerank request failed")?
            .error_for_status()
            .context("openrouter rerank returned non-2xx")?;
        let parsed: CohereRerankResponse = resp
            .json()
            .await
            .context("decoding openrouter rerank response")?;
        validate_rerank_results(parsed.results, docs.len())
    }
}

// ───────────────────────── worker ─────────────────────────

pub fn spawn_worker(
    notes: Connection,
    store: Arc<AiStore>,
    embedder: Arc<dyn EmbedderBackend>,
    on_status_changed: Arc<dyn Fn() + Send + Sync>,
    wake: Arc<Notify>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let did_work = match store.control().await {
                Ok(control) if control.automatic_embeddings => {
                    let result = tokio::select! {
                        () = shutdown.cancelled() => return,
                        result = tick(&notes, &store, embedder.as_ref()) => result,
                    };
                    match result {
                        Ok(true) => {
                            on_status_changed();
                            true
                        }
                        Ok(false) => false,
                        Err(e) => {
                            on_status_changed();
                            tracing::warn!(error = ?e, "embed worker tick failed");
                            false
                        }
                    }
                }
                Ok(_) => false,
                Err(error) => {
                    tracing::warn!(?error, "reading AI worker settings failed");
                    false
                }
            };
            if did_work {
                continue;
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = wake.notified() => {},
                () = sleep(Duration::from_secs(30)) => {},
            }
        }
    });
}

async fn tick(notes: &Connection, store: &AiStore, embedder: &dyn EmbedderBackend) -> Result<bool> {
    store.reconcile(notes).await?;
    let jobs = store.take_jobs(16).await?;
    if jobs.is_empty() {
        return Ok(false);
    }
    let prepared = jobs
        .iter()
        .enumerate()
        .flat_map(|(job_index, job)| {
            split_for_embedding(&job.input_text, &job.input_header)
                .into_iter()
                .map(move |chunk| PreparedChunk { job_index, chunk })
        })
        .collect::<Vec<_>>();
    let texts = prepared
        .iter()
        .map(|prepared| prepared.chunk.text.clone())
        .collect::<Vec<_>>();
    let items = match embedder.embed_passages(texts.clone()).await {
        Ok(embeddings) => {
            match validate_embedding_response(embeddings, prepared.len(), embedder.ndims()) {
                Ok(embeddings) => {
                    assemble_embedded_documents(&jobs, &prepared, embeddings, &HashSet::new())
                }
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        "embedding batch response was invalid; retrying per item"
                    );
                    embed_individually(store, embedder, &jobs, &prepared, texts).await?
                }
            }
        }
        Err(error) => {
            let terminal = crate::failure::provider_failure_is_terminal(&error);
            if terminal {
                tracing::warn!(?error, "embedding batch failed; retrying per item");
                embed_individually(store, embedder, &jobs, &prepared, texts).await?
            } else {
                tracing::warn!(?error, "embedding batch failed; deferring the batch retry");
                store
                    .record_failure(
                        jobs.iter()
                            .map(|job| (job.content_uuid, job.input_hash.clone()))
                            .collect(),
                        &error.to_string(),
                        false,
                    )
                    .await?;
                Vec::new()
            }
        }
    };
    // The provider call above can be slow enough for source content to change while it is in
    // flight. Reconciliation atomically replaces those jobs with their new input hashes, so
    // `write_embeddings` will ignore stale results instead of removing the newer work item.
    let source_documents = store.reconcile(notes).await?;
    store.write_embeddings(source_documents, items).await?;
    Ok(true)
}

struct PreparedChunk {
    job_index: usize,
    chunk: TextChunk,
}

fn validate_embedding_response(
    embeddings: Vec<Vec<f32>>,
    expected_count: usize,
    expected_dimensions: usize,
) -> Result<Vec<Vec<f32>>> {
    if embeddings.len() != expected_count {
        bail!(
            "embedding provider returned {} vectors for {expected_count} documents",
            embeddings.len()
        );
    }
    if let Some((index, dimensions)) =
        embeddings
            .iter()
            .enumerate()
            .find_map(|(index, embedding)| {
                (embedding.len() != expected_dimensions).then_some((index, embedding.len()))
            })
    {
        bail!(
            "embedding provider returned {dimensions} dimensions for item {index}, expected {expected_dimensions}"
        );
    }
    Ok(embeddings)
}

async fn embed_individually(
    store: &AiStore,
    embedder: &dyn EmbedderBackend,
    jobs: &[IndexJob],
    prepared: &[PreparedChunk],
    texts: Vec<String>,
) -> Result<Vec<EmbeddedDocument>> {
    let mut embeddings = Vec::with_capacity(prepared.len());
    let mut failures = HashMap::<usize, (String, bool)>::new();
    for (prepared_chunk, text) in prepared.iter().zip(texts) {
        if failures.contains_key(&prepared_chunk.job_index) {
            embeddings.push(Vec::new());
            continue;
        }
        let job = &jobs[prepared_chunk.job_index];
        let result = match embedder.embed_passages(vec![text]).await {
            Ok(embeddings) => validate_embedding_response(embeddings, 1, embedder.ndims())
                .map(|mut embeddings| embeddings.remove(0))
                .map_err(|error| (error, true)),
            Err(error) => {
                let terminal = crate::failure::provider_failure_is_terminal(&error);
                Err((error, terminal))
            }
        };
        match result {
            Ok(embedding) => {
                embeddings.push(embedding);
            }
            Err((error, terminal)) => {
                tracing::warn!(
                    content_uuid = %job.content_uuid,
                    ?error,
                    terminal,
                    "individual embedding failed"
                );
                let failure = failures
                    .entry(prepared_chunk.job_index)
                    .or_insert_with(|| (error.to_string(), terminal));
                failure.1 |= terminal;
                embeddings.push(Vec::new());
            }
        }
    }
    for (job_index, (error, terminal)) in &failures {
        let job = &jobs[*job_index];
        store
            .record_failure(
                vec![(job.content_uuid, job.input_hash.clone())],
                error,
                *terminal,
            )
            .await?;
    }
    Ok(assemble_embedded_documents(
        jobs,
        prepared,
        embeddings,
        &failures.keys().copied().collect(),
    ))
}

fn assemble_embedded_documents(
    jobs: &[IndexJob],
    prepared: &[PreparedChunk],
    embeddings: Vec<Vec<f32>>,
    failed_jobs: &HashSet<usize>,
) -> Vec<EmbeddedDocument> {
    let mut chunks = (0..jobs.len()).map(|_| Vec::new()).collect::<Vec<_>>();
    for (prepared_chunk, embedding) in prepared.iter().zip(embeddings) {
        if failed_jobs.contains(&prepared_chunk.job_index) {
            continue;
        }
        chunks[prepared_chunk.job_index].push(EmbeddedChunk {
            chunk_index: prepared_chunk.chunk.chunk_index,
            input_text: prepared_chunk.chunk.text.clone(),
            embedding,
        });
    }
    jobs.iter()
        .enumerate()
        .filter(|(index, _)| !failed_jobs.contains(index))
        .map(|(index, job)| EmbeddedDocument {
            content_uuid: job.content_uuid,
            input_hash: job.input_hash.clone(),
            chunks: std::mem::take(&mut chunks[index]),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    struct MutatingEmbedder {
        notes: Connection,
        block_uuid: uuid::Uuid,
    }

    #[derive(Default)]
    struct PoisonBatchEmbedder {
        call_sizes: StdMutex<Vec<usize>>,
    }

    #[derive(Default)]
    struct TransientBatchEmbedder {
        call_sizes: StdMutex<Vec<usize>>,
    }

    struct UniformEmbedder;

    #[async_trait]
    impl EmbedderBackend for UniformEmbedder {
        fn ndims(&self) -> usize {
            2
        }

        fn id(&self) -> String {
            "test:uniform".into()
        }

        async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
            Ok(texts.into_iter().map(|_| vec![1.0, 0.0]).collect())
        }

        async fn embed_query(&self, _text: String) -> Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }
    }

    #[async_trait]
    impl EmbedderBackend for PoisonBatchEmbedder {
        fn ndims(&self) -> usize {
            2
        }

        fn id(&self) -> String {
            "test:poison-batch".into()
        }

        async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
            self.call_sizes.lock().unwrap().push(texts.len());
            if texts.len() > 1 {
                return Err(llm_relay::LlmError::ApiError {
                    status: 400,
                    body: "simulated batch rejection".into(),
                }
                .into());
            }
            if texts[0].contains("POISON") {
                bail!("simulated poison input rejection");
            }
            Ok(vec![vec![1.0, 0.0]])
        }

        async fn embed_query(&self, _text: String) -> Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }
    }

    #[async_trait]
    impl EmbedderBackend for TransientBatchEmbedder {
        fn ndims(&self) -> usize {
            2
        }

        fn id(&self) -> String {
            "test:transient-batch".into()
        }

        async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
            self.call_sizes.lock().unwrap().push(texts.len());
            Err(llm_relay::LlmError::ApiError {
                status: 429,
                body: "simulated rate limit".into(),
            }
            .into())
        }

        async fn embed_query(&self, _text: String) -> Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }
    }

    #[async_trait]
    impl EmbedderBackend for MutatingEmbedder {
        fn ndims(&self) -> usize {
            2
        }

        fn id(&self) -> String {
            "test:mutating".into()
        }

        async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
            notes_core::db::set_block_content(
                &self.notes,
                self.block_uuid,
                notes_core::db::BlockContent {
                    markdown: "new content written while embedding".into(),
                },
            )
            .await?;
            Ok(texts.into_iter().map(|_| vec![1.0, 0.0]).collect())
        }

        async fn embed_query(&self, _text: String) -> Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }
    }

    #[test]
    fn rejects_malformed_rerank_indices_and_scores() {
        assert!(
            validate_rerank_results(
                vec![CohereRerankResult {
                    index: 2,
                    relevance_score: 0.9,
                }],
                2,
            )
            .is_err()
        );
        assert!(
            validate_rerank_results(
                vec![
                    CohereRerankResult {
                        index: 0,
                        relevance_score: 0.9,
                    },
                    CohereRerankResult {
                        index: 0,
                        relevance_score: 0.8,
                    },
                ],
                2,
            )
            .is_err()
        );
        assert!(
            validate_rerank_results(
                vec![CohereRerankResult {
                    index: 0,
                    relevance_score: f32::NAN,
                }],
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn sorts_valid_rerank_results_by_score() {
        let results = validate_rerank_results(
            vec![
                CohereRerankResult {
                    index: 0,
                    relevance_score: 0.2,
                },
                CohereRerankResult {
                    index: 1,
                    relevance_score: 0.8,
                },
            ],
            2,
        )
        .expect("valid results");
        assert_eq!(results, vec![(1, 0.8), (0, 0.2)]);
    }

    #[test]
    fn oversized_provider_input_is_utf8_safe_capped_and_marked() {
        let input = format!(
            "prefix {}",
            "Ж".repeat(crate::chunking::MAX_PROVIDER_INPUT_CHARS + 100)
        );
        let capped = crate::chunking::cap_provider_input(&input);

        assert_eq!(
            capped.chars().count(),
            crate::chunking::MAX_PROVIDER_INPUT_CHARS
        );
        assert!(capped.ends_with(crate::chunking::PROVIDER_TRUNCATION_MARKER));
        assert!(capped.starts_with("prefix Ж"));
    }

    #[tokio::test]
    async fn failed_batch_retries_items_and_isolates_the_poison_input() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Batch".into())
            .await
            .expect("create page");
        let mut previous = None;
        for index in 0..15 {
            let markdown = if index == 7 {
                "POISON".into()
            } else {
                format!("healthy document {index} {}", "x".repeat(150))
            };
            let block = notes_core::db::create_block(
                &notes,
                page.uuid,
                None,
                previous,
                notes_core::BlockStyle::Paragraph,
                markdown,
            )
            .await
            .expect("create block");
            previous = Some(block.uuid);
        }
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");
        let embedder = PoisonBatchEmbedder::default();

        assert!(
            tick(&notes, &store, &embedder)
                .await
                .expect("embedding tick")
        );

        let status = store.status(16).await.expect("AI status");
        assert_eq!(status.indexed, 15);
        assert_eq!(status.pending, 1);
        assert_eq!(status.failed, 1);
        let calls = embedder.call_sizes.lock().unwrap().clone();
        assert_eq!(calls.first(), Some(&16));
        assert_eq!(calls.len(), 17);
        assert!(calls[1..].iter().all(|size| *size == 1));
    }

    #[tokio::test]
    async fn transient_batch_failure_does_not_fan_out_to_individual_requests() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Rate limited".into())
            .await
            .expect("create page");
        notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            notes_core::BlockStyle::Paragraph,
            "one document".into(),
        )
        .await
        .expect("create block");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");
        let embedder = TransientBatchEmbedder::default();

        assert!(
            tick(&notes, &store, &embedder)
                .await
                .expect("embedding tick")
        );

        assert_eq!(*embedder.call_sizes.lock().unwrap(), vec![2]);
        let status = store.status(2).await.expect("AI status");
        assert_eq!(status.indexed, 0);
        assert_eq!(status.pending, 2);
        assert_eq!(status.failed, 2);
    }

    #[tokio::test]
    async fn individual_retry_stops_after_a_chunk_failure_for_the_same_job() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Chunk failure".into())
            .await
            .expect("create page");
        let markdown = format!("{} POISON\n{}", "x".repeat(1_100), "y".repeat(2_000));
        notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            notes_core::BlockStyle::Paragraph,
            markdown,
        )
        .await
        .expect("create block");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");
        let embedder = PoisonBatchEmbedder::default();

        assert!(
            tick(&notes, &store, &embedder)
                .await
                .expect("embedding tick")
        );

        let calls = embedder.call_sizes.lock().unwrap().clone();
        assert!(calls[0] > 2, "the block must be split into multiple chunks");
        assert_eq!(calls[1..], [1, 1]);
        let status = store.status(2).await.expect("AI status");
        assert_eq!(status.indexed, 1);
        assert_eq!(status.pending, 1);
        assert_eq!(status.failed, 1);
    }

    #[tokio::test]
    async fn consecutive_ticks_without_oplog_movement_reconcile_once() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        notes_core::db::create_page(&notes, "Stable page".into())
            .await
            .expect("create page");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");

        assert!(
            tick(&notes, &store, &UniformEmbedder)
                .await
                .expect("first tick")
        );
        assert_eq!(store.reconcile_scan_count(), 1);
        assert!(
            !tick(&notes, &store, &UniformEmbedder)
                .await
                .expect("second tick")
        );
        assert_eq!(store.reconcile_scan_count(), 1);
    }

    #[tokio::test]
    async fn tier_two_document_round_trips_through_reconcile_embed_and_query() {
        use crate::store::VectorStore;

        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Large table".into())
            .await
            .expect("create page");
        let markdown = (0..300)
            .map(|index| format!("| row {index:03} | value {index:03} |"))
            .collect::<Vec<_>>()
            .join("\n");
        let block = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            notes_core::BlockStyle::Paragraph,
            markdown,
        )
        .await
        .expect("create long block");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");

        assert!(
            tick(&notes, &store, &UniformEmbedder)
                .await
                .expect("embedding tick")
        );

        let status = store.status(2).await.expect("AI status");
        assert_eq!(
            status.indexed, 2,
            "indexed counts retrieval units, not chunks"
        );
        assert_eq!(
            status.generation_status,
            crate::store::GenerationStatus::Active
        );
        let matches = store.search(vec![1.0, 0.0], 32).await.expect("KNN query");
        let block_matches = matches
            .iter()
            .filter(|matched| matched.content_uuid == block.uuid)
            .collect::<Vec<_>>();
        assert!(block_matches.len() > 1);
        assert!(
            block_matches
                .iter()
                .all(|matched| matched.chunk_text.starts_with("Large table\n"))
        );
        assert!(
            matches
                .iter()
                .any(|matched| matched.content_uuid == page.uuid)
        );
    }

    #[tokio::test]
    async fn tick_does_not_commit_an_embedding_for_content_changed_in_flight() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Race".into())
            .await
            .expect("create page");
        let block = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            notes_core::BlockStyle::Paragraph,
            "old content".into(),
        )
        .await
        .expect("create block");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");
        let embedder = MutatingEmbedder {
            notes: notes.clone(),
            block_uuid: block.uuid,
        };

        assert!(
            tick(&notes, &store, &embedder)
                .await
                .expect("embedding tick")
        );

        let status = store.status(2).await.expect("AI status");
        assert_eq!(status.indexed, 0);
        assert_eq!(status.pending, 2);
        let current = store
            .take_jobs(16)
            .await
            .expect("current jobs")
            .into_iter()
            .find(|job| job.content_uuid == block.uuid)
            .expect("current block job");
        assert_eq!(current.content_uuid, block.uuid);
        assert!(
            current
                .input_text
                .contains("new content written while embedding")
        );
    }
}
