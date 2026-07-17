use crate::store::{AiStore, ExtractedEdge, IndexJob};
use anyhow::Result;
use notes_core::operation::{EdgeAdd, EdgeRemove, NodeCreate, NodeSetContent};
use notes_core::{Connection, NodeKind, OpKind, Origin};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

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
) {
    tokio::spawn(async move {
        loop {
            match store.control().await {
                Ok(control) if control.entity_extraction => {
                    match tick(&notes, &store, &extractor, on_entities_changed.as_ref()).await {
                        Ok(true) => on_status_changed(),
                        Ok(false) => {}
                        Err(error) => {
                            on_status_changed();
                            tracing::warn!(?error, "extract worker tick failed");
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(?error, "reading AI worker settings failed"),
            }
            sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn tick(
    notes: &Connection,
    store: &AiStore,
    extractor: &EntityExtractor,
    on_entities_changed: &(dyn Fn() + Send + Sync),
) -> Result<bool> {
    let stale_sources = store
        .reconcile_extractions(notes, notes_core::sync_cursor(notes).await?)
        .await?;
    for source_uuid in stale_sources {
        let removals = store.extraction_removals(source_uuid, &[]).await?;
        let kinds = removals
            .into_iter()
            .map(|edge| {
                OpKind::EdgeRemove(EdgeRemove {
                    src_uuid: edge.src_uuid,
                    dst_uuid: edge.dst_uuid,
                    edge_kind: edge.kind,
                })
            })
            .collect::<Vec<_>>();
        if !kinds.is_empty() {
            let operations = notes_core::local_ops(notes, kinds).await?;
            notes_core::apply_batch(notes, &operations, Origin::Local).await?;
            on_entities_changed();
        }
        store.forget_extraction_source(source_uuid).await?;
    }
    let jobs = store.take_extraction_jobs(1).await?;
    let changed = !jobs.is_empty();
    for job in jobs {
        if job.input_text.trim().chars().count() < 3 {
            store.finish_extraction(&job, Vec::new()).await?;
            continue;
        }
        match extractor.extract(job.input_text.clone()).await {
            Ok(result) => {
                if !store.extraction_job_is_current(notes, &job).await? {
                    continue;
                }
                if let Err(error) = apply_extraction(notes, store, &job, result).await {
                    tracing::warn!(node_uuid = %job.node_uuid, ?error, "applying extraction failed");
                    store
                        .record_extraction_failure(&job, &error.to_string(), true)
                        .await?;
                } else {
                    on_entities_changed();
                }
            }
            Err(error) => {
                tracing::warn!(node_uuid = %job.node_uuid, ?error, "extraction failed");
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

async fn apply_extraction(
    notes: &Connection,
    store: &AiStore,
    job: &IndexJob,
    result: ExtractionResult,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let mut kinds = Vec::new();
    let mut entities = HashMap::<String, uuid::Uuid>::new();
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
        match notes_core::db::get_node_by_uuid(notes, uuid).await? {
            None => kinds.push(OpKind::NodeCreate(NodeCreate {
                uuid,
                node_kind: NodeKind::Entity,
                title: Some(name.to_owned()),
                content: description,
                content_json: None,
                parent_uuid: None,
                position: None,
                created_at: now,
            })),
            Some(existing) if !description.is_empty() && existing.content != description => {
                kinds.push(OpKind::NodeSetContent(NodeSetContent {
                    uuid,
                    content: description,
                    content_json: None,
                }));
            }
            Some(_) => {}
        }
        entities.insert(key, uuid);
        desired_edges.push(ExtractedEdge {
            src_uuid: job.node_uuid,
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
    for edge in store
        .extraction_removals(job.node_uuid, &desired_edges)
        .await?
    {
        kinds.push(OpKind::EdgeRemove(EdgeRemove {
            src_uuid: edge.src_uuid,
            dst_uuid: edge.dst_uuid,
            edge_kind: edge.kind,
        }));
    }
    for edge in &desired_edges {
        kinds.push(OpKind::EdgeAdd(EdgeAdd {
            src_uuid: edge.src_uuid,
            dst_uuid: edge.dst_uuid,
            edge_kind: edge.kind.clone(),
            weight: 1.0,
        }));
    }
    let operations = notes_core::local_ops(notes, kinds).await?;
    notes_core::apply_batch(notes, &operations, Origin::Local).await?;
    store.finish_extraction(job, desired_edges).await
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
