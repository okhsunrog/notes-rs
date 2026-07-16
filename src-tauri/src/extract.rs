use crate::sqlite::Connection;
use anyhow::{Context, Result};
use rig::client::{CompletionClient, ProviderClient};
use rig::extractor::Extractor;
use rig::providers::openrouter;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::OnceCell;
use tokio::time::{Duration, sleep};

/// Stable change-detection hash of (title, content). Sha1 is fine here —
/// we're not protecting against adversarial collisions, just detecting
/// "is this the same text the LLM already saw?".
fn content_hash(title: Option<&str>, content: &str) -> String {
    let mut h = Sha1::new();
    h.update(title.unwrap_or("").as_bytes());
    h.update([0x1f]); // delimiter so "ab|c" and "a|bc" don't collide
    h.update(content.as_bytes());
    let bytes = h.finalize();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes.as_ref() as &[u8] {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Lighter, cheaper model is fine for structured extraction.
const EXTRACT_MODEL: &str = "deepseek/deepseek-v4-flash";

const PREAMBLE: &str = r#"
Extract named entities and their relations from the user's note.

Entities are concrete, reusable nouns the user might reference later: people, organizations,
projects, technologies, places, concepts, events. Skip generic words ("idea", "thing"),
single common verbs, and stop-words.

For each entity provide:
- name: canonical form (e.g. "PostgreSQL" not "postgres")
- description: 1-line summary of what it is, in the same language as the note

For each pair of entities mentioned together, optionally provide a relation describing
how they're connected (e.g. "uses", "part_of", "developed_by"). Skip if uncertain.

If the note has no extractable entities, return empty arrays. Do not invent.
"#;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExtractedEntity {
    /// Canonical name of the entity.
    pub name: String,
    /// One-sentence description in the same language as the source.
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExtractedRelation {
    /// Source entity name (must match one in `entities`).
    pub src: String,
    /// Destination entity name (must match one in `entities`).
    pub dst: String,
    /// Relation kind (snake_case verb phrase): uses, part_of, depends_on, developed_by, …
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExtractionResult {
    pub entities: Vec<ExtractedEntity>,
    pub relations: Vec<ExtractedRelation>,
}

pub struct EntityExtractor {
    inner: OnceCell<Extractor<openrouter::CompletionModel, ExtractionResult>>,
}

impl EntityExtractor {
    pub fn new() -> Self {
        Self {
            inner: OnceCell::new(),
        }
    }

    async fn get(&self) -> Result<&Extractor<openrouter::CompletionModel, ExtractionResult>> {
        self.inner
            .get_or_try_init(|| async {
                let client =
                    openrouter::Client::from_env().context("OPENROUTER_API_KEY not set")?;
                let extractor = client
                    .extractor::<ExtractionResult>(EXTRACT_MODEL)
                    .preamble(PREAMBLE)
                    .retries(2)
                    .build();
                Ok::<_, anyhow::Error>(extractor)
            })
            .await
    }

    pub async fn extract(&self, text: String) -> Result<ExtractionResult> {
        let ex = self.get().await?;
        Ok(ex.extract(text).await?)
    }
}

pub fn spawn_worker(conn: Connection, extractor: Arc<EntityExtractor>, app: AppHandle) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = tick(&conn, &extractor, &app).await {
                tracing::warn!(error = ?e, "extract worker tick failed");
            }
            sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn tick(conn: &Connection, extractor: &EntityExtractor, app: &AppHandle) -> Result<()> {
    let batch = crate::db::take_pending_extractions(conn, 1).await?;
    for (node_id, title, content) in batch {
        let new_hash = content_hash(title.as_deref(), &content);
        // Skip the LLM call if (title, content) is identical to the last
        // successful extraction — typo-fix cycles re-fire the update trigger
        // but produce no semantic change worth extracting again.
        let prev_hash = crate::db::get_last_extracted_hash(conn, node_id).await?;
        if prev_hash.as_deref() == Some(new_hash.as_str()) {
            crate::db::finish_extraction(conn, node_id).await?;
            continue;
        }
        let text = format!("{}\n{content}", title.as_deref().unwrap_or(""));
        match extractor.extract(text).await {
            Ok(result) => {
                let had_entities = !result.entities.is_empty();
                match apply(conn, node_id, result).await {
                    Ok(()) => {
                        crate::db::set_last_extracted_hash(conn, node_id, new_hash).await?;
                        crate::db::finish_extraction(conn, node_id).await?;
                        if had_entities {
                            // Wake the UI so the entities sidebar refreshes without polling.
                            let _ = app.emit("entities:changed", ());
                        }
                    }
                    Err(e) => {
                        tracing::warn!(node_id, error = ?e, "applying extraction failed");
                        crate::db::record_extraction_failure(conn, node_id).await?;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(node_id, error = ?e, "extraction failed; will retry with backoff");
                crate::db::record_extraction_failure(conn, node_id).await?;
            }
        }
    }
    Ok(())
}

async fn apply(conn: &Connection, source_id: i64, result: ExtractionResult) -> Result<()> {
    use std::collections::HashMap;
    let mut name_to_id: HashMap<String, i64> = HashMap::new();

    for ent in result.entities {
        let id = crate::db::upsert_entity(conn, ent.name.clone(), ent.description).await?;
        name_to_id.insert(ent.name.to_lowercase(), id);
        crate::db::link_nodes(conn, source_id, id, "mentions".into(), 1.0).await?;
    }

    for rel in result.relations {
        let src = name_to_id.get(&rel.src.to_lowercase()).copied();
        let dst = name_to_id.get(&rel.dst.to_lowercase()).copied();
        if let (Some(s), Some(d)) = (src, dst)
            && s != d
        {
            crate::db::link_nodes(conn, s, d, rel.kind, 1.0).await?;
        }
    }
    Ok(())
}
