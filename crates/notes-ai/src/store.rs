use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use notes_core::{BlockStyle, Connection};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use rusqlite::OptionalExtension;
use rusqlite::ffi::sqlite3_auto_extension;
use rusqlite_migration::{M, Migrations};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Once;
use unicode_categories::UnicodeCategories;

use crate::chunking::{SINGLE_VECTOR_MAX_CHARS, TextChunk, split_for_embedding};

const INPUT_FORMAT_VERSION: u32 = 2;
const MAX_ATTEMPTS: i64 = 8;
const BACKOFF_BASE_SECS: i64 = 5;
const PAGE_VECTOR_MAX_CHARS: usize = 1_000;

static VEC_INIT: Once = Once::new();

fn register_sqlite_vec() {
    VEC_INIT.call_once(|| unsafe {
        type ExtensionEntry = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        sqlite3_auto_extension(Some(std::mem::transmute::<*const (), ExtensionEntry>(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("store/migrations/V001__initial.sql")),
        M::up(include_str!("store/migrations/V002__chunked_vectors.sql")),
        M::up(include_str!("store/migrations/V003__reconcile_cursor.sql")),
    ])
}

#[derive(Debug, Clone)]
pub struct IndexJob {
    pub content_uuid: uuid::Uuid,
    pub input_hash: String,
    pub input_text: String,
    pub input_header: String,
}

#[derive(Debug, Clone)]
pub struct EmbeddedChunk {
    pub chunk_index: u32,
    pub input_text: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct EmbeddedDocument {
    pub content_uuid: uuid::Uuid,
    pub input_hash: String,
    pub chunks: Vec<EmbeddedChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExtractedEdge {
    pub src_uuid: uuid::Uuid,
    pub dst_uuid: uuid::Uuid,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedEntityRecord {
    pub uuid: uuid::Uuid,
    pub name: String,
    pub normalized_name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AiEntity {
    pub uuid: uuid::Uuid,
    pub name: String,
    pub description: String,
    pub mention_count: u64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct VectorMatch {
    pub content_uuid: uuid::Uuid,
    pub distance: f64,
    pub chunk_index: u32,
    pub chunk_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedContentKind {
    Page,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInspectionItem {
    pub content_uuid: uuid::Uuid,
    pub kind: IndexedContentKind,
    pub composed_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexLengthBucket {
    pub label: &'static str,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedDocument {
    pub item: IndexInspectionItem,
    pub composed_text: String,
    pub chunks: Vec<TextChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInspection {
    pub document_count: usize,
    pub skipped_content_free_blocks: usize,
    pub tier_two_blocks: usize,
    pub composed_chars: usize,
    pub estimated_tokens: usize,
    pub histogram: Vec<IndexLengthBucket>,
    pub longest: Vec<IndexInspectionItem>,
    pub selected: Option<InspectedDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStatus {
    pub identity: String,
    pub dimensions: usize,
    pub generation_id: uuid::Uuid,
    pub generation_status: GenerationStatus,
    pub control: AiControl,
    pub pending: u64,
    pub failed: u64,
    pub indexed: u64,
    pub source_documents: u64,
    pub extraction_pending: u64,
    pub extraction_failed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationStatus {
    Building,
    Active,
    Retired,
}

impl GenerationStatus {
    fn from_database(value: &str) -> Result<Self> {
        match value {
            "building" => Ok(Self::Building),
            "active" => Ok(Self::Active),
            "retired" => Ok(Self::Retired),
            _ => bail!("unknown AI index generation status {value:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiControl {
    pub automatic_embeddings: bool,
    pub entity_extraction: bool,
    pub query_rewriting: bool,
}

impl Default for AiControl {
    fn default() -> Self {
        Self {
            automatic_embeddings: false,
            entity_extraction: false,
            query_rewriting: true,
        }
    }
}

#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn search(&self, embedding: Vec<f32>, limit: u32) -> Result<Vec<VectorMatch>>;
}

#[derive(Clone)]
pub struct AiStore {
    connection: Connection,
    identity: String,
    dimensions: usize,
    generation_id: uuid::Uuid,
    table_name: String,
    #[cfg(test)]
    reconcile_scans: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl AiStore {
    pub async fn open(path: impl AsRef<Path>, identity: String, dimensions: usize) -> Result<Self> {
        Self::open_with_control(path, identity, dimensions, AiControl::default()).await
    }

    pub async fn open_with_control(
        path: impl AsRef<Path>,
        identity: String,
        dimensions: usize,
        default_control: AiControl,
    ) -> Result<Self> {
        if dimensions == 0 {
            bail!("embedding dimensions must be positive");
        }
        let initialize_control = !path.as_ref().exists();
        register_sqlite_vec();
        let connection = Connection::open(path.as_ref())
            .await
            .context("opening server AI database")?;
        connection
            .call(|database| {
                database.pragma_update(None, "journal_mode", "WAL")?;
                database.pragma_update(None, "synchronous", "NORMAL")?;
                database.pragma_update(None, "foreign_keys", "ON")?;
                migrations()
                    .to_latest(database)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
            })
            .await
            .context("applying server AI database migrations")?;

        let configured_identity = identity.clone();
        let (generation_id, table_name) = connection
            .call(move |database| {
                let existing = database
                    .query_row(
                        "SELECT id, table_name FROM index_generations
                         WHERE identity = ?1 AND dimensions = ?2
                           AND status IN ('building', 'active')
                         ORDER BY status = 'active' DESC, created_at DESC LIMIT 1",
                        rusqlite::params![configured_identity, dimensions as i64],
                        |row| Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?;
                if let Some(existing) = existing {
                    return Ok(existing);
                }
                let generation_id = uuid::Uuid::now_v7();
                let table_name = vector_table_name(generation_id);
                database.execute(
                    "INSERT INTO index_generations
                       (id, identity, dimensions, table_name, status, created_at)
                     VALUES (?1, ?2, ?3, ?4, 'building', unixepoch())",
                    rusqlite::params![
                        generation_id,
                        configured_identity,
                        dimensions as i64,
                        table_name
                    ],
                )?;
                Ok((generation_id, table_name))
            })
            .await?;
        ensure_vector_table(&connection, &table_name, dimensions).await?;
        let store = Self {
            connection,
            identity,
            dimensions,
            generation_id,
            table_name,
            #[cfg(test)]
            reconcile_scans: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        if initialize_control {
            store.set_control(default_control).await?;
        }
        Ok(store)
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    pub async fn control(&self) -> Result<AiControl> {
        self.connection
            .call(|database| {
                database.query_row(
                    "SELECT automatic_embeddings, entity_extraction, query_rewriting
                     FROM index_control WHERE singleton = 1",
                    [],
                    |row| {
                        Ok(AiControl {
                            automatic_embeddings: row.get(0)?,
                            entity_extraction: row.get(1)?,
                            query_rewriting: row.get(2)?,
                        })
                    },
                )
            })
            .await
    }

    pub async fn set_control(&self, control: AiControl) -> Result<AiControl> {
        self.connection
            .call(move |database| {
                database.execute(
                    "UPDATE index_control SET automatic_embeddings = ?1,
                       entity_extraction = ?2, query_rewriting = ?3
                     WHERE singleton = 1",
                    rusqlite::params![
                        control.automatic_embeddings,
                        control.entity_extraction,
                        control.query_rewriting
                    ],
                )?;
                Ok(control)
            })
            .await
    }

    pub async fn list_entities(&self, limit: u32) -> Result<Vec<AiEntity>> {
        if !(1..=200).contains(&limit) {
            bail!("entity limit must be between 1 and 200");
        }
        self.connection
            .call(move |database| {
                let mut statement = database.prepare(
                    "SELECT entity.uuid, entity.name, entity.description, entity.updated_at,
                            COUNT(DISTINCT edge.source_uuid) AS mention_count
                       FROM entities entity
                       LEFT JOIN extraction_edges edge
                         ON edge.dst_uuid = entity.uuid AND edge.kind = 'mentions'
                      GROUP BY entity.uuid, entity.name, entity.description, entity.updated_at
                      ORDER BY mention_count DESC, entity.updated_at DESC,
                               entity.normalized_name
                      LIMIT ?1",
                )?;
                statement
                    .query_map([limit], |row| {
                        Ok(AiEntity {
                            uuid: row.get(0)?,
                            name: row.get(1)?,
                            description: row.get(2)?,
                            updated_at: row.get(3)?,
                            mention_count: row.get::<_, i64>(4)? as u64,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
    }

    pub async fn reset_index(&self) -> Result<()> {
        let generation_id = self.generation_id;
        let table_name = self.table_name.clone();
        self.connection
            .call(move |database| {
                let transaction = database.transaction()?;
                transaction.execute(&format!("DELETE FROM {table_name}"), [])?;
                transaction.execute(
                    "DELETE FROM generation_vectors WHERE generation_id = ?1",
                    [generation_id],
                )?;
                transaction.execute(
                    "DELETE FROM embedding_jobs WHERE generation_id = ?1",
                    [generation_id],
                )?;
                transaction.execute(
                    "UPDATE index_generations
                     SET status = 'building', activated_at = NULL,
                         last_reconciled_cursor = NULL, source_documents = 0
                     WHERE id = ?1",
                    [generation_id],
                )?;
                transaction.commit()
            })
            .await
    }

    pub async fn reconcile(&self, notes: &Connection) -> Result<u64> {
        let source = index_source_cursor(notes).await?;
        let generation_id = self.generation_id;
        let reconciled = self
            .connection
            .call(move |database| {
                database.query_row(
                    "SELECT last_reconciled_cursor, source_documents
                     FROM index_generations WHERE id = ?1",
                    [generation_id],
                    |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
                )
            })
            .await?;
        if reconciled.0.as_deref() == Some(source.token.as_str()) {
            return Ok(reconciled.1 as u64);
        }
        #[cfg(test)]
        self.reconcile_scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let documents = index_documents(notes).await?;
        let source_documents = documents.len() as u64;
        let table_name = self.table_name.clone();
        self.connection
            .call(move |database| {
                let transaction = database.transaction()?;
                let current = {
                    let mut statement = transaction.prepare(
                        "SELECT content_uuid, input_hash FROM generation_vectors
                         WHERE generation_id = ?1
                         GROUP BY content_uuid, input_hash",
                    )?;
                    statement
                        .query_map([generation_id], |row| {
                            Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, String>(1)?))
                        })?
                        .collect::<Result<HashMap<_, _>, _>>()?
                };
                let present = documents.keys().copied().collect::<HashSet<_>>();
                for (content_uuid, document) in documents {
                    if current.get(&content_uuid) == Some(&document.input_hash) {
                        transaction.execute(
                            "DELETE FROM embedding_jobs
                             WHERE generation_id = ?1 AND content_uuid = ?2",
                            rusqlite::params![generation_id, content_uuid],
                        )?;
                        continue;
                    }
                    transaction.execute(
                        "INSERT INTO embedding_jobs
                           (generation_id, content_uuid, input_hash, input_text, input_header,
                            source_seq,
                            enqueued_at, retry_count, last_attempt, last_error, terminal)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch(), 0, NULL, NULL, 0)
                         ON CONFLICT(generation_id, content_uuid) DO UPDATE SET
                           input_hash = excluded.input_hash,
                           input_text = excluded.input_text,
                           input_header = excluded.input_header,
                           source_seq = excluded.source_seq,
                           enqueued_at = excluded.enqueued_at,
                           retry_count = CASE
                             WHEN embedding_jobs.input_hash = excluded.input_hash
                             THEN embedding_jobs.retry_count ELSE 0 END,
                           last_attempt = CASE
                             WHEN embedding_jobs.input_hash = excluded.input_hash
                             THEN embedding_jobs.last_attempt ELSE NULL END,
                           last_error = CASE
                             WHEN embedding_jobs.input_hash = excluded.input_hash
                             THEN embedding_jobs.last_error ELSE NULL END,
                           terminal = CASE
                             WHEN embedding_jobs.input_hash = excluded.input_hash
                             THEN embedding_jobs.terminal ELSE 0 END",
                        rusqlite::params![
                            generation_id,
                            content_uuid,
                            document.input_hash,
                            document.text,
                            document.header,
                            source.server_seq
                        ],
                    )?;
                }
                let stale = {
                    let mut statement = transaction.prepare(
                        "SELECT DISTINCT content_uuid FROM generation_vectors
                         WHERE generation_id = ?1",
                    )?;
                    statement
                        .query_map([generation_id], |row| row.get::<_, uuid::Uuid>(0))?
                        .filter_map(|row| match row {
                            Ok(uuid) if !present.contains(&uuid) => Some(Ok(uuid)),
                            Ok(_) => None,
                            Err(error) => Some(Err(error)),
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };
                for content_uuid in stale {
                    delete_document_vectors(
                        &transaction,
                        &table_name,
                        generation_id,
                        content_uuid,
                    )?;
                }
                let stale_jobs = {
                    let mut statement = transaction.prepare(
                        "SELECT content_uuid FROM embedding_jobs WHERE generation_id = ?1",
                    )?;
                    statement
                        .query_map([generation_id], |row| row.get::<_, uuid::Uuid>(0))?
                        .filter_map(|row| match row {
                            Ok(uuid) if !present.contains(&uuid) => Some(Ok(uuid)),
                            Ok(_) => None,
                            Err(error) => Some(Err(error)),
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };
                for content_uuid in stale_jobs {
                    transaction.execute(
                        "DELETE FROM embedding_jobs
                         WHERE generation_id = ?1 AND content_uuid = ?2",
                        rusqlite::params![generation_id, content_uuid],
                    )?;
                }
                transaction.execute(
                    "UPDATE index_generations
                     SET last_reconciled_cursor = ?2, source_documents = ?3
                     WHERE id = ?1",
                    rusqlite::params![generation_id, source.token, source_documents as i64],
                )?;
                activate_generation_if_complete(&transaction, generation_id, source_documents)?;
                transaction.commit()?;
                Ok(())
            })
            .await?;
        Ok(source_documents)
    }

    pub async fn cached_source_document_count(&self) -> Result<u64> {
        let generation_id = self.generation_id;
        self.connection
            .call(move |database| {
                database.query_row(
                    "SELECT source_documents FROM index_generations WHERE id = ?1",
                    [generation_id],
                    |row| row.get::<_, i64>(0).map(|count| count as u64),
                )
            })
            .await
    }

    #[cfg(test)]
    pub(crate) fn reconcile_scan_count(&self) -> usize {
        self.reconcile_scans
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn take_jobs(&self, limit: u32) -> Result<Vec<IndexJob>> {
        let generation_id = self.generation_id;
        self.connection
            .call(move |database| {
                let mut statement = database.prepare(
                    "SELECT content_uuid, input_hash, input_text, input_header
                     FROM embedding_jobs
                     WHERE generation_id = ?1 AND terminal = 0 AND retry_count < ?3
                       AND (last_attempt IS NULL
                            OR unixepoch() - last_attempt >= ?4 * (1 << retry_count))
                     ORDER BY enqueued_at, content_uuid LIMIT ?2",
                )?;
                statement
                    .query_map(
                        rusqlite::params![
                            generation_id,
                            limit as i64,
                            MAX_ATTEMPTS,
                            BACKOFF_BASE_SECS
                        ],
                        |row| {
                            Ok(IndexJob {
                                content_uuid: row.get(0)?,
                                input_hash: row.get(1)?,
                                input_text: row.get(2)?,
                                input_header: row.get(3)?,
                            })
                        },
                    )?
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
    }

    pub async fn reconcile_extractions(
        &self,
        notes: &Connection,
        source_seq: u64,
    ) -> Result<Vec<uuid::Uuid>> {
        let documents = extraction_documents(notes).await?;
        self.connection
            .call(move |database| {
                let transaction = database.transaction()?;
                let completed = {
                    let mut statement = transaction
                        .prepare("SELECT content_uuid, input_hash FROM extraction_state")?;
                    statement
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<Result<HashMap<uuid::Uuid, String>, _>>()?
                };
                let present = documents.keys().copied().collect::<HashSet<_>>();
                for (content_uuid, document) in documents {
                    if completed.get(&content_uuid) == Some(&document.input_hash) {
                        transaction.execute(
                            "DELETE FROM extraction_jobs WHERE content_uuid = ?1",
                            [content_uuid],
                        )?;
                        continue;
                    }
                    transaction.execute(
                        "INSERT INTO extraction_jobs
                           (content_uuid, input_hash, input_text, source_seq, enqueued_at,
                            retry_count, last_attempt, last_error, terminal)
                         VALUES (?1, ?2, ?3, ?4, unixepoch(), 0, NULL, NULL, 0)
                         ON CONFLICT(content_uuid) DO UPDATE SET
                           input_hash = excluded.input_hash,
                           input_text = excluded.input_text,
                           source_seq = excluded.source_seq,
                           enqueued_at = excluded.enqueued_at,
                           retry_count = CASE WHEN extraction_jobs.input_hash = excluded.input_hash
                             THEN extraction_jobs.retry_count ELSE 0 END,
                           last_attempt = CASE WHEN extraction_jobs.input_hash = excluded.input_hash
                             THEN extraction_jobs.last_attempt ELSE NULL END,
                           last_error = CASE WHEN extraction_jobs.input_hash = excluded.input_hash
                             THEN extraction_jobs.last_error ELSE NULL END,
                           terminal = CASE WHEN extraction_jobs.input_hash = excluded.input_hash
                             THEN extraction_jobs.terminal ELSE 0 END",
                        rusqlite::params![
                            content_uuid,
                            document.input_hash,
                            document.text,
                            source_seq as i64
                        ],
                    )?;
                }
                let stale_sources = {
                    let mut statement = transaction.prepare(
                        "SELECT content_uuid FROM extraction_state
                         UNION SELECT source_uuid FROM extraction_edges",
                    )?;
                    statement
                        .query_map([], |row| row.get::<_, uuid::Uuid>(0))?
                        .filter_map(|row| match row {
                            Ok(uuid) if !present.contains(&uuid) => Some(Ok(uuid)),
                            Ok(_) => None,
                            Err(error) => Some(Err(error)),
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };
                let stale_jobs = {
                    let mut statement =
                        transaction.prepare("SELECT content_uuid FROM extraction_jobs")?;
                    statement
                        .query_map([], |row| row.get::<_, uuid::Uuid>(0))?
                        .filter_map(|row| match row {
                            Ok(uuid) if !present.contains(&uuid) => Some(Ok(uuid)),
                            Ok(_) => None,
                            Err(error) => Some(Err(error)),
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };
                for content_uuid in stale_jobs {
                    transaction.execute(
                        "DELETE FROM extraction_jobs WHERE content_uuid = ?1",
                        [content_uuid],
                    )?;
                }
                transaction.commit()?;
                Ok(stale_sources)
            })
            .await
    }

    pub async fn forget_extraction_source(&self, source_uuid: uuid::Uuid) -> Result<()> {
        self.connection
            .call(move |database| {
                let transaction = database.transaction()?;
                transaction.execute(
                    "DELETE FROM extraction_edges WHERE source_uuid = ?1",
                    [source_uuid],
                )?;
                transaction.execute(
                    "DELETE FROM extraction_state WHERE content_uuid = ?1",
                    [source_uuid],
                )?;
                transaction.execute(
                    "DELETE FROM extraction_jobs WHERE content_uuid = ?1",
                    [source_uuid],
                )?;
                delete_orphan_entities(&transaction)?;
                transaction.commit()
            })
            .await
    }

    pub async fn take_extraction_jobs(&self, limit: u32) -> Result<Vec<IndexJob>> {
        self.connection
            .call(move |database| {
                let mut statement = database.prepare(
                    "SELECT content_uuid, input_hash, input_text FROM extraction_jobs
                     WHERE terminal = 0 AND retry_count < 5
                       AND (last_attempt IS NULL
                            OR unixepoch() - last_attempt >= 30 * (1 << retry_count))
                     ORDER BY enqueued_at, content_uuid LIMIT ?1",
                )?;
                statement
                    .query_map([limit], |row| {
                        Ok(IndexJob {
                            content_uuid: row.get(0)?,
                            input_hash: row.get(1)?,
                            input_text: row.get(2)?,
                            input_header: String::new(),
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
    }

    pub async fn record_extraction_failure(
        &self,
        job: &IndexJob,
        error: &str,
        terminal: bool,
    ) -> Result<()> {
        let content_uuid = job.content_uuid;
        let input_hash = job.input_hash.clone();
        let error = error.chars().take(2_000).collect::<String>();
        self.connection
            .call(move |database| {
                database.execute(
                    "UPDATE extraction_jobs
                     SET retry_count = retry_count + 1, last_attempt = unixepoch(),
                         last_error = ?3,
                         terminal = CASE WHEN ?4 THEN 1
                           WHEN retry_count + 1 >= 5 THEN 1 ELSE 0 END
                     WHERE content_uuid = ?1 AND input_hash = ?2",
                    rusqlite::params![content_uuid, input_hash, error, terminal],
                )?;
                Ok(())
            })
            .await
    }

    pub async fn extraction_job_is_current(
        &self,
        notes: &Connection,
        job: &IndexJob,
    ) -> Result<bool> {
        let content = notes_core::db::get_content(notes, job.content_uuid).await?;
        Ok(content.is_some_and(|content| {
            extraction_input_for_content(&content).input_hash == job.input_hash
        }))
    }

    pub async fn finish_extraction(
        &self,
        job: &IndexJob,
        entities: Vec<ExtractedEntityRecord>,
        edges: Vec<ExtractedEdge>,
    ) -> Result<()> {
        let source_uuid = job.content_uuid;
        let input_hash = job.input_hash.clone();
        self.connection
            .call(move |database| {
                let transaction = database.transaction()?;
                let current = transaction
                    .query_row(
                        "SELECT source_seq FROM extraction_jobs
                         WHERE content_uuid = ?1 AND input_hash = ?2",
                        rusqlite::params![source_uuid, input_hash],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                let Some(source_seq) = current else {
                    return Ok(());
                };
                transaction.execute(
                    "DELETE FROM extraction_edges WHERE source_uuid = ?1",
                    [source_uuid],
                )?;
                for entity in &entities {
                    transaction.execute(
                        "INSERT INTO entities(uuid, name, normalized_name, description, updated_at)
                         VALUES (?1, ?2, ?3, ?4, unixepoch())
                         ON CONFLICT(uuid) DO UPDATE SET
                           name = excluded.name,
                           normalized_name = excluded.normalized_name,
                           description = CASE WHEN excluded.description = ''
                             THEN entities.description ELSE excluded.description END,
                           updated_at = excluded.updated_at",
                        rusqlite::params![
                            entity.uuid,
                            entity.name,
                            entity.normalized_name,
                            entity.description,
                        ],
                    )?;
                }
                for edge in &edges {
                    transaction.execute(
                        "INSERT INTO extraction_edges(source_uuid, src_uuid, dst_uuid, kind)
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![source_uuid, edge.src_uuid, edge.dst_uuid, edge.kind],
                    )?;
                }
                delete_orphan_entities(&transaction)?;
                transaction.execute(
                    "INSERT INTO extraction_state(content_uuid, input_hash, source_seq)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(content_uuid) DO UPDATE SET
                       input_hash = excluded.input_hash, source_seq = excluded.source_seq",
                    rusqlite::params![source_uuid, input_hash, source_seq],
                )?;
                transaction.execute(
                    "DELETE FROM extraction_jobs WHERE content_uuid = ?1 AND input_hash = ?2",
                    rusqlite::params![source_uuid, input_hash],
                )?;
                transaction.commit()?;
                Ok(())
            })
            .await
    }

    pub async fn record_failure(
        &self,
        jobs: Vec<(uuid::Uuid, String)>,
        error: &str,
        terminal: bool,
    ) -> Result<()> {
        let generation_id = self.generation_id;
        let error = error.chars().take(2_000).collect::<String>();
        self.connection
            .call(move |database| {
                let transaction = database.transaction()?;
                for (content_uuid, input_hash) in jobs {
                    transaction.execute(
                        "UPDATE embedding_jobs
                         SET retry_count = retry_count + 1, last_attempt = unixepoch(),
                             last_error = ?4, terminal = ?5
                         WHERE generation_id = ?1 AND content_uuid = ?2 AND input_hash = ?3",
                        rusqlite::params![generation_id, content_uuid, input_hash, error, terminal],
                    )?;
                }
                transaction.commit()
            })
            .await
    }

    pub async fn write_embeddings(
        &self,
        source_documents: u64,
        documents: Vec<EmbeddedDocument>,
    ) -> Result<()> {
        let generation_id = self.generation_id;
        let dimensions = self.dimensions;
        let table_name = self.table_name.clone();
        self.connection
            .call(move |database| {
                let transaction = database.transaction()?;
                for document in documents {
                    if document.chunks.is_empty() {
                        return Err(rusqlite::Error::InvalidParameterName(format!(
                            "embedding for {} has no chunks",
                            document.content_uuid
                        )));
                    }
                    for (expected_index, chunk) in document.chunks.iter().enumerate() {
                        if chunk.chunk_index != expected_index as u32 {
                            return Err(rusqlite::Error::InvalidParameterName(format!(
                                "embedding chunks for {} are not contiguous",
                                document.content_uuid
                            )));
                        }
                        if chunk.embedding.len() != dimensions {
                            return Err(rusqlite::Error::InvalidParameterName(format!(
                                "embedding for {} chunk {} has dimension {}, expected {dimensions}",
                                document.content_uuid,
                                chunk.chunk_index,
                                chunk.embedding.len()
                            )));
                        }
                    }
                    let still_current = transaction
                        .query_row(
                            "SELECT source_seq FROM embedding_jobs
                             WHERE generation_id = ?1 AND content_uuid = ?2 AND input_hash = ?3",
                            rusqlite::params![
                                generation_id,
                                document.content_uuid,
                                document.input_hash
                            ],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()?;
                    let Some(source_seq) = still_current else {
                        continue;
                    };
                    delete_document_vectors(
                        &transaction,
                        &table_name,
                        generation_id,
                        document.content_uuid,
                    )?;
                    for chunk in document.chunks {
                        let rowid = next_vector_rowid(&transaction, generation_id)?;
                        let blob = chunk
                            .embedding
                            .iter()
                            .flat_map(|value| value.to_le_bytes())
                            .collect::<Vec<_>>();
                        transaction.execute(
                            &format!("INSERT INTO {table_name}(rowid, embedding) VALUES (?1, ?2)"),
                            rusqlite::params![rowid, blob],
                        )?;
                        transaction.execute(
                            "INSERT INTO generation_vectors
                               (generation_id, content_uuid, chunk_index, chunk_text,
                                vector_rowid, input_hash, source_seq)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            rusqlite::params![
                                generation_id,
                                document.content_uuid,
                                i64::from(chunk.chunk_index),
                                chunk.input_text,
                                rowid,
                                document.input_hash,
                                source_seq
                            ],
                        )?;
                    }
                    transaction.execute(
                        "DELETE FROM embedding_jobs
                         WHERE generation_id = ?1 AND content_uuid = ?2 AND input_hash = ?3",
                        rusqlite::params![
                            generation_id,
                            document.content_uuid,
                            document.input_hash
                        ],
                    )?;
                }
                activate_generation_if_complete(&transaction, generation_id, source_documents)?;
                transaction.commit()
            })
            .await
    }

    pub async fn status(&self, source_documents: u64) -> Result<IndexStatus> {
        let generation_id = self.generation_id;
        let identity = self.identity.clone();
        let dimensions = self.dimensions;
        self.connection
            .call(move |database| {
                let status = database.query_row(
                    "SELECT status FROM index_generations WHERE id = ?1",
                    [generation_id],
                    |row| row.get::<_, String>(0),
                )?;
                let control = database.query_row(
                    "SELECT automatic_embeddings, entity_extraction, query_rewriting
                     FROM index_control WHERE singleton = 1",
                    [],
                    |row| {
                        Ok(AiControl {
                            automatic_embeddings: row.get(0)?,
                            entity_extraction: row.get(1)?,
                            query_rewriting: row.get(2)?,
                        })
                    },
                )?;
                let (pending, failed) = database.query_row(
                    "SELECT COUNT(*), COALESCE(SUM(retry_count > 0), 0)
                     FROM embedding_jobs WHERE generation_id = ?1",
                    [generation_id],
                    |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
                )?;
                let indexed = database.query_row(
                    "SELECT COUNT(DISTINCT content_uuid) FROM generation_vectors
                     WHERE generation_id = ?1",
                    [generation_id],
                    |row| row.get::<_, i64>(0).map(|value| value as u64),
                )?;
                let (extraction_pending, extraction_failed) = database.query_row(
                    "SELECT COUNT(*), COALESCE(SUM(retry_count > 0), 0)
                     FROM extraction_jobs",
                    [],
                    |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
                )?;
                Ok(IndexStatus {
                    identity,
                    dimensions,
                    generation_id,
                    generation_status: GenerationStatus::from_database(&status).map_err(
                        |error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                error.into(),
                            )
                        },
                    )?,
                    control,
                    pending,
                    failed,
                    indexed,
                    source_documents,
                    extraction_pending,
                    extraction_failed,
                })
            })
            .await
    }
}

fn delete_orphan_entities(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute(
        "DELETE FROM entities
          WHERE NOT EXISTS (
            SELECT 1 FROM extraction_edges
             WHERE src_uuid = entities.uuid OR dst_uuid = entities.uuid
          )",
        [],
    )?;
    Ok(())
}

fn activate_generation_if_complete(
    transaction: &rusqlite::Transaction<'_>,
    generation_id: uuid::Uuid,
    source_documents: u64,
) -> rusqlite::Result<()> {
    let pending = transaction.query_row(
        "SELECT COUNT(*) FROM embedding_jobs WHERE generation_id = ?1",
        [generation_id],
        |row| row.get::<_, i64>(0).map(|value| value as u64),
    )?;
    let indexed = transaction.query_row(
        "SELECT COUNT(DISTINCT content_uuid) FROM generation_vectors
         WHERE generation_id = ?1",
        [generation_id],
        |row| row.get::<_, i64>(0).map(|value| value as u64),
    )?;
    if pending != 0 || indexed != source_documents {
        return Ok(());
    }
    transaction.execute(
        "UPDATE index_generations SET status = 'retired'
         WHERE status = 'active' AND id != ?1",
        [generation_id],
    )?;
    transaction.execute(
        "UPDATE index_generations
         SET status = 'active', activated_at = unixepoch() WHERE id = ?1",
        [generation_id],
    )?;
    Ok(())
}

#[async_trait]
impl VectorStore for AiStore {
    async fn search(&self, embedding: Vec<f32>, limit: u32) -> Result<Vec<VectorMatch>> {
        if embedding.len() != self.dimensions {
            bail!(
                "query embedding has dimension {}, expected {}",
                embedding.len(),
                self.dimensions
            );
        }
        let generation_id = self.generation_id;
        let table_name = self.table_name.clone();
        let blob = embedding
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        self.connection
            .call(move |database| {
                let active = database.query_row(
                    "SELECT status = 'active' FROM index_generations WHERE id = ?1",
                    [generation_id],
                    |row| row.get::<_, bool>(0),
                )?;
                if !active {
                    return Ok(Vec::new());
                }
                let mut statement = database.prepare(&format!(
                    "SELECT mapping.content_uuid, vectors.distance,
                            mapping.chunk_index, mapping.chunk_text
                     FROM {table_name} vectors
                     JOIN generation_vectors mapping
                       ON mapping.generation_id = ?3
                      AND mapping.vector_rowid = vectors.rowid
                     WHERE vectors.embedding MATCH ?1 AND k = ?2
                     ORDER BY vectors.distance"
                ))?;
                statement
                    .query_map(
                        rusqlite::params![blob, limit as i64, generation_id],
                        |row| {
                            Ok(VectorMatch {
                                content_uuid: row.get(0)?,
                                distance: row.get(1)?,
                                chunk_index: row.get(2)?,
                                chunk_text: row.get(3)?,
                            })
                        },
                    )?
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
    }
}

struct IndexDocument {
    text: String,
    header: String,
    input_hash: String,
    kind: IndexedContentKind,
}

struct IndexSourceCursor {
    server_seq: i64,
    token: String,
}

async fn index_source_cursor(notes: &Connection) -> Result<IndexSourceCursor> {
    // `last_server_seq` alone does not move for local edits. Pair it with the operation clock so
    // both local writes and newly applied remote writes invalidate the full-corpus reconcile.
    let (server_seq, last_hlc) = notes
        .call(|database| {
            database.query_row(
                "SELECT
                   COALESCE((SELECT value FROM sync_meta WHERE key = 'last_server_seq'), '0'),
                   COALESCE((SELECT value FROM sync_meta WHERE key = 'last_hlc'), '')",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
        })
        .await?;
    let server_seq = server_seq
        .parse::<u64>()
        .context("invalid last_server_seq in sync_meta")?;
    let server_seq = i64::try_from(server_seq).context("sync cursor exceeds SQLite range")?;
    Ok(IndexSourceCursor {
        server_seq,
        token: format!("{server_seq}:{last_hlc}"),
    })
}

const ANCESTOR_EXCERPT_CHARS: usize = 150;

struct DocumentChainRow {
    content_uuid: uuid::Uuid,
    depth: i32,
    page_title: Option<String>,
    journal_date: Option<String>,
    block_style: BlockStyle,
    section_heading: Option<String>,
    content: String,
}

struct PagePreviewRow {
    page_uuid: uuid::Uuid,
    page_title: Option<String>,
    journal_date: Option<String>,
    block_style: Option<BlockStyle>,
    content: Option<String>,
}

struct CompositionInput {
    page_title: Option<String>,
    journal_date: Option<String>,
    block_style: BlockStyle,
    section_heading: Option<String>,
    body: String,
    ancestors: Vec<(i32, String)>,
}

async fn index_documents(notes: &Connection) -> Result<BTreeMap<uuid::Uuid, IndexDocument>> {
    let (rows, page_rows) = notes
        .call(|database| {
            let transaction = database.transaction()?;
            let rows = {
                let mut statement = transaction.prepare(
                    "WITH RECURSIVE chain(
                   qid, parent_uuid, page_title, journal_date, block_style,
                   section_heading, content, depth
                 ) AS (
                   SELECT block.uuid, block.parent_uuid, page.title,
                          identity.journal_date, block.style,
                          CASE WHEN page.layout = 'document' THEN (
                            SELECT heading.markdown FROM blocks heading
                             WHERE heading.page_uuid = block.page_uuid
                               AND heading.parent_uuid IS block.parent_uuid
                               AND heading.order_key < block.order_key
                               AND heading.style IN ('heading_1', 'heading_2', 'heading_3')
                             ORDER BY heading.order_key DESC, heading.uuid DESC
                             LIMIT 1
                          ) END,
                          block.markdown, 0
                     FROM blocks block
                     JOIN pages page ON page.uuid = block.page_uuid
                     JOIN page_identities identity ON identity.page_uuid = page.uuid
                   UNION ALL
                   SELECT chain.qid, parent.parent_uuid, chain.page_title,
                          chain.journal_date, chain.block_style, chain.section_heading,
                          parent.markdown, chain.depth + 1
                     FROM chain JOIN blocks parent ON parent.uuid = chain.parent_uuid
                 )
                 SELECT qid, depth, page_title, journal_date, block_style,
                        section_heading, content
                   FROM chain ORDER BY qid, depth",
                )?;
                statement
                    .query_map([], |row| {
                        Ok(DocumentChainRow {
                            content_uuid: row.get(0)?,
                            depth: row.get(1)?,
                            page_title: row.get(2)?,
                            journal_date: row.get(3)?,
                            block_style: row.get(4)?,
                            section_heading: row.get(5)?,
                            content: row.get(6)?,
                        })
                    })?
                    .collect::<Result<Vec<DocumentChainRow>, _>>()?
            };
            let page_rows = {
                let mut statement = transaction.prepare(
                    "WITH RECURSIVE page_tree(
                       page_uuid, uuid, path, block_style, content
                     ) AS (
                       SELECT page_uuid, uuid, order_key, style, markdown
                         FROM blocks WHERE parent_uuid IS NULL
                       UNION ALL
                       SELECT child.page_uuid, child.uuid,
                              page_tree.path || '/' || child.order_key,
                              child.style, child.markdown
                         FROM page_tree JOIN blocks child
                           ON child.parent_uuid = page_tree.uuid
                     )
                     SELECT page.uuid, page.title, identity.journal_date,
                            page_tree.block_style, page_tree.content
                       FROM pages page
                       JOIN page_identities identity ON identity.page_uuid = page.uuid
                       LEFT JOIN page_tree ON page_tree.page_uuid = page.uuid
                      ORDER BY page.uuid, page_tree.path, page_tree.uuid",
                )?;
                statement
                    .query_map([], |row| {
                        Ok(PagePreviewRow {
                            page_uuid: row.get(0)?,
                            page_title: row.get(1)?,
                            journal_date: row.get(2)?,
                            block_style: row.get(3)?,
                            content: row.get(4)?,
                        })
                    })?
                    .collect::<Result<Vec<PagePreviewRow>, _>>()?
            };
            transaction.commit()?;
            Ok((rows, page_rows))
        })
        .await?;
    let mut inputs = BTreeMap::<uuid::Uuid, CompositionInput>::new();
    for row in rows {
        let input = inputs
            .entry(row.content_uuid)
            .or_insert_with(|| CompositionInput {
                page_title: row.page_title,
                journal_date: row.journal_date,
                block_style: row.block_style,
                section_heading: row.section_heading,
                body: String::new(),
                ancestors: Vec::new(),
            });
        if row.depth == 0 {
            input.body = row.content;
        } else {
            input.ancestors.push((row.depth, row.content));
        }
    }
    let mut documents = inputs
        .into_iter()
        .filter_map(|(uuid, input)| {
            let text = compose_text(&input)?;
            let header = composition_header(&input);
            Some((
                uuid,
                index_document(text, header, IndexedContentKind::Block),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let mut current_page = None;
    let mut preview_rows = Vec::new();
    for row in page_rows {
        if current_page != Some(row.page_uuid) {
            if let Some(page_uuid) = current_page
                && let Some(text) = compose_page_preview(&preview_rows)
            {
                documents.insert(
                    page_uuid,
                    index_document(text, String::new(), IndexedContentKind::Page),
                );
            }
            current_page = Some(row.page_uuid);
            preview_rows.clear();
        }
        preview_rows.push(row);
    }
    if let Some(page_uuid) = current_page
        && let Some(text) = compose_page_preview(&preview_rows)
    {
        documents.insert(
            page_uuid,
            index_document(text, String::new(), IndexedContentKind::Page),
        );
    }
    Ok(documents)
}

pub async fn inspect_index(
    notes: &Connection,
    selected_uuid: Option<uuid::Uuid>,
) -> Result<IndexInspection> {
    let documents = index_documents(notes).await?;
    let total_blocks = notes
        .call(|database| {
            database.query_row("SELECT COUNT(*) FROM blocks", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .await? as usize;
    let mut histogram = [0_usize; 7];
    let mut composed_chars = 0;
    let mut provider_chars = 0;
    let mut tier_two_blocks = 0;
    let mut block_documents = 0;
    let mut longest = Vec::with_capacity(documents.len());
    for (content_uuid, document) in &documents {
        let chars = document.text.chars().count();
        composed_chars += chars;
        histogram[length_bucket(chars)] += 1;
        let chunks = split_for_embedding(&document.text, &document.header);
        provider_chars += chunks
            .iter()
            .map(|chunk| chunk.text.chars().count())
            .sum::<usize>();
        if document.kind == IndexedContentKind::Block {
            block_documents += 1;
            tier_two_blocks += usize::from(chars > SINGLE_VECTOR_MAX_CHARS);
        }
        longest.push(IndexInspectionItem {
            content_uuid: *content_uuid,
            kind: document.kind,
            composed_chars: chars,
        });
    }
    longest.sort_by(|left, right| {
        right
            .composed_chars
            .cmp(&left.composed_chars)
            .then_with(|| left.content_uuid.cmp(&right.content_uuid))
    });
    longest.truncate(20);
    let selected = selected_uuid.and_then(|content_uuid| {
        let document = documents.get(&content_uuid)?;
        Some(InspectedDocument {
            item: IndexInspectionItem {
                content_uuid,
                kind: document.kind,
                composed_chars: document.text.chars().count(),
            },
            composed_text: document.text.clone(),
            chunks: split_for_embedding(&document.text, &document.header),
        })
    });
    Ok(IndexInspection {
        document_count: documents.len(),
        skipped_content_free_blocks: total_blocks.saturating_sub(block_documents),
        tier_two_blocks,
        composed_chars,
        estimated_tokens: provider_chars.div_ceil(4),
        histogram: [
            "0-255",
            "256-511",
            "512-1023",
            "1024-1999",
            "2000-3999",
            "4000-7999",
            "8000+",
        ]
        .into_iter()
        .zip(histogram)
        .map(|(label, count)| IndexLengthBucket { label, count })
        .collect(),
        longest,
        selected,
    })
}

fn length_bucket(chars: usize) -> usize {
    match chars {
        0..=255 => 0,
        256..=511 => 1,
        512..=1_023 => 2,
        1_024..=1_999 => 3,
        2_000..=3_999 => 4,
        4_000..=7_999 => 5,
        _ => 6,
    }
}

fn index_document(text: String, header: String, kind: IndexedContentKind) -> IndexDocument {
    let mut digest = Sha256::new();
    digest.update(INPUT_FORMAT_VERSION.to_le_bytes());
    digest.update(text.as_bytes());
    IndexDocument {
        input_hash: format!("{:x}", digest.finalize()),
        text,
        header,
        kind,
    }
}

fn compose_page_preview(rows: &[PagePreviewRow]) -> Option<String> {
    let first = rows.first()?;
    let mut parts = Vec::new();
    if let Some(title) = page_heading(first.page_title.as_deref(), first.journal_date.as_deref()) {
        push_page_preview_part(&mut parts, &title);
    }
    for row in rows {
        let (Some(style), Some(content)) = (row.block_style, row.content.as_deref()) else {
            continue;
        };
        if !has_meaningful_content(style, content) {
            continue;
        }
        let content = normalize_whitespace(content);
        if content.is_empty() || !push_page_preview_part(&mut parts, &content) {
            break;
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn push_page_preview_part(parts: &mut Vec<String>, value: &str) -> bool {
    let used = parts.iter().map(|part| part.chars().count()).sum::<usize>()
        + parts.len().saturating_sub(1);
    let separator = usize::from(!parts.is_empty());
    let available = PAGE_VECTOR_MAX_CHARS.saturating_sub(used + separator);
    if available == 0 {
        return false;
    }
    let complete = value.chars().count() <= available;
    parts.push(if complete {
        value.to_owned()
    } else {
        truncate_excerpt(value, available)
    });
    complete
}

fn page_heading(title: Option<&str>, journal_date: Option<&str>) -> Option<String> {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            journal_date
                .map(str::trim)
                .filter(|date| !date.is_empty())
                .map(|date| format!("Journal {date}"))
        })
}

async fn extraction_documents(notes: &Connection) -> Result<BTreeMap<uuid::Uuid, IndexDocument>> {
    let rows = notes
        .call(|database| {
            let mut rows = Vec::new();
            {
                let mut statement = database.prepare("SELECT uuid, title FROM pages")?;
                rows.extend(
                    statement
                        .query_map([], |row| {
                            Ok((
                                row.get::<_, uuid::Uuid>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                String::new(),
                            ))
                        })?
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            {
                let mut statement = database.prepare("SELECT uuid, markdown FROM blocks")?;
                rows.extend(
                    statement
                        .query_map([], |row| {
                            Ok((row.get::<_, uuid::Uuid>(0)?, None, row.get::<_, String>(1)?))
                        })?
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            Ok(rows)
        })
        .await?;
    Ok(rows
        .into_iter()
        .map(|(uuid, title, content)| (uuid, extraction_input(title.as_deref(), &content)))
        .collect())
}

fn extraction_input_for_content(content: &notes_core::db::Content) -> IndexDocument {
    match content {
        notes_core::db::Content::Page(page) => extraction_input(page.title.as_deref(), ""),
        notes_core::db::Content::Block(block) => extraction_input(None, &block.markdown),
    }
}

fn extraction_input(title: Option<&str>, content: &str) -> IndexDocument {
    let text = format!("{}\n{content}", title.unwrap_or_default());
    let mut digest = Sha256::new();
    digest.update(b"notes-rs:extraction-input:v1\0");
    digest.update(text.as_bytes());
    IndexDocument {
        text,
        header: String::new(),
        input_hash: format!("{:x}", digest.finalize()),
        kind: IndexedContentKind::Block,
    }
}

fn compose_text(input: &CompositionInput) -> Option<String> {
    if !has_meaningful_content(input.block_style, &input.body) {
        return None;
    }
    let header = composition_header(input);
    let body = input.body.trim();
    Some(if header.is_empty() {
        body.to_owned()
    } else {
        format!("{header}\n{body}")
    })
}

fn composition_header(input: &CompositionInput) -> String {
    let mut parts = Vec::new();
    if let Some(heading) = page_heading(input.page_title.as_deref(), input.journal_date.as_deref())
    {
        parts.push(heading);
    }
    if let Some(section_heading) = input
        .section_heading
        .as_deref()
        .map(normalize_whitespace)
        .filter(|heading| !heading.is_empty())
    {
        parts.push(section_heading);
    }
    let mut ancestors = input.ancestors.iter().collect::<Vec<_>>();
    ancestors.sort_by_key(|(depth, _)| std::cmp::Reverse(*depth));
    parts.extend(
        ancestors
            .into_iter()
            .map(|(_, content)| truncate_excerpt(content, ANCESTOR_EXCERPT_CHARS))
            .filter(|excerpt| !excerpt.is_empty()),
    );
    parts.join("\n")
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_excerpt(value: &str, max_chars: usize) -> String {
    let normalized = normalize_whitespace(value);
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    if max_chars == 0 {
        return String::new();
    }
    let content_limit = max_chars.saturating_sub(1);
    let byte_limit = normalized
        .char_indices()
        .nth(content_limit)
        .map_or(normalized.len(), |(index, _)| index);
    let prefix = &normalized[..byte_limit];
    let boundary = prefix.rfind(char::is_whitespace).unwrap_or(byte_limit);
    format!("{}…", normalized[..boundary].trim_end())
}

fn has_meaningful_content(style: BlockStyle, markdown: &str) -> bool {
    if style == BlockStyle::Divider {
        return false;
    }
    let mut code_block_depth = 0_u32;
    for event in Parser::new_ext(markdown, Options::all()) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code_block_depth += 1,
            Event::End(TagEnd::CodeBlock) => code_block_depth = code_block_depth.saturating_sub(1),
            Event::Text(text) if code_block_depth > 0 => {
                if text.chars().any(|character| !character.is_whitespace()) {
                    return true;
                }
            }
            Event::Text(text) => {
                if text.chars().any(|character| {
                    character.is_alphanumeric()
                        || (!character.is_ascii()
                            && !character.is_punctuation()
                            && !character.is_separator())
                }) {
                    return true;
                }
            }
            Event::Code(code) | Event::InlineMath(code) | Event::DisplayMath(code)
                if code.chars().any(|character| !character.is_whitespace()) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn vector_table_name(generation_id: uuid::Uuid) -> String {
    format!("vec_{}", generation_id.simple())
}

async fn ensure_vector_table(
    connection: &Connection,
    table_name: &str,
    dimensions: usize,
) -> Result<()> {
    let table_name = table_name.to_owned();
    connection
        .call(move |database| {
            database.execute_batch(&format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS {table_name}
                 USING vec0(embedding float[{dimensions}]);"
            ))
        })
        .await
        .context("creating generation vector table")
}

fn next_vector_rowid(
    transaction: &rusqlite::Transaction<'_>,
    generation_id: uuid::Uuid,
) -> rusqlite::Result<i64> {
    transaction.query_row(
        "SELECT COALESCE(MAX(vector_rowid), 0) + 1
         FROM generation_vectors WHERE generation_id = ?1",
        [generation_id],
        |row| row.get(0),
    )
}

fn delete_document_vectors(
    transaction: &rusqlite::Transaction<'_>,
    table_name: &str,
    generation_id: uuid::Uuid,
    content_uuid: uuid::Uuid,
) -> rusqlite::Result<()> {
    let rowids = {
        let mut statement = transaction.prepare(
            "SELECT vector_rowid FROM generation_vectors
             WHERE generation_id = ?1 AND content_uuid = ?2",
        )?;
        statement
            .query_map(rusqlite::params![generation_id, content_uuid], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for rowid in rowids {
        transaction.execute(
            &format!("DELETE FROM {table_name} WHERE rowid = ?1"),
            [rowid],
        )?;
    }
    transaction.execute(
        "DELETE FROM generation_vectors
         WHERE generation_id = ?1 AND content_uuid = ?2",
        rusqlite::params![generation_id, content_uuid],
    )?;
    Ok(())
}

pub fn embedding_identity_fingerprint(endpoint: &str, model: &str, dimensions: usize) -> String {
    let mut digest = Sha256::new();
    digest.update(b"notes-rs:embedding-identity:v1\0");
    digest.update(endpoint.trim_end_matches('/').as_bytes());
    digest.update([0]);
    digest.update(model.as_bytes());
    digest.update([0]);
    digest.update(dimensions.to_le_bytes());
    digest.update([0]);
    digest.update(b"cosine:provider-normalized:input-v1");
    format!("sha256:{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notes_core::{BlockStyle, PageLayout};

    fn composition_fixture(body: &str) -> CompositionInput {
        CompositionInput {
            page_title: Some("Project Atlas".into()),
            journal_date: None,
            block_style: BlockStyle::Paragraph,
            section_heading: None,
            body: body.into(),
            ancestors: Vec::new(),
        }
    }

    fn one_chunk_embedding(job: &IndexJob, embedding: Vec<f32>) -> EmbeddedDocument {
        EmbeddedDocument {
            content_uuid: job.content_uuid,
            input_hash: job.input_hash.clone(),
            chunks: vec![EmbeddedChunk {
                chunk_index: 0,
                input_text: job.input_text.clone(),
                embedding,
            }],
        }
    }

    #[test]
    fn composition_golden_keeps_the_frozen_header_order() {
        let mut input = composition_fixture("Ship the embedding pipeline.");
        input.ancestors = vec![
            (1, "  Parent   planning notes ".into()),
            (2, "Root objective".into()),
        ];

        assert_eq!(
            compose_text(&input).as_deref(),
            Some(
                "Project Atlas\nRoot objective\nParent planning notes\nShip the embedding pipeline."
            )
        );
    }

    #[test]
    fn journal_composition_uses_an_iso_pseudo_title() {
        let mut input = composition_fixture("Captured thought");
        input.page_title = None;
        input.journal_date = Some("2026-07-19".into());

        assert_eq!(
            compose_text(&input).as_deref(),
            Some("Journal 2026-07-19\nCaptured thought")
        );
    }

    #[test]
    fn ancestor_excerpts_render_root_to_parent_and_stop_at_a_word_boundary() {
        let long_tail = format!("{} secondword thirdword", "a".repeat(140));
        let mut input = composition_fixture("Leaf");
        input.ancestors = vec![(1, long_tail), (3, "Root".into()), (2, "Middle".into())];

        let text = compose_text(&input).expect("meaningful composition");
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines[1], "Root");
        assert_eq!(lines[2], "Middle");
        assert_eq!(lines[3], format!("{}…", "a".repeat(140)));
        assert!(!lines[3].contains("secondwor"));
    }

    #[test]
    fn content_free_blocks_are_skipped_but_short_semantic_content_is_kept() {
        let punctuation = composition_fixture("*** — []() <br>");
        assert!(compose_text(&punctuation).is_none());

        let mut divider = composition_fixture("horizontal divider");
        divider.block_style = BlockStyle::Divider;
        assert!(compose_text(&divider).is_none());

        let emoji = composition_fixture("🧭");
        assert_eq!(compose_text(&emoji).as_deref(), Some("Project Atlas\n🧭"));
        let short = composition_fixture("x");
        assert_eq!(compose_text(&short).as_deref(), Some("Project Atlas\nx"));
    }

    #[test]
    fn page_preview_stops_at_the_shared_character_budget() {
        let rows = vec![
            PagePreviewRow {
                page_uuid: uuid::Uuid::nil(),
                page_title: Some("Budgeted page".into()),
                journal_date: None,
                block_style: Some(BlockStyle::Paragraph),
                content: Some("semantic ".repeat(300)),
            },
            PagePreviewRow {
                page_uuid: uuid::Uuid::nil(),
                page_title: Some("Budgeted page".into()),
                journal_date: None,
                block_style: Some(BlockStyle::Paragraph),
                content: Some("must not appear".into()),
            },
        ];

        let preview = compose_page_preview(&rows).expect("page preview");
        assert!(preview.chars().count() <= PAGE_VECTOR_MAX_CHARS);
        assert!(preview.chars().count() > PAGE_VECTOR_MAX_CHARS - 16);
        assert!(preview.starts_with("Budgeted page\nsemantic"));
        assert!(preview.ends_with('…'));
        assert!(!preview.contains("must not appear"));
    }

    #[tokio::test]
    async fn page_preview_uses_full_tree_preorder() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Preorder".into())
            .await
            .expect("create page");
        let root = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            BlockStyle::Paragraph,
            "root".into(),
        )
        .await
        .expect("create root");
        notes_core::db::create_block(
            &notes,
            page.uuid,
            Some(root.uuid),
            None,
            BlockStyle::Paragraph,
            "child".into(),
        )
        .await
        .expect("create child");
        notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            Some(root.uuid),
            BlockStyle::Paragraph,
            "sibling".into(),
        )
        .await
        .expect("create sibling");

        let documents = index_documents(&notes).await.expect("compose documents");
        assert_eq!(documents[&page.uuid].text, "Preorder\nroot\nchild\nsibling");
    }

    #[tokio::test]
    async fn index_inspection_reports_skips_tier_two_and_exact_chunks() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Inspection".into())
            .await
            .expect("create page");
        let long = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            BlockStyle::Paragraph,
            "Ж".repeat(3_000),
        )
        .await
        .expect("create long block");
        notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            Some(long.uuid),
            BlockStyle::Paragraph,
            "*** — []()".into(),
        )
        .await
        .expect("create content-free block");

        let report = inspect_index(&notes, Some(long.uuid))
            .await
            .expect("inspect index");
        assert_eq!(report.document_count, 2);
        assert_eq!(report.skipped_content_free_blocks, 1);
        assert_eq!(report.tier_two_blocks, 1);
        assert_eq!(
            report
                .histogram
                .iter()
                .map(|bucket| bucket.count)
                .sum::<usize>(),
            2
        );
        assert_eq!(report.longest[0].content_uuid, long.uuid);
        let selected = report.selected.expect("selected block");
        assert_eq!(selected.item.kind, IndexedContentKind::Block);
        assert_eq!(
            selected.composed_text,
            format!("Inspection\n{}", "Ж".repeat(3_000))
        );
        assert!(selected.chunks.len() > 1);
        assert!(report.estimated_tokens > 0);
    }

    #[tokio::test]
    async fn document_composition_uses_only_the_nearest_preceding_heading() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Document".into())
            .await
            .expect("create page");
        notes_core::db::set_page_layout(&notes, page.uuid, PageLayout::Document)
            .await
            .expect("set document layout");
        let before = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            BlockStyle::Paragraph,
            "Before heading".into(),
        )
        .await
        .expect("create leading paragraph");
        let heading = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            Some(before.uuid),
            BlockStyle::Heading2,
            "Deployment".into(),
        )
        .await
        .expect("create heading");
        let after = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            Some(heading.uuid),
            BlockStyle::Paragraph,
            "Roll out gradually".into(),
        )
        .await
        .expect("create trailing paragraph");

        let documents = index_documents(&notes).await.expect("compose documents");
        assert_eq!(documents[&before.uuid].text, "Document\nBefore heading");
        assert_eq!(
            documents[&after.uuid].text,
            "Document\nDeployment\nRoll out gradually"
        );
    }

    #[test]
    fn ai_schema_migration_is_valid() {
        migrations().validate().expect("valid AI migrations");
    }

    #[tokio::test]
    async fn new_store_requires_explicit_opt_in_for_paid_background_work() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");

        assert_eq!(
            store.control().await.expect("AI control"),
            AiControl {
                automatic_embeddings: false,
                entity_extraction: false,
                query_rewriting: true,
            }
        );
    }

    #[test]
    fn identity_changes_with_endpoint_model_or_dimensions() {
        let base = embedding_identity_fingerprint("https://example.test/v1", "embed", 8);
        assert_ne!(
            base,
            embedding_identity_fingerprint("https://other.test/v1", "embed", 8)
        );
        assert_ne!(
            base,
            embedding_identity_fingerprint("https://example.test/v1", "other", 8)
        );
        assert_ne!(
            base,
            embedding_identity_fingerprint("https://example.test/v1", "embed", 16)
        );
    }

    #[tokio::test]
    async fn generation_indexes_uuid_keyed_documents_and_activates_atomically() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Indexed".into())
            .await
            .expect("create page");
        let block = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            BlockStyle::Paragraph,
            "document".into(),
        )
        .await
        .expect("create note");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");
        let source_documents = store.reconcile(&notes).await.expect("reconcile");
        let jobs = store.take_jobs(16).await.expect("jobs");
        assert_eq!(jobs.len(), 2);
        store
            .write_embeddings(
                source_documents,
                jobs.iter()
                    .map(|job| one_chunk_embedding(job, vec![0.25, 0.75]))
                    .collect(),
            )
            .await
            .expect("write embedding");
        let status = store.status(source_documents).await.expect("status");
        assert_eq!(status.generation_status, GenerationStatus::Active);
        assert_eq!(status.indexed, 2);
        let matches = store.search(vec![0.25, 0.75], 4).await.expect("search");
        assert_eq!(matches.len(), 2);
        assert!(
            matches
                .iter()
                .any(|matched| matched.content_uuid == page.uuid)
        );
        assert!(
            matches
                .iter()
                .any(|matched| matched.content_uuid == block.uuid)
        );

        store.reset_index().await.expect("reset index");
        let status = store.status(source_documents).await.expect("reset status");
        assert_eq!(status.generation_status, GenerationStatus::Building);
        assert_eq!(status.indexed, 0);

        let rebuilt_documents = store
            .reconcile(&notes)
            .await
            .expect("reconcile after reset at unchanged cursor");
        assert_eq!(rebuilt_documents, 2);
        assert_eq!(store.take_jobs(16).await.expect("rebuilt jobs").len(), 2);
    }

    #[tokio::test]
    async fn empty_generation_activates_after_reconciliation() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");

        let source_documents = store.reconcile(&notes).await.expect("reconcile");
        let status = store.status(source_documents).await.expect("status");

        assert_eq!(source_documents, 0);
        assert_eq!(status.generation_status, GenerationStatus::Active);
    }

    #[tokio::test]
    async fn title_only_page_is_a_searchable_semantic_document() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Semantic landing page".into())
            .await
            .expect("create title-only page");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");

        let source_documents = store.reconcile(&notes).await.expect("reconcile");
        let jobs = store.take_jobs(16).await.expect("jobs");
        assert_eq!(source_documents, 1);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].content_uuid, page.uuid);
        assert_eq!(jobs[0].input_text, "Semantic landing page");
        store
            .write_embeddings(
                source_documents,
                vec![one_chunk_embedding(&jobs[0], vec![0.25, 0.75])],
            )
            .await
            .expect("write page embedding");

        let matches = store.search(vec![0.25, 0.75], 4).await.expect("search");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].content_uuid, page.uuid);
    }

    #[tokio::test]
    async fn cached_source_count_never_recomposes_documents_for_status() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        notes_core::db::create_page(&notes, "First".into())
            .await
            .expect("create first page");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");

        assert_eq!(store.cached_source_document_count().await.unwrap(), 0);
        assert_eq!(store.reconcile_scan_count(), 0);
        assert_eq!(store.reconcile(&notes).await.unwrap(), 1);
        assert_eq!(store.cached_source_document_count().await.unwrap(), 1);
        assert_eq!(store.reconcile_scan_count(), 1);

        notes_core::db::create_page(&notes, "Second".into())
            .await
            .expect("create second page");
        assert_eq!(store.cached_source_document_count().await.unwrap(), 1);
        assert_eq!(store.reconcile_scan_count(), 1);
    }

    #[tokio::test]
    async fn stale_embedding_result_cannot_replace_a_newer_job() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Before".into())
            .await
            .expect("create page");
        let block = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            BlockStyle::Paragraph,
            "old content".into(),
        )
        .await
        .expect("create note");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");
        let source_documents = store.reconcile(&notes).await.expect("first reconcile");
        let stale_job = store
            .take_jobs(16)
            .await
            .expect("old jobs")
            .into_iter()
            .find(|job| job.content_uuid == block.uuid)
            .expect("old block job");

        notes_core::db::set_block_content(
            &notes,
            block.uuid,
            notes_core::db::BlockContent {
                markdown: "new content".into(),
            },
        )
        .await
        .expect("change note");
        store.reconcile(&notes).await.expect("second reconcile");
        let current_job = store
            .take_jobs(16)
            .await
            .expect("new jobs")
            .into_iter()
            .find(|job| job.content_uuid == block.uuid)
            .expect("new block job");
        assert_ne!(stale_job.input_hash, current_job.input_hash);

        store
            .write_embeddings(
                source_documents,
                vec![one_chunk_embedding(&stale_job, vec![1.0, 0.0])],
            )
            .await
            .expect("ignore stale result");
        let status = store.status(source_documents).await.expect("status");
        assert_eq!(status.indexed, 0);
        assert_eq!(status.pending, 2);
        assert_eq!(status.generation_status, GenerationStatus::Building);
    }

    #[tokio::test]
    async fn extracted_entities_stay_in_ai_storage_and_orphans_are_collected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&notes, "Project".into())
            .await
            .expect("create page");
        let block = notes_core::db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            BlockStyle::Paragraph,
            "Rust powers the project".into(),
        )
        .await
        .expect("create block");
        let store = AiStore::open(directory.path().join("ai.db"), "identity".into(), 2)
            .await
            .expect("AI store");

        store
            .reconcile_extractions(&notes, 1)
            .await
            .expect("reconcile extraction jobs");
        let job = store
            .take_extraction_jobs(16)
            .await
            .expect("extraction jobs")
            .into_iter()
            .find(|job| job.content_uuid == block.uuid)
            .expect("block extraction job");
        let entity_uuid = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"notes-rs:entity:rust");
        store
            .finish_extraction(
                &job,
                vec![ExtractedEntityRecord {
                    uuid: entity_uuid,
                    name: "Rust".into(),
                    normalized_name: "rust".into(),
                    description: "A programming language".into(),
                }],
                vec![ExtractedEdge {
                    src_uuid: block.uuid,
                    dst_uuid: entity_uuid,
                    kind: "mentions".into(),
                }],
            )
            .await
            .expect("finish extraction");

        assert!(
            notes_core::db::get_content(&notes, entity_uuid)
                .await
                .expect("query notes database")
                .is_none(),
            "derived entities must never be written to notes-core"
        );
        let (entities, edges) = store
            .connection
            .call(|database| {
                Ok((
                    database.query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?,
                    database.query_row("SELECT COUNT(*) FROM extraction_edges", [], |row| {
                        row.get(0)
                    })?,
                ))
            })
            .await
            .expect("inspect AI database");
        assert_eq!((entities, edges), (1_i64, 1_i64));
        let listed = store.list_entities(50).await.expect("list entities");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].uuid, entity_uuid);
        assert_eq!(listed[0].name, "Rust");
        assert_eq!(listed[0].description, "A programming language");
        assert_eq!(listed[0].mention_count, 1);

        notes_core::db::set_block_content(
            &notes,
            block.uuid,
            notes_core::db::BlockContent {
                markdown: "Nothing to extract".into(),
            },
        )
        .await
        .expect("update block");
        store
            .reconcile_extractions(&notes, 2)
            .await
            .expect("reconcile changed extraction input");
        let changed_job = store
            .take_extraction_jobs(16)
            .await
            .expect("changed extraction jobs")
            .into_iter()
            .find(|job| job.content_uuid == block.uuid)
            .expect("changed block extraction job");
        store
            .finish_extraction(&changed_job, Vec::new(), Vec::new())
            .await
            .expect("replace extraction with an empty result");
        let entities = store
            .connection
            .call(|database| {
                database.query_row("SELECT COUNT(*) FROM entities", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .await
            .expect("count orphan entities");
        assert_eq!(entities, 0);
        assert!(
            store
                .list_entities(50)
                .await
                .expect("list entities")
                .is_empty()
        );
    }
}
