use crate::config::{EmbeddingProvider, RerankProvider};
use crate::store::AiStore;
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
use std::sync::Arc;
#[cfg(feature = "local-models")]
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep};

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
) {
    tokio::spawn(async move {
        loop {
            match store.control().await {
                Ok(control) if control.automatic_embeddings => {
                    match tick(&notes, &store, embedder.as_ref()).await {
                        Ok(true) => on_status_changed(),
                        Ok(false) => {}
                        Err(e) => {
                            on_status_changed();
                            tracing::warn!(error = ?e, "embed worker tick failed");
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(?error, "reading AI worker settings failed"),
            }
            sleep(Duration::from_millis(500)).await;
        }
    });
}

async fn tick(notes: &Connection, store: &AiStore, embedder: &dyn EmbedderBackend) -> Result<bool> {
    let source_seq = notes_core::sync_cursor(notes).await?;
    let source_nodes = store.reconcile(notes, source_seq).await?;
    let jobs = store.take_jobs(16).await?;
    if jobs.is_empty() {
        return Ok(false);
    }
    let texts = jobs
        .iter()
        .map(|job| job.input_text.clone())
        .collect::<Vec<_>>();
    let embs = match embedder.embed_passages(texts).await {
        Ok(embeddings) => embeddings,
        Err(error) => {
            store
                .record_failure(
                    jobs.iter()
                        .map(|job| (job.node_uuid, job.input_hash.clone()))
                        .collect(),
                    &error.to_string(),
                    crate::failure::provider_failure_is_terminal(&error),
                )
                .await?;
            return Err(error.context("embedding batch failed; retry scheduled with backoff"));
        }
    };
    let valid = embs.len() == jobs.len()
        && embs
            .iter()
            .all(|embedding| embedding.len() == embedder.ndims());
    if !valid {
        let error = format!(
            "embedding provider returned {} vectors for {} nodes or an unexpected dimension (expected {})",
            embs.len(),
            jobs.len(),
            embedder.ndims()
        );
        store
            .record_failure(
                jobs.iter()
                    .map(|job| (job.node_uuid, job.input_hash.clone()))
                    .collect(),
                &error,
                true,
            )
            .await?;
        anyhow::bail!(error);
    }
    let items = jobs
        .into_iter()
        .zip(embs)
        .map(|(job, embedding)| (job.node_uuid, job.input_hash, embedding))
        .collect();
    store.write_embeddings(source_nodes, items).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
