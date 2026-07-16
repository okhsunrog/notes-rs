use crate::sqlite::Connection;
use anyhow::Result;
use llm_relay::RigClient;
use rig::client::CompletionClient;
use rig::completion::CompletionModel;
use rig::extractor::ExtractorBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};
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

pub struct EntityExtractor;

impl EntityExtractor {
    pub fn new() -> Self {
        Self
    }

    pub async fn extract(&self, text: String) -> Result<ExtractionResult> {
        let config = crate::settings::extraction_completion_config()?;
        match config.rig_client()? {
            RigClient::OpenAi(client) => {
                extract_with_model(client.completion_model(&config.model), text).await
            }
            RigClient::Anthropic(client) => {
                extract_with_model(client.completion_model(&config.model), text).await
            }
        }
    }
}

async fn extract_with_model<M: CompletionModel + 'static>(
    model: M,
    text: String,
) -> Result<ExtractionResult> {
    let extractor = ExtractorBuilder::new(model)
        .preamble(PREAMBLE)
        // The durable extraction queue already retries with exponential
        // backoff. Retrying inside one tick multiplies permanent 4xx failures
        // and can burn through a provider quota without improving recovery.
        .retries(0)
        .build();
    Ok(extractor.extract(text).await?)
}

pub fn spawn_worker(
    conn: Connection,
    extractor: Arc<EntityExtractor>,
    app: AppHandle,
    paused: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        loop {
            if !paused.load(Ordering::Acquire)
                && let Err(e) = tick(&conn, &extractor, &app).await
            {
                tracing::warn!(error = ?e, "extract worker tick failed");
            }
            sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn tick(conn: &Connection, extractor: &EntityExtractor, app: &AppHandle) -> Result<()> {
    let batch = crate::db::take_pending_extractions(conn, 1).await?;
    for (node_id, title, content) in batch {
        let meaningful_text = format!("{}\n{content}", title.as_deref().unwrap_or(""));
        if meaningful_text.trim().chars().count() < 3 {
            crate::db::finish_extraction(conn, node_id).await?;
            continue;
        }
        let new_hash = content_hash(title.as_deref(), &content);
        // Skip the LLM call if (title, content) is identical to the last
        // successful extraction — typo-fix cycles re-fire the update trigger
        // but produce no semantic change worth extracting again.
        let prev_hash = crate::db::get_last_extracted_hash(conn, node_id).await?;
        if prev_hash.as_deref() == Some(new_hash.as_str()) {
            crate::db::finish_extraction(conn, node_id).await?;
            continue;
        }
        match extractor.extract(meaningful_text).await {
            Ok(result) => {
                match apply(conn, node_id, title.clone(), content.clone(), result).await {
                    Ok(()) => {
                        crate::db::set_last_extracted_hash(conn, node_id, new_hash).await?;
                        crate::db::finish_extraction(conn, node_id).await?;
                        // A replacement can add or remove the final mention,
                        // so refresh even when the new result is empty.
                        let _ = app.emit("entities:changed", ());
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

async fn apply(
    conn: &Connection,
    source_id: i64,
    expected_title: Option<String>,
    expected_content: String,
    result: ExtractionResult,
) -> Result<()> {
    let entities = result
        .entities
        .into_iter()
        .map(|entity| (entity.name, entity.description))
        .collect();
    let relations = result
        .relations
        .into_iter()
        .map(|relation| (relation.src, relation.dst, relation.kind))
        .collect();
    crate::db::replace_extracted_edges(
        conn,
        source_id,
        expected_title,
        expected_content,
        entities,
        relations,
    )
    .await
}
