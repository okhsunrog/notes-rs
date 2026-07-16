use crate::sqlite::Connection;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
#[cfg(feature = "local-models")]
use fastembed::{
    EmbeddingModel as FeModel, InitOptions, RerankInitOptions, RerankerModel, TextEmbedding,
    TextRerank,
};
use rig::client::ProviderClient;
use rig::embeddings::EmbeddingModel;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Reads env vars and constructs the configured embedder.
///
/// `EMBED_PROVIDER` — one of: `openrouter` (default), `local`, `openai`,
///                   `cohere`, `voyageai`, `gemini`. `local` requires building
///                   with `--features local-models`.
/// `EMBED_MODEL`    — provider-specific model id; falls back to a sensible
///                   default per provider.
/// `EMBED_NDIMS`    — ndims override; required for some providers, has model-
///                   specific defaults for the known ones (OpenAI / Qwen).
pub fn make_embedder() -> Result<Arc<dyn EmbedderBackend>> {
    let provider = std::env::var("EMBED_PROVIDER").unwrap_or_else(|_| "openrouter".into());
    if crate::settings::local_only() && !matches!(provider.as_str(), "local" | "fastembed") {
        bail!("AI_LOCAL_ONLY requires EMBED_PROVIDER=local");
    }
    let model_env = std::env::var("EMBED_MODEL").ok();
    let ndims_env = std::env::var("EMBED_NDIMS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    match provider.as_str() {
        "local" | "fastembed" => {
            #[cfg(feature = "local-models")]
            {
                Ok(Arc::new(LocalBgeM3::new()?))
            }
            #[cfg(not(feature = "local-models"))]
            {
                bail!(
                    "EMBED_PROVIDER=local requested but this binary was built without the \
                     'local-models' feature. Rebuild with `cargo build --features local-models`, \
                     or pick a cloud provider (set EMBED_PROVIDER to openrouter/openai/gemini/etc)."
                );
            }
        }
        "openai" => {
            let model = model_env.unwrap_or_else(|| "text-embedding-3-small".into());
            let ndims = ndims_env
                .or_else(|| default_ndims_for_model(&model))
                .with_context(|| format!("EMBED_NDIMS required for OpenAI model {model}"))?;
            let base_url = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into());
            let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
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
        "openrouter" => {
            use rig::providers::openrouter;
            let model = model_env.unwrap_or_else(|| "qwen/qwen3-embedding-8b".into());
            let ndims = ndims_env
                .or_else(|| default_ndims_for_model(&model))
                .with_context(|| {
                    format!(
                        "EMBED_NDIMS required for openrouter model {model}; \
                         set it explicitly (e.g. 4096 for qwen/qwen3-embedding-8b)"
                    )
                })?;
            let client = crate::settings::openrouter_client()?;
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
        "cohere" => {
            use rig::providers::cohere;
            let model = model_env.unwrap_or_else(|| "embed-multilingual-v3.0".into());
            let client = cohere::Client::from_env().context("COHERE_API_KEY not set")?;
            let passages = client.embedding_model(&model, "search_document");
            let query = client.embedding_model(&model, "search_query");
            Ok(Arc::new(AsymmetricRigEmbedder {
                passages,
                query,
                id: format!("cohere:{model}"),
            }))
        }
        "voyageai" => {
            use rig::providers::voyageai;
            let model = model_env.unwrap_or_else(|| "voyage-3-large".into());
            let api_key = std::env::var("VOYAGE_API_KEY").context("VOYAGE_API_KEY not set")?;
            let ndims = ndims_env
                .or_else(|| voyageai::model_dimensions_from_identifier(&model))
                .with_context(|| format!("EMBED_NDIMS required for Voyage model {model}"))?;
            Ok(Arc::new(VoyageEmbedder::new(api_key, model, ndims)?))
        }
        "gemini" => {
            use rig::providers::gemini;
            let model = model_env.unwrap_or_else(|| "gemini-embedding-2".into());
            let client = gemini::Client::from_env().context("GEMINI_API_KEY not set")?;
            let m = <gemini::embedding::EmbeddingModel as EmbeddingModel>::make(
                &client,
                model.clone(),
                ndims_env,
            );
            Ok(Arc::new(RigEmbedder {
                model: m,
                id: format!("gemini:{model}"),
            }))
        }
        other => bail!("unknown EMBED_PROVIDER: {other}"),
    }
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

pub fn make_reranker() -> Result<Arc<dyn RerankBackend>> {
    let provider = std::env::var("RERANK_PROVIDER").unwrap_or_else(|_| "openrouter".into());
    if crate::settings::local_only() && !matches!(provider.as_str(), "local" | "fastembed") {
        bail!("AI_LOCAL_ONLY requires RERANK_PROVIDER=local");
    }
    let model_env = std::env::var("RERANK_MODEL").ok();

    match provider.as_str() {
        "local" | "fastembed" => {
            #[cfg(feature = "local-models")]
            {
                Ok(Arc::new(LocalReranker::new()?))
            }
            #[cfg(not(feature = "local-models"))]
            {
                bail!(
                    "RERANK_PROVIDER=local requires the 'local-models' Cargo feature; \
                     rebuild with `--features local-models` or use a cloud provider."
                );
            }
        }
        "openrouter" => {
            let model = model_env.unwrap_or_else(|| "cohere/rerank-v3.5".into());
            Ok(Arc::new(OpenRouterReranker::new(model)?))
        }
        other => bail!("unknown RERANK_PROVIDER: {other}"),
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
    pub fn new(model: String) -> Result<Self> {
        let api_key = std::env::var("OPENROUTER_API_KEY").context("OPENROUTER_API_KEY not set")?;
        let base = std::env::var("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| crate::settings::DEFAULT_OPENROUTER_BASE_URL.into());
        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            url: format!("{}/rerank", base.trim_end_matches('/')),
        })
    }
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

fn classify_embedding_failure(error: &anyhow::Error) -> (&'static str, bool) {
    for cause in error.chain() {
        if let Some(request) = cause.downcast_ref::<reqwest::Error>() {
            if request.is_timeout() || request.is_connect() {
                return ("network", false);
            }
            if let Some(status) = request.status() {
                return if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                    ("transient", false)
                } else if matches!(status.as_u16(), 401 | 403) {
                    ("auth", true)
                } else {
                    ("provider_request", true)
                };
            }
        }
        if let Some(relay) = cause.downcast_ref::<llm_relay::LlmError>() {
            return match relay {
                llm_relay::LlmError::ApiError { status, .. }
                    if matches!(*status, 408 | 429) || *status >= 500 =>
                {
                    ("transient", false)
                }
                llm_relay::LlmError::ApiError {
                    status: 401 | 403, ..
                } => ("auth", true),
                llm_relay::LlmError::ApiError { .. } => ("provider_request", true),
                llm_relay::LlmError::Config(_)
                | llm_relay::LlmError::Client(_)
                | llm_relay::LlmError::ResponseTooLarge { .. } => ("configuration", true),
                _ => ("provider_response", false),
            };
        }
    }
    ("provider_response", false)
}

pub fn spawn_worker(conn: Connection, embedder: Arc<dyn EmbedderBackend>, paused: Arc<AtomicBool>) {
    tokio::spawn(async move {
        loop {
            if !paused.load(Ordering::Acquire)
                && let Err(e) = tick(&conn, embedder.as_ref()).await
            {
                tracing::warn!(error = ?e, "embed worker tick failed");
            }
            sleep(Duration::from_millis(500)).await;
        }
    });
}

async fn tick(conn: &Connection, embedder: &dyn EmbedderBackend) -> Result<()> {
    let batch = crate::db::take_pending_embeddings(conn, 16).await?;
    if batch.is_empty() {
        return Ok(());
    }
    let original_inputs = batch
        .iter()
        .cloned()
        .collect::<std::collections::HashMap<_, _>>();
    let (ids, texts): (Vec<i64>, Vec<String>) = batch.into_iter().unzip();
    let embs = match embedder.embed_passages(texts).await {
        Ok(embeddings) => embeddings,
        Err(error) => {
            let (kind, terminal) = classify_embedding_failure(&error);
            crate::db::record_embedding_failure(
                conn,
                ids.clone(),
                kind,
                &error.to_string(),
                terminal,
            )
            .await?;
            return Err(error.context("embedding batch failed; retry scheduled with backoff"));
        }
    };
    let valid = embs.len() == ids.len()
        && embs
            .iter()
            .all(|embedding| embedding.len() == embedder.ndims());
    if !valid {
        let error = format!(
            "embedding provider returned {} vectors for {} nodes or an unexpected dimension (expected {})",
            embs.len(),
            ids.len(),
            embedder.ndims()
        );
        crate::db::record_embedding_failure(conn, ids.clone(), "schema", &error, true).await?;
        anyhow::bail!(error);
    }
    // A structural Undo/import can replace nodes while a network request is
    // in flight. Only commit vectors whose current composed input is exactly
    // the text sent to the provider.
    let current_inputs = crate::db::take_pending_embeddings(conn, ids.len().max(16) as u32)
        .await?
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    let items: Vec<(i64, Vec<f32>)> = ids
        .into_iter()
        .zip(embs)
        .filter(|(id, _)| current_inputs.get(id) == original_inputs.get(id))
        .collect();
    crate::db::write_embeddings(conn, items).await?;
    Ok(())
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
