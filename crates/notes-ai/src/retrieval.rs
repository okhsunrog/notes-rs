use crate::embed::{EmbedderBackend, RerankBackend};
use crate::store::VectorStore;
use anyhow::{Result, bail};
use notes_core::Connection;
use notes_core::db::{self, Content, SearchHit};
use std::collections::HashMap;
use std::sync::Arc;

const RRF_K: f64 = 60.0;
pub const RERANK_POOL_MIN: u32 = 32;
pub const RELEVANCE_FLOOR: f64 = 0.30;
const LOW_CONFIDENCE_FALLBACK_LIMIT: usize = 4;

#[derive(Clone)]
pub struct RetrievalPipeline {
    notes: Connection,
    vectors: Arc<dyn VectorStore>,
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<dyn RerankBackend>,
}

impl RetrievalPipeline {
    pub fn new(
        notes: Connection,
        vectors: Arc<dyn VectorStore>,
        embedder: Arc<dyn EmbedderBackend>,
        reranker: Arc<dyn RerankBackend>,
    ) -> Self {
        Self {
            notes,
            vectors,
            embedder,
            reranker,
        }
    }

    pub async fn hybrid_candidates(&self, query: String, limit: u32) -> Result<Vec<SearchHit>> {
        validate(&query, limit)?;
        let embedding = self.embedder.embed_query(query.clone()).await?;
        let (fts, vectors) = tokio::try_join!(
            db::search_fts(&self.notes, query, limit.saturating_mul(4)),
            self.vectors.search(embedding, limit.saturating_mul(4))
        )?;
        let vector_uuids = vectors
            .iter()
            .map(|result| result.content_uuid)
            .collect::<Vec<_>>();
        let vector_content = db::get_contents(&self.notes, vector_uuids).await?;
        let mut content = fts
            .iter()
            .map(|hit| (hit.content.uuid(), hit.content.clone()))
            .chain(
                vector_content
                    .into_iter()
                    .map(|content| (content.uuid(), content)),
            )
            .collect::<HashMap<_, _>>();
        let mut scores = HashMap::<uuid::Uuid, f64>::new();
        for (rank, hit) in fts.into_iter().enumerate() {
            *scores.entry(hit.content.uuid()).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
        for (rank, hit) in vectors.into_iter().enumerate() {
            *scores.entry(hit.content_uuid).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
        let mut hits = scores
            .into_iter()
            .filter_map(|(uuid, score)| {
                content
                    .remove(&uuid)
                    .map(|content| SearchHit { content, score })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| right.score.total_cmp(&left.score));
        hits.truncate(limit as usize);
        Ok(hits)
    }

    pub async fn retrieve(&self, query: String, limit: u32) -> Result<Vec<SearchHit>> {
        let pool = limit.saturating_mul(4).max(RERANK_POOL_MIN);
        let candidates = self.hybrid_candidates(query.clone(), pool).await?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let documents = candidates.iter().map(hit_text).collect();
        let scored = self.reranker.rerank(query, documents).await?;
        Ok(select_reranked_hits(scored, &candidates, limit as usize))
    }

    pub fn notes(&self) -> &Connection {
        &self.notes
    }

    pub fn reranker(&self) -> &Arc<dyn RerankBackend> {
        &self.reranker
    }
}

fn hit_text(hit: &SearchHit) -> String {
    hit.content.text().to_owned()
}

pub fn select_reranked_hits(
    scored: Vec<(usize, f32)>,
    candidates: &[SearchHit],
    limit: usize,
) -> Vec<SearchHit> {
    let confident = scored
        .iter()
        .filter(|(_, score)| f64::from(*score) >= RELEVANCE_FLOOR)
        .take(limit)
        .filter_map(|(index, score)| {
            candidates.get(*index).map(|candidate| SearchHit {
                content: candidate.content.clone(),
                score: f64::from(*score),
            })
        })
        .collect::<Vec<_>>();
    if !confident.is_empty() {
        return confident;
    }
    scored
        .into_iter()
        .take(limit.min(LOW_CONFIDENCE_FALLBACK_LIMIT))
        .filter_map(|(index, score)| {
            candidates.get(index).map(|candidate| SearchHit {
                content: candidate.content.clone(),
                score: f64::from(score),
            })
        })
        .collect()
}

fn validate(query: &str, limit: u32) -> Result<()> {
    if query.trim().is_empty() || query.chars().count() > 4_096 {
        bail!("query must contain 1 to 4096 characters");
    }
    if !(1..=128).contains(&limit) {
        bail!("limit must be between 1 and 128");
    }
    Ok(())
}

pub fn content_documents(content: &[Content]) -> Vec<String> {
    content
        .iter()
        .map(|content| content.text().to_owned())
        .collect()
}
