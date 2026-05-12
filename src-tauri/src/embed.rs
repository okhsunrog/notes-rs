use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use fastembed::{
    EmbeddingModel as FeModel, InitOptions, RerankInitOptions, RerankerModel, TextEmbedding,
    TextRerank,
};
use rig::client::{EmbeddingsClient, ProviderClient};
use rig::embeddings::EmbeddingModel;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep};
use tokio_rusqlite::Connection;

// ───────────────────────── trait + factory ─────────────────────────

#[async_trait]
pub trait EmbedderBackend: Send + Sync {
    fn ndims(&self) -> usize;
    /// Display name like "local:multilingual-e5-small". Used to detect provider drift.
    fn id(&self) -> String;
    async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;
    async fn embed_query(&self, text: String) -> Result<Vec<f32>>;
}

/// Reads env vars and constructs the configured backend.
///
/// `EMBED_PROVIDER` — one of: `local` (default), `openai`, `openrouter`, `cohere`,
///                   `voyageai`, `gemini`. Embedding-only providers run independently of
///                   the chat provider.
/// `EMBED_MODEL`    — provider-specific model id; falls back to a sensible default.
/// `EMBED_NDIMS`    — ndims override; required for some providers (e.g. OpenAI Matryoshka).
pub fn make_embedder() -> Result<Arc<dyn EmbedderBackend>> {
    let provider = std::env::var("EMBED_PROVIDER").unwrap_or_else(|_| "local".into());
    let model_env = std::env::var("EMBED_MODEL").ok();
    let ndims_env = std::env::var("EMBED_NDIMS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    match provider.as_str() {
        "local" | "fastembed" => Ok(Arc::new(LocalBgeM3::new()?)),
        "openai" => {
            use rig::providers::openai;
            let model = model_env.unwrap_or_else(|| "text-embedding-3-small".into());
            let client = openai::Client::from_env().context("OPENAI_API_KEY not set")?;
            let m = client.embedding_model(&model);
            Ok(Arc::new(RigEmbedder {
                model: m,
                id: format!("openai:{model}"),
            }))
        }
        "openrouter" => {
            use rig::providers::openrouter;
            let model = model_env
                .unwrap_or_else(|| "openai/text-embedding-3-small".into());
            let ndims = ndims_env.context(
                "EMBED_NDIMS required for openrouter (e.g. 1536 for text-embedding-3-small)",
            )?;
            let client = openrouter::Client::from_env()
                .context("OPENROUTER_API_KEY not set")?;
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
            // Cohere requires input_type per call; we use "search_document" for passages.
            let m = client.embedding_model(&model, "search_document");
            Ok(Arc::new(RigEmbedder {
                model: m,
                id: format!("cohere:{model}"),
            }))
        }
        "voyageai" => {
            use rig::providers::voyageai;
            let model = model_env.unwrap_or_else(|| "voyage-3-large".into());
            let client = voyageai::Client::from_env().context("VOYAGE_API_KEY not set")?;
            let m = <voyageai::EmbeddingModel<reqwest::Client> as EmbeddingModel>::make(
                &client,
                model.clone(),
                ndims_env,
            );
            Ok(Arc::new(RigEmbedder {
                model: m,
                id: format!("voyageai:{model}"),
            }))
        }
        "gemini" => {
            use rig::providers::gemini;
            let model = model_env.unwrap_or_else(|| "text-embedding-004".into());
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

// ───────────────────────── local fastembed (BGE-M3) ─────────────────────────

/// 1024-dim dense embeddings, multilingual (100+ languages including Russian
/// and English). BGE-M3 doesn't use the `passage:`/`query:` prefix the E5
/// family needs — texts go in raw on both sides.
pub struct LocalBgeM3 {
    inner: Mutex<TextEmbedding>,
}

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
        let out = self.model.embed_text(&text).await.context("rig embed_text")?;
        Ok(out.vec.into_iter().map(|x| x as f32).collect())
    }
}

// ───────────────────────── reranker (unchanged) ─────────────────────────

pub struct Reranker {
    inner: Mutex<TextRerank>,
}

impl Reranker {
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

    pub async fn rerank(&self, query: String, docs: Vec<String>) -> Result<Vec<(usize, f32)>> {
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

// ───────────────────────── worker ─────────────────────────

pub fn spawn_worker(conn: Connection, embedder: Arc<dyn EmbedderBackend>) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = tick(&conn, embedder.as_ref()).await {
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
    let (ids, texts): (Vec<i64>, Vec<String>) = batch.into_iter().unzip();
    let embs = embedder.embed_passages(texts).await?;
    let items: Vec<(i64, Vec<f32>)> = ids.into_iter().zip(embs).collect();
    crate::db::write_embeddings(conn, items).await?;
    Ok(())
}
