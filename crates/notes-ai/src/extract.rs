use crate::store::{AiStore, ExtractedEdge, ExtractedEntityRecord, IndexJob};
use anyhow::Result;
use notes_core::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;

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
#[serde(deny_unknown_fields)]
pub struct ExtractedEntity {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractedRelation {
    pub src: String,
    pub dst: String,
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractionResult {
    pub entities: Vec<ExtractedEntity>,
    pub relations: Vec<ExtractedRelation>,
}

pub struct EntityExtractor {
    config: llm_relay::ClientConfig,
}

impl EntityExtractor {
    pub fn new(config: llm_relay::ClientConfig) -> Self {
        Self { config }
    }

    pub async fn extract(&self, text: String) -> Result<ExtractionResult> {
        let client = llm_relay::LlmClient::new(self.config.clone().max_tokens(2_048))?;
        let response = client
            .complete_structured::<ExtractionResult>(&text, "entity_extraction", Some(PREAMBLE))
            .await?;
        tracing::info!(
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            "entity extraction completed"
        );
        Ok(response.data)
    }
}

pub fn spawn_worker(
    notes: Connection,
    store: Arc<AiStore>,
    extractor: Arc<EntityExtractor>,
    on_entities_changed: Arc<dyn Fn() + Send + Sync>,
    on_status_changed: Arc<dyn Fn() + Send + Sync>,
    wake: Arc<Notify>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let did_work = match store.control().await {
                Ok(control) if control.entity_extraction => {
                    let result = tokio::select! {
                        () = shutdown.cancelled() => return,
                        result = tick(&notes, &store, &extractor, on_entities_changed.as_ref()) => result,
                    };
                    match result {
                        Ok(true) => {
                            on_status_changed();
                            true
                        }
                        Ok(false) => false,
                        Err(error) => {
                            on_status_changed();
                            tracing::warn!(?error, "extract worker tick failed");
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

async fn tick(
    notes: &Connection,
    store: &AiStore,
    extractor: &EntityExtractor,
    on_entities_changed: &(dyn Fn() + Send + Sync),
) -> Result<bool> {
    let stale_sources = store.reconcile_extractions(notes).await?;
    for source_uuid in stale_sources {
        store.forget_extraction_source(source_uuid).await?;
        on_entities_changed();
    }
    let jobs = store.take_extraction_jobs(1).await?;
    let changed = !jobs.is_empty();
    for job in jobs {
        if job.input_text.trim().chars().count() < 3 {
            store
                .finish_extraction(&job, Vec::new(), Vec::new())
                .await?;
            continue;
        }
        match extractor.extract(job.input_text.clone()).await {
            Ok(result) => {
                if !store.extraction_job_is_current(notes, &job).await? {
                    continue;
                }
                if let Err(error) = apply_extraction(store, &job, result).await {
                    tracing::warn!(content_uuid = %job.content_uuid, ?error, "applying extraction failed");
                    store
                        .record_extraction_failure(&job, &error.to_string(), true)
                        .await?;
                } else {
                    on_entities_changed();
                }
            }
            Err(error) => {
                tracing::warn!(content_uuid = %job.content_uuid, ?error, "extraction failed");
                store
                    .record_extraction_failure(
                        &job,
                        &error.to_string(),
                        crate::failure::provider_failure_is_terminal(&error),
                    )
                    .await?;
            }
        }
    }
    Ok(changed)
}

async fn apply_extraction(store: &AiStore, job: &IndexJob, result: ExtractionResult) -> Result<()> {
    let mut entities = HashMap::<String, uuid::Uuid>::new();
    let mut entity_records = Vec::new();
    let mut desired_edges = Vec::new();
    for entity in result.entities {
        let name = entity.name.trim();
        if name.is_empty() || name.chars().count() > 200 {
            continue;
        }
        let key = name.to_lowercase();
        let uuid = entity_uuid(&key);
        let description = entity
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .unwrap_or_default()
            .to_owned();
        entity_records.push(ExtractedEntityRecord {
            uuid,
            name: name.to_owned(),
            normalized_name: key.clone(),
            description,
        });
        entities.insert(key, uuid);
        desired_edges.push(ExtractedEdge {
            src_uuid: job.content_uuid,
            dst_uuid: uuid,
            kind: "mentions".into(),
        });
    }
    for relation in result.relations {
        let Some(&src_uuid) = entities.get(&relation.src.trim().to_lowercase()) else {
            continue;
        };
        let Some(&dst_uuid) = entities.get(&relation.dst.trim().to_lowercase()) else {
            continue;
        };
        let kind = relation.kind.trim().to_lowercase();
        if src_uuid == dst_uuid || kind.is_empty() || kind.chars().count() > 64 {
            continue;
        }
        desired_edges.push(ExtractedEdge {
            src_uuid,
            dst_uuid,
            kind: format!("ai:{kind}"),
        });
    }
    desired_edges.sort_by(|left, right| {
        (&left.src_uuid, &left.dst_uuid, &left.kind).cmp(&(
            &right.src_uuid,
            &right.dst_uuid,
            &right.kind,
        ))
    });
    desired_edges.dedup();
    store
        .finish_extraction(job, entity_records, desired_edges)
        .await
}

fn entity_uuid(normalized_name: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("notes-rs:entity:{normalized_name}").as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_contract_rejects_provider_shortcuts() {
        let error = serde_json::from_value::<ExtractionResult>(serde_json::json!({
            "entities": ["Aurora:A project."],
            "relations": []
        }))
        .expect_err("entity strings must not bypass the typed schema");
        assert!(
            error
                .to_string()
                .contains("expected struct ExtractedEntity")
        );
    }

    #[test]
    fn entity_identity_is_case_insensitive_and_stable() {
        assert_eq!(entity_uuid("rust"), entity_uuid("rust"));
        assert_ne!(entity_uuid("rust"), entity_uuid("tauri"));
    }
}
