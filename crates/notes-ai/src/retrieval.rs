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
        let mut snippets = fts
            .iter()
            .filter_map(|hit| {
                hit.snippet
                    .clone()
                    .map(|snippet| (hit.content.uuid(), snippet))
            })
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
                content.remove(&uuid).map(|content| SearchHit {
                    content,
                    score,
                    snippet: snippets.remove(&uuid),
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| right.score.total_cmp(&left.score));
        hits.truncate(limit as usize);
        Ok(hits)
    }

    pub async fn retrieve(&self, query: String, limit: u32) -> Result<Vec<SearchHit>> {
        self.retrieve_with_rerank(query, limit, true).await
    }

    pub async fn retrieve_with_rerank(
        &self,
        query: String,
        limit: u32,
        rerank: bool,
    ) -> Result<Vec<SearchHit>> {
        if !rerank {
            return self.hybrid_candidates(query, limit).await;
        }
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
                snippet: candidate.snippet.clone(),
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
                snippet: candidate.snippet.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct StubEmbedder;

    #[async_trait]
    impl EmbedderBackend for StubEmbedder {
        fn ndims(&self) -> usize {
            1
        }

        fn id(&self) -> String {
            "stub:embedder".into()
        }

        async fn embed_passages(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
            Ok(texts.into_iter().map(|_| vec![0.0]).collect())
        }

        async fn embed_query(&self, _text: String) -> Result<Vec<f32>> {
            Ok(vec![0.0])
        }
    }

    struct EmptyVectors;

    #[async_trait]
    impl VectorStore for EmptyVectors {
        async fn search(
            &self,
            _embedding: Vec<f32>,
            _limit: u32,
        ) -> Result<Vec<crate::store::VectorMatch>> {
            Ok(Vec::new())
        }
    }

    struct CountingReranker(AtomicUsize);

    #[async_trait]
    impl RerankBackend for CountingReranker {
        async fn rerank(&self, _query: String, _docs: Vec<String>) -> Result<Vec<(usize, f32)>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn rerank_false_keeps_rrf_order_without_calling_the_reranker() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = db::open(directory.path().join("notes.db"))
            .await
            .expect("open notes database");
        let page = db::create_page(&notes, "Needle project".into())
            .await
            .expect("create searchable page");
        let reranker = Arc::new(CountingReranker(AtomicUsize::new(0)));
        let pipeline = RetrievalPipeline::new(
            notes,
            Arc::new(EmptyVectors),
            Arc::new(StubEmbedder),
            reranker.clone(),
        );

        let hits = pipeline
            .retrieve_with_rerank("Needle".into(), 10, false)
            .await
            .expect("retrieve without reranking");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].content.uuid(), page.uuid);
        assert_eq!(reranker.0.load(Ordering::SeqCst), 0);
    }
}
