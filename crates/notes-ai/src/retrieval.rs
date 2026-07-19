use crate::embed::{EmbedderBackend, RerankBackend};
use crate::store::{VectorMatch, VectorStore};
use anyhow::{Result, bail};
use notes_core::Connection;
use notes_core::db::{self, Content, SearchHit};
use std::collections::HashMap;
use std::sync::Arc;

const RRF_K: f64 = 60.0;
const HYBRID_POOL_MULTIPLIER: u32 = 4;
const VECTOR_CHUNK_OVERFETCH_MULTIPLIER: u32 = 4;
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

struct RankedCandidate {
    hit: SearchHit,
    rerank_text: String,
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
        Ok(self
            .hybrid_ranked_candidates(query, limit)
            .await?
            .into_iter()
            .map(|candidate| candidate.hit)
            .collect())
    }

    async fn hybrid_ranked_candidates(
        &self,
        query: String,
        limit: u32,
    ) -> Result<Vec<RankedCandidate>> {
        validate(&query, limit)?;
        let embedding = self.embedder.embed_query(query.clone()).await?;
        let pool_limit = limit.saturating_mul(HYBRID_POOL_MULTIPLIER);
        let vector_chunk_limit = pool_limit.saturating_mul(VECTOR_CHUNK_OVERFETCH_MULTIPLIER);
        let (fts, vector_chunks) = tokio::try_join!(
            db::search_fts(&self.notes, query, pool_limit),
            self.vectors.search(embedding, vector_chunk_limit)
        )?;
        let vectors = dedup_vector_matches(vector_chunks, pool_limit as usize);
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
        let mut rerank_texts = vectors
            .iter()
            .filter(|result| !result.chunk_text.is_empty())
            .map(|result| (result.content_uuid, result.chunk_text.clone()))
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
                content.remove(&uuid).map(|content| RankedCandidate {
                    rerank_text: rerank_texts
                        .remove(&uuid)
                        .unwrap_or_else(|| content.text().to_owned()),
                    hit: SearchHit {
                        content,
                        score,
                        snippet: snippets.remove(&uuid),
                    },
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| right.hit.score.total_cmp(&left.hit.score));
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
        let candidates = self.hybrid_ranked_candidates(query.clone(), pool).await?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let documents = candidates
            .iter()
            .map(|candidate| candidate.rerank_text.clone())
            .collect();
        let scored = self.reranker.rerank(query, documents).await?;
        let hits = candidates
            .into_iter()
            .map(|candidate| candidate.hit)
            .collect::<Vec<_>>();
        Ok(select_reranked_hits(scored, &hits, limit as usize))
    }

    pub fn notes(&self) -> &Connection {
        &self.notes
    }

    pub fn reranker(&self) -> &Arc<dyn RerankBackend> {
        &self.reranker
    }
}

fn dedup_vector_matches(matches: Vec<VectorMatch>, limit: usize) -> Vec<VectorMatch> {
    let mut best = HashMap::<uuid::Uuid, VectorMatch>::new();
    for candidate in matches {
        match best.entry(candidate.content_uuid) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::hash_map::Entry::Occupied(mut entry)
                if candidate.distance < entry.get().distance =>
            {
                entry.insert(candidate);
            }
            _ => {}
        }
    }
    let mut matches = best.into_values().collect::<Vec<_>>();
    matches.sort_by(|left, right| left.distance.total_cmp(&right.distance));
    matches.truncate(limit);
    matches
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
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

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

    struct StaticVectors(Vec<VectorMatch>);

    #[derive(Default)]
    struct CapturingVectors(AtomicU32);

    #[async_trait]
    impl VectorStore for StaticVectors {
        async fn search(&self, _embedding: Vec<f32>, _limit: u32) -> Result<Vec<VectorMatch>> {
            Ok(self.0.clone())
        }
    }

    #[async_trait]
    impl VectorStore for CapturingVectors {
        async fn search(&self, _embedding: Vec<f32>, limit: u32) -> Result<Vec<VectorMatch>> {
            self.0.store(limit, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct CapturingReranker(Mutex<Vec<String>>);

    #[async_trait]
    impl RerankBackend for CapturingReranker {
        async fn rerank(&self, _query: String, docs: Vec<String>) -> Result<Vec<(usize, f32)>> {
            *self.0.lock().unwrap() = docs;
            Ok(vec![(0, 0.9)])
        }
    }

    #[test]
    fn vector_chunk_dedup_keeps_the_best_distance_and_matched_text() {
        let first = uuid::Uuid::from_u128(1);
        let second = uuid::Uuid::from_u128(2);
        let deduped = dedup_vector_matches(
            vec![
                VectorMatch {
                    content_uuid: first,
                    distance: 0.4,
                    chunk_index: 0,
                    chunk_text: "weaker chunk".into(),
                },
                VectorMatch {
                    content_uuid: second,
                    distance: 0.2,
                    chunk_index: 0,
                    chunk_text: "second document".into(),
                },
                VectorMatch {
                    content_uuid: first,
                    distance: 0.1,
                    chunk_index: 3,
                    chunk_text: "best chunk".into(),
                },
            ],
            2,
        );

        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].content_uuid, first);
        assert_eq!(deduped[0].chunk_index, 3);
        assert_eq!(deduped[0].chunk_text, "best chunk");
        assert_eq!(deduped[1].content_uuid, second);
    }

    #[test]
    fn vector_chunk_dedup_truncates_only_after_selecting_unique_content() {
        let repeated = uuid::Uuid::from_u128(1);
        let matches = (0..8)
            .map(|chunk_index| VectorMatch {
                content_uuid: repeated,
                distance: 0.01 + f64::from(chunk_index) / 100.0,
                chunk_index,
                chunk_text: format!("repeated chunk {chunk_index}"),
            })
            .chain((2..=6).map(|id| VectorMatch {
                content_uuid: uuid::Uuid::from_u128(id),
                distance: id as f64 / 10.0,
                chunk_index: 0,
                chunk_text: format!("document {id}"),
            }))
            .collect();

        let deduped = dedup_vector_matches(matches, 4);

        assert_eq!(deduped.len(), 4);
        assert_eq!(deduped[0].content_uuid, repeated);
        assert_eq!(deduped[0].chunk_index, 0);
        assert_eq!(
            deduped
                .iter()
                .map(|candidate| candidate.content_uuid)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            4
        );
    }

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

    #[tokio::test]
    async fn knn_overfetches_chunks_for_the_unique_hybrid_pool() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = db::open(directory.path().join("notes.db"))
            .await
            .expect("open notes database");
        let vectors = Arc::new(CapturingVectors::default());
        let pipeline = RetrievalPipeline::new(
            notes,
            vectors.clone(),
            Arc::new(StubEmbedder),
            Arc::new(CountingReranker(AtomicUsize::new(0))),
        );

        let hits = pipeline
            .hybrid_candidates("absent query".into(), 5)
            .await
            .expect("hybrid candidates");

        assert!(hits.is_empty());
        assert_eq!(
            vectors.0.load(Ordering::SeqCst),
            5 * HYBRID_POOL_MULTIPLIER * VECTOR_CHUNK_OVERFETCH_MULTIPLIER
        );
    }

    #[tokio::test]
    async fn reranker_receives_the_best_matched_chunk_instead_of_the_whole_block() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = db::open(directory.path().join("notes.db"))
            .await
            .expect("open notes database");
        let page = db::create_page(&notes, "Vector page".into())
            .await
            .expect("create page");
        let block = db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            notes_core::BlockStyle::Paragraph,
            "the complete block is deliberately different".into(),
        )
        .await
        .expect("create block");
        let reranker = Arc::new(CapturingReranker::default());
        let pipeline = RetrievalPipeline::new(
            notes,
            Arc::new(StaticVectors(vec![VectorMatch {
                content_uuid: block.uuid,
                distance: 0.1,
                chunk_index: 2,
                chunk_text: "Vector page\nmatched tail chunk".into(),
            }])),
            Arc::new(StubEmbedder),
            reranker.clone(),
        );

        let hits = pipeline
            .retrieve("semantic query absent from FTS".into(), 5)
            .await
            .expect("retrieve with reranking");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].content.uuid(), block.uuid);
        assert_eq!(
            *reranker.0.lock().unwrap(),
            vec!["Vector page\nmatched tail chunk"]
        );
    }
}
