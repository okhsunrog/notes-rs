use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use notes_core::Connection;
use rusqlite::OptionalExtension;
use rusqlite::ffi::sqlite3_auto_extension;
use rusqlite_migration::{M, Migrations};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Once;

const INPUT_FORMAT_VERSION: u32 = 1;
const MAX_ATTEMPTS: i64 = 8;
const BACKOFF_BASE_SECS: i64 = 5;

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
    Migrations::new(vec![M::up(include_str!(
        "store/migrations/V001__initial.sql"
    ))])
}

#[derive(Debug, Clone)]
pub struct IndexJob {
    pub content_uuid: uuid::Uuid,
    pub input_hash: String,
    pub input_text: String,
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

#[derive(Debug, Clone, Copy)]
pub struct VectorMatch {
    pub content_uuid: uuid::Uuid,
    pub distance: f64,
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
            automatic_embeddings: true,
            entity_extraction: true,
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
                    "UPDATE index_generations SET status = 'building', activated_at = NULL
                     WHERE id = ?1",
                    [generation_id],
                )?;
                transaction.commit()
            })
            .await
    }

    pub async fn reconcile(&self, notes: &Connection, source_seq: u64) -> Result<u64> {
        let documents = index_documents(notes).await?;
        let source_documents = documents.len() as u64;
        let generation_id = self.generation_id;
        let table_name = self.table_name.clone();
        self.connection
            .call(move |database| {
                let transaction = database.transaction()?;
                let current = {
                    let mut statement = transaction.prepare(
                        "SELECT content_uuid, input_hash FROM generation_vectors
                         WHERE generation_id = ?1",
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
                           (generation_id, content_uuid, input_hash, input_text, source_seq,
                            enqueued_at, retry_count, last_attempt, last_error, terminal)
                         VALUES (?1, ?2, ?3, ?4, ?5, unixepoch(), 0, NULL, NULL, 0)
                         ON CONFLICT(generation_id, content_uuid) DO UPDATE SET
                           input_hash = excluded.input_hash,
                           input_text = excluded.input_text,
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
                            source_seq as i64
                        ],
                    )?;
                }
                let stale = {
                    let mut statement = transaction.prepare(
                        "SELECT content_uuid, vector_rowid FROM generation_vectors
                         WHERE generation_id = ?1",
                    )?;
                    statement
                        .query_map([generation_id], |row| {
                            Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, i64>(1)?))
                        })?
                        .filter_map(|row| match row {
                            Ok((uuid, rowid)) if !present.contains(&uuid) => {
                                Some(Ok((uuid, rowid)))
                            }
                            Ok(_) => None,
                            Err(error) => Some(Err(error)),
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };
                for (content_uuid, vector_rowid) in stale {
                    transaction.execute(
                        &format!("DELETE FROM {table_name} WHERE rowid = ?1"),
                        [vector_rowid],
                    )?;
                    transaction.execute(
                        "DELETE FROM generation_vectors
                         WHERE generation_id = ?1 AND content_uuid = ?2",
                        rusqlite::params![generation_id, content_uuid],
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
                activate_generation_if_complete(&transaction, generation_id, source_documents)?;
                transaction.commit()?;
                Ok(())
            })
            .await?;
        Ok(source_documents)
    }

    pub async fn source_document_count(&self, notes: &Connection) -> Result<u64> {
        Ok(index_documents(notes).await?.len() as u64)
    }

    pub async fn take_jobs(&self, limit: u32) -> Result<Vec<IndexJob>> {
        let generation_id = self.generation_id;
        self.connection
            .call(move |database| {
                let mut statement = database.prepare(
                    "SELECT content_uuid, input_hash, input_text FROM embedding_jobs
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
        items: Vec<(uuid::Uuid, String, Vec<f32>)>,
    ) -> Result<()> {
        let generation_id = self.generation_id;
        let dimensions = self.dimensions;
        let table_name = self.table_name.clone();
        self.connection
            .call(move |database| {
                let transaction = database.transaction()?;
                for (content_uuid, input_hash, embedding) in items {
                    if embedding.len() != dimensions {
                        return Err(rusqlite::Error::InvalidParameterName(format!(
                            "embedding for {content_uuid} has dimension {}, expected {dimensions}",
                            embedding.len()
                        )));
                    }
                    let still_current = transaction
                        .query_row(
                            "SELECT source_seq FROM embedding_jobs
                             WHERE generation_id = ?1 AND content_uuid = ?2 AND input_hash = ?3",
                            rusqlite::params![generation_id, content_uuid, input_hash],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()?;
                    let Some(source_seq) = still_current else {
                        continue;
                    };
                    let rowid = match transaction
                        .query_row(
                            "SELECT vector_rowid FROM generation_vectors
                             WHERE generation_id = ?1 AND content_uuid = ?2",
                            rusqlite::params![generation_id, content_uuid],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()?
                    {
                        Some(rowid) => rowid,
                        None => next_vector_rowid(&transaction, generation_id)?,
                    };
                    let blob = embedding
                        .iter()
                        .flat_map(|value| value.to_le_bytes())
                        .collect::<Vec<_>>();
                    transaction.execute(
                        &format!("DELETE FROM {table_name} WHERE rowid = ?1"),
                        [rowid],
                    )?;
                    transaction.execute(
                        &format!("INSERT INTO {table_name}(rowid, embedding) VALUES (?1, ?2)"),
                        rusqlite::params![rowid, blob],
                    )?;
                    transaction.execute(
                        "INSERT INTO generation_vectors
                           (generation_id, content_uuid, vector_rowid, input_hash, source_seq)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(generation_id, content_uuid) DO UPDATE SET
                           vector_rowid = excluded.vector_rowid,
                           input_hash = excluded.input_hash,
                           source_seq = excluded.source_seq",
                        rusqlite::params![
                            generation_id,
                            content_uuid,
                            rowid,
                            input_hash,
                            source_seq
                        ],
                    )?;
                    transaction.execute(
                        "DELETE FROM embedding_jobs
                         WHERE generation_id = ?1 AND content_uuid = ?2 AND input_hash = ?3",
                        rusqlite::params![generation_id, content_uuid, input_hash],
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
                    "SELECT COUNT(*) FROM generation_vectors WHERE generation_id = ?1",
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
        "SELECT COUNT(*) FROM generation_vectors WHERE generation_id = ?1",
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
                    "SELECT mapping.content_uuid, vectors.distance
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
    input_hash: String,
}

type DocumentChainRow = (uuid::Uuid, i32, Option<String>, String);

async fn index_documents(notes: &Connection) -> Result<BTreeMap<uuid::Uuid, IndexDocument>> {
    let rows = notes
        .call(|database| {
            let mut statement = database.prepare(
                "WITH RECURSIVE chain(qid, uuid, parent_uuid, title, content, depth) AS (
                   SELECT block.uuid, block.uuid, block.parent_uuid, page.title,
                          block.markdown, 0
                     FROM blocks block JOIN pages page ON page.uuid = block.page_uuid
                   UNION ALL
                   SELECT chain.qid, chain.uuid, parent.parent_uuid, NULL,
                          parent.markdown, chain.depth + 1
                     FROM chain JOIN blocks parent ON parent.uuid = chain.parent_uuid
                 )
                 SELECT uuid, depth, title, content FROM chain ORDER BY uuid, depth",
            )?;
            statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<Result<Vec<DocumentChainRow>, _>>()
        })
        .await?;
    let mut chains = BTreeMap::<uuid::Uuid, Vec<(i32, Option<String>, String)>>::new();
    for (uuid, depth, title, content) in rows {
        chains
            .entry(uuid)
            .or_default()
            .push((depth, title, content));
    }
    Ok(chains
        .into_iter()
        .filter_map(|(uuid, chain)| {
            let text = compose_text(&chain);
            if text.trim().is_empty() {
                return None;
            }
            let mut digest = Sha256::new();
            digest.update(INPUT_FORMAT_VERSION.to_le_bytes());
            digest.update(text.as_bytes());
            Some((
                uuid,
                IndexDocument {
                    input_hash: format!("{:x}", digest.finalize()),
                    text,
                },
            ))
        })
        .collect())
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
        input_hash: format!("{:x}", digest.finalize()),
    }
}

fn compose_text(chain: &[(i32, Option<String>, String)]) -> String {
    let mut parts = Vec::new();
    let mut titles = chain
        .iter()
        .filter_map(|(depth, title, _)| {
            (*depth > 0)
                .then_some((*depth, title.as_deref()?.trim()))
                .filter(|(_, title)| !title.is_empty())
        })
        .collect::<Vec<_>>();
    titles.sort_by_key(|(depth, _)| std::cmp::Reverse(*depth));
    if !titles.is_empty() {
        parts.push(
            titles
                .into_iter()
                .map(|(_, title)| title)
                .collect::<Vec<_>>()
                .join(" > "),
        );
    }
    if let Some((_, _, parent_content)) = chain.iter().find(|(depth, _, _)| *depth == 1) {
        let excerpt = parent_content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(200)
            .collect::<String>();
        if !excerpt.is_empty() {
            parts.push(excerpt);
        }
    }
    if let Some((_, title, content)) = chain.iter().find(|(depth, _, _)| *depth == 0) {
        if let Some(title) = title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            parts.push(title.to_owned());
        }
        if !content.trim().is_empty() {
            parts.push(content.clone());
        }
    }
    parts.join("\n")
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
    use notes_core::BlockStyle;

    #[test]
    fn ai_schema_migration_is_valid() {
        migrations().validate().expect("valid AI migrations");
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
        let source_documents = store.reconcile(&notes, 1).await.expect("reconcile");
        let jobs = store.take_jobs(16).await.expect("jobs");
        assert_eq!(jobs.len(), 1);
        store
            .write_embeddings(
                source_documents,
                vec![(
                    jobs[0].content_uuid,
                    jobs[0].input_hash.clone(),
                    vec![0.25, 0.75],
                )],
            )
            .await
            .expect("write embedding");
        let status = store.status(source_documents).await.expect("status");
        assert_eq!(status.generation_status, GenerationStatus::Active);
        assert_eq!(status.indexed, 1);
        let matches = store.search(vec![0.25, 0.75], 4).await.expect("search");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].content_uuid, block.uuid);

        store.reset_index().await.expect("reset index");
        let status = store.status(source_documents).await.expect("reset status");
        assert_eq!(status.generation_status, GenerationStatus::Building);
        assert_eq!(status.indexed, 0);
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

        let source_documents = store.reconcile(&notes, 0).await.expect("reconcile");
        let status = store.status(source_documents).await.expect("status");

        assert_eq!(source_documents, 0);
        assert_eq!(status.generation_status, GenerationStatus::Active);
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
        let source_documents = store.reconcile(&notes, 1).await.expect("first reconcile");
        let stale_job = store.take_jobs(1).await.expect("old job").remove(0);

        notes_core::db::set_block_content(
            &notes,
            block.uuid,
            notes_core::db::BlockContent {
                markdown: "new content".into(),
            },
        )
        .await
        .expect("change note");
        store.reconcile(&notes, 2).await.expect("second reconcile");
        let current_job = store.take_jobs(1).await.expect("new job").remove(0);
        assert_ne!(stale_job.input_hash, current_job.input_hash);

        store
            .write_embeddings(
                source_documents,
                vec![(stale_job.content_uuid, stale_job.input_hash, vec![1.0, 0.0])],
            )
            .await
            .expect("ignore stale result");
        let status = store.status(source_documents).await.expect("status");
        assert_eq!(status.indexed, 0);
        assert_eq!(status.pending, 1);
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
