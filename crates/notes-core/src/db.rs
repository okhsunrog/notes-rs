use crate::model::{BackgroundQueue, FailureKind, NodeKind, ReorderDirection};
use crate::operation::{
    self, AttachmentAdd, AttachmentRemove, EdgeAdd, NodeCreate, NodeDelete, NodeMove,
    NodeSetContent, NodeSetTitle, OpKind, Origin,
};
use crate::sqlite::Connection;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use rusqlite::ffi::sqlite3_auto_extension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Once;

mod archive;
mod attachments;
mod blocks;
mod graph;
mod migrations;
mod nodes;
mod queues;
mod search;

pub use archive::*;
pub use attachments::*;
pub use blocks::*;
pub use graph::*;
pub use nodes::*;
pub use queues::*;
pub use search::*;

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

pub async fn open(path: impl AsRef<Path>, embedder_id: &str, ndims: usize) -> Result<Connection> {
    register_sqlite_vec();
    let conn = Connection::open(path.as_ref())
        .await
        .context("opening sqlite database")?;
    conn.call(|c| -> rusqlite::Result<()> {
        c.pragma_update(None, "journal_mode", "WAL")?;
        c.pragma_update(None, "synchronous", "NORMAL")?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        c.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(())
    })
    .await?;
    migrations::migrate(&conn, ndims).await?;
    cleanup_orphan_entities(&conn).await?;
    check_embedder_compat(&conn, embedder_id, ndims).await?;
    Ok(conn)
}

async fn check_embedder_compat(conn: &Connection, embedder_id: &str, ndims: usize) -> Result<()> {
    let id = embedder_id.to_string();
    let stored = conn
        .call(move |c| -> rusqlite::Result<Option<(String, i64)>> {
            let mut stmt = c.prepare("SELECT provider, ndims FROM embed_meta WHERE id = 1")?;
            let mut rows = stmt.query([])?;
            if let Some(r) = rows.next()? {
                Ok(Some((r.get(0)?, r.get(1)?)))
            } else {
                Ok(None)
            }
        })
        .await?;
    match stored {
        None => {
            let id2 = id.clone();
            conn.call(move |c| -> rusqlite::Result<()> {
                c.execute(
                    "INSERT INTO embed_meta(id, provider, ndims) VALUES (1, ?1, ?2)",
                    rusqlite::params![id2, ndims as i64],
                )?;
                Ok(())
            })
            .await?;
            Ok(())
        }
        Some((p, n)) if p == id && n as usize == ndims => Ok(()),
        Some((p, n)) => {
            tracing::warn!(
                old_provider = %p,
                old_ndims = n,
                new_provider = %id,
                new_ndims = ndims,
                "embedder changed — rebuilding vec_nodes and re-enqueueing all embeddings",
            );
            let id2 = id.clone();
            let ndims_u = ndims;
            conn.call(move |c| -> rusqlite::Result<()> {
                let tx = c.transaction()?;
                tx.execute_batch("DROP TABLE IF EXISTS vec_nodes;")?;
                tx.execute_batch(&format!(
                    "CREATE VIRTUAL TABLE vec_nodes USING vec0(embedding float[{ndims_u}]);"
                ))?;
                tx.execute("DELETE FROM embed_queue", [])?;
                tx.execute(
                    "INSERT INTO embed_queue(node_id, enqueued_at)
                     SELECT id, unixepoch() FROM nodes
                     WHERE kind IN ('block', 'page')
                       AND (content != '' OR title IS NOT NULL)",
                    [],
                )?;
                tx.execute(
                    "UPDATE embed_meta SET provider = ?1, ndims = ?2 WHERE id = 1",
                    rusqlite::params![id2, ndims_u as i64],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct Node {
    pub id: i64,
    pub uuid: uuid::Uuid,
    pub kind: NodeKind,
    pub title: Option<String>,
    pub content: String,
    pub content_json: Option<String>,
    pub parent_id: Option<i64>,
    pub position: Option<f64>,
    pub created_at: i64,
    pub updated_at: i64,
}

const NODE_COLUMNS: &str =
    "id, uuid, kind, title, content, content_json, parent_id, position, created_at, updated_at";
const NODE_COLUMNS_N: &str = "n.id, n.uuid, n.kind, n.title, n.content, n.content_json, n.parent_id, n.position, n.created_at, n.updated_at";

fn row_to_node(r: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    Ok(Node {
        id: r.get(0)?,
        uuid: r.get(1)?,
        kind: r.get(2)?,
        title: r.get(3)?,
        content: r.get(4)?,
        content_json: r.get(5)?,
        parent_id: r.get(6)?,
        position: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

async fn apply_local(conn: &Connection, kinds: Vec<OpKind>) -> Result<()> {
    let operations = operation::local_ops(conn, kinds).await?;
    operation::apply_batch(conn, &operations, Origin::Local).await?;
    Ok(())
}

async fn require_node_uuid(conn: &Connection, id: i64) -> Result<uuid::Uuid> {
    conn.call(move |database| {
        database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [id], |row| {
            row.get(0)
        })
    })
    .await
    .with_context(|| format!("node {id} not found"))
}

async fn require_node_uuids(
    conn: &Connection,
    first: i64,
    second: i64,
) -> Result<(uuid::Uuid, uuid::Uuid)> {
    conn.call(move |database| {
        let first_uuid =
            database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [first], |row| {
                row.get(0)
            })?;
        let second_uuid =
            database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [second], |row| {
                row.get(0)
            })?;
        Ok((first_uuid, second_uuid))
    })
    .await
}

async fn count_missing_block_refs(conn: &Connection, block_uuids: Vec<String>) -> Result<u32> {
    conn.call(move |database| {
        let mut broken = 0;
        for uuid in block_uuids {
            if uuid.trim().is_empty() {
                continue;
            }
            let Ok(uuid) = uuid::Uuid::parse_str(uuid.trim()) else {
                broken += 1;
                continue;
            };
            let exists = database
                .query_row("SELECT 1 FROM nodes WHERE uuid = ?1", [uuid], |_| Ok(()))
                .optional()?
                .is_some();
            if !exists {
                broken += 1;
            }
        }
        Ok(broken)
    })
    .await
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SearchHit {
    pub node: Node,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct Edge {
    pub src: i64,
    pub dst: i64,
    pub kind: String,
    pub weight: f64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataArchive {
    pub format: String,
    pub version: u32,
    pub exported_at: i64,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub entity_descriptions: Vec<EntityDescriptionRecord>,
    #[serde(default)]
    pub files: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityDescriptionRecord {
    pub source_node_id: i64,
    pub entity_node_id: i64,
    pub description: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct GraphSnapshot {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

type AttachmentDeleteRecord = (Node, Option<uuid::Uuid>, Option<String>);
type ReorderPlan = (uuid::Uuid, Option<uuid::Uuid>, Vec<(uuid::Uuid, f64)>);

#[cfg(test)]
mod tests {
    use super::*;

    async fn temporary_database() -> (tempfile::NamedTempFile, Connection) {
        let database = tempfile::NamedTempFile::new().expect("create temporary database");
        let connection = open(database.path(), "test:4d", 4)
            .await
            .expect("open temporary database");
        (database, connection)
    }

    #[tokio::test]
    async fn refuses_to_move_a_block_into_its_descendant() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Page".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let parent = create_block(&connection, Some(page.id), None, "parent".into(), None)
            .await
            .expect("create parent");
        let child = create_block(&connection, Some(parent.id), None, "child".into(), None)
            .await
            .expect("create child");

        let error = move_block(&connection, parent.id, Some(child.id), None)
            .await
            .expect_err("cycle must be rejected");
        assert!(error.to_string().contains("descendants"));
    }

    #[tokio::test]
    async fn natural_language_fts_search_treats_punctuation_as_text() {
        let (_database, connection) = temporary_database().await;
        let block = create_node(
            &connection,
            NodeKind::Block,
            None,
            "Offline-first Rust notebook".into(),
            None,
        )
        .await
        .expect("create searchable block");

        let hits = search_fts(&connection, "offline-first? Rust".into(), 10)
            .await
            .expect("punctuation must not become FTS syntax");
        assert!(hits.iter().any(|hit| hit.node.id == block.id));

        assert!(
            search_fts(&connection, "???".into(), 10)
                .await
                .expect("punctuation-only search")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn replacing_extraction_removes_stale_mentions_and_relations() {
        let (_database, connection) = temporary_database().await;
        let source = create_node(
            &connection,
            NodeKind::Page,
            Some("Source".into()),
            String::new(),
            None,
        )
        .await
        .expect("create source");
        replace_extracted_edges(
            &connection,
            source.id,
            source.title.clone(),
            source.content.clone(),
            vec![
                ("Rust".into(), Some("language".into())),
                ("Tauri".into(), Some("framework".into())),
            ],
            vec![("Tauri".into(), "Rust".into(), "uses".into())],
        )
        .await
        .expect("apply extraction");

        replace_extracted_edges(
            &connection,
            source.id,
            source.title.clone(),
            source.content.clone(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("clear extraction");
        let generated_edges: i64 = connection
            .call(|database| {
                database.query_row(
                    "SELECT COUNT(*) FROM edges
                     WHERE kind = 'mentions' OR kind = 'uses'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("count generated edges");
        assert_eq!(generated_edges, 0);
    }

    #[tokio::test]
    async fn deleting_the_last_extraction_source_removes_orphan_entities() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Source".into()),
            "Entity source".into(),
            None,
        )
        .await
        .expect("create source page");
        replace_extracted_edges(
            &connection,
            page.id,
            page.title.clone(),
            page.content.clone(),
            vec![
                ("Alpha".into(), Some("First".into())),
                ("Beta".into(), None),
            ],
            vec![("Alpha".into(), "Beta".into(), "related".into())],
        )
        .await
        .expect("extract entities");
        assert_eq!(
            list_entities(&connection, 10)
                .await
                .expect("list entities")
                .len(),
            2
        );

        delete_page(&connection, page.id)
            .await
            .expect("delete page");

        assert!(
            list_entities(&connection, 10)
                .await
                .expect("list entities after delete")
                .is_empty()
        );
        assert!(
            graph_snapshot(&connection, None)
                .await
                .expect("graph after delete")
                .nodes
                .is_empty()
        );
    }

    #[tokio::test]
    async fn entity_descriptions_keep_source_provenance() {
        let (_database, connection) = temporary_database().await;
        let first = create_node(
            &connection,
            NodeKind::Page,
            Some("First".into()),
            "Rust note".into(),
            None,
        )
        .await
        .expect("create first source");
        let second = create_node(
            &connection,
            NodeKind::Page,
            Some("Second".into()),
            "Another Rust note".into(),
            None,
        )
        .await
        .expect("create second source");

        for (source, description) in [
            (&first, "A systems language"),
            (&second, "A memory-safe language"),
        ] {
            replace_extracted_edges(
                &connection,
                source.id,
                source.title.clone(),
                source.content.clone(),
                vec![("Rust".into(), Some(description.into()))],
                Vec::new(),
            )
            .await
            .expect("apply extraction");
        }

        let content = connection
            .call(|database| {
                database.query_row(
                    "SELECT content FROM nodes WHERE kind = 'entity' AND title = 'Rust'",
                    [],
                    |row| row.get::<_, String>(0),
                )
            })
            .await
            .expect("read merged entity description");
        assert!(content.contains("A systems language"));
        assert!(content.contains("A memory-safe language"));

        replace_extracted_edges(
            &connection,
            second.id,
            second.title.clone(),
            second.content.clone(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("clear second extraction");
        let content = connection
            .call(|database| {
                database.query_row(
                    "SELECT content FROM nodes WHERE kind = 'entity' AND title = 'Rust'",
                    [],
                    |row| row.get::<_, String>(0),
                )
            })
            .await
            .expect("read remaining entity description");
        assert_eq!(content, "A systems language");
    }

    #[tokio::test]
    async fn stale_extraction_cannot_attach_entities_to_a_changed_source() {
        let (_database, connection) = temporary_database().await;
        let source = create_node(
            &connection,
            NodeKind::Page,
            Some("Before".into()),
            "old content".into(),
            None,
        )
        .await
        .expect("create source");
        update_node(
            &connection,
            source.id,
            Some("After".into()),
            "new content".into(),
            None,
        )
        .await
        .expect("change source");

        let error = replace_extracted_edges(
            &connection,
            source.id,
            source.title,
            source.content,
            vec![("Stale entity".into(), None)],
            Vec::new(),
        )
        .await
        .expect_err("stale result must be rejected");
        assert!(error.to_string().contains("source changed"));
        assert!(
            get_page_by_title(&connection, "Stale entity".into())
                .await
                .expect("query stale entity")
                .is_none()
        );
        let entity_count: i64 = connection
            .call(|database| {
                database.query_row(
                    "SELECT COUNT(*) FROM nodes WHERE kind = 'entity'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("count entities");
        assert_eq!(entity_count, 0);
    }

    #[tokio::test]
    async fn split_block_is_atomic_and_normalizes_positions() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Page".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let block = create_block(&connection, Some(page.id), None, "old".into(), None)
            .await
            .expect("create block");
        let parts = vec![
            BlockContent {
                content: "first [[Linked]]".into(),
                wikilink_titles: vec!["Linked".into()],
                block_uuids: Vec::new(),
            },
            BlockContent {
                content: "second".into(),
                wikilink_titles: Vec::new(),
                block_uuids: Vec::new(),
            },
        ];
        let changed = split_block(&connection, block.id, parts)
            .await
            .expect("split block");
        assert_eq!(changed.len(), 2);
        assert_eq!(changed[0].content, "first [[Linked]]");
        assert_eq!(changed[1].content, "second");
        assert_eq!(changed[0].position, Some(1024.0));
        assert_eq!(changed[1].position, Some(2048.0));

        let refs: i64 = connection
            .call(move |database| {
                database.query_row(
                    "SELECT COUNT(*) FROM edges WHERE src = ?1 AND kind = 'refs'",
                    [block.id],
                    |row| row.get(0),
                )
            })
            .await
            .expect("count refs");
        assert_eq!(refs, 1);
    }

    #[tokio::test]
    async fn reorder_block_swaps_siblings_without_fractional_positions() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Page".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let first = create_block(&connection, Some(page.id), None, "first".into(), None)
            .await
            .expect("create first");
        let second = create_block(&connection, Some(page.id), None, "second".into(), None)
            .await
            .expect("create second");

        reorder_block(&connection, second.id, ReorderDirection::Up)
            .await
            .expect("reorder");
        let siblings = list_block_children(&connection, page.id)
            .await
            .expect("list siblings");
        assert_eq!(
            siblings.iter().map(|node| node.id).collect::<Vec<_>>(),
            vec![second.id, first.id]
        );
        assert_eq!(siblings[0].position, Some(1024.0));
        assert_eq!(siblings[1].position, Some(2048.0));
    }

    #[tokio::test]
    async fn uuid_block_intents_indent_outdent_and_reorder() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Page".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let first = create_block(&connection, Some(page.id), None, "first".into(), None)
            .await
            .expect("create first");
        let second = create_block(&connection, Some(page.id), None, "second".into(), None)
            .await
            .expect("create second");
        let third = create_block(&connection, Some(page.id), None, "third".into(), None)
            .await
            .expect("create third");

        let indented = indent_block(&connection, second.uuid)
            .await
            .expect("indent by UUID");
        assert_eq!(indented.parent_id, Some(first.id));

        let outdented = outdent_block(&connection, second.uuid)
            .await
            .expect("outdent by UUID");
        assert_eq!(outdented.parent_id, Some(page.id));

        move_block_in_direction(&connection, third.uuid, ReorderDirection::Up)
            .await
            .expect("reorder by UUID");
        let siblings = list_block_children(&connection, page.id)
            .await
            .expect("list siblings");
        assert_eq!(siblings[0].uuid, first.uuid);
        assert_eq!(siblings[1].uuid, third.uuid);
        assert_eq!(siblings[2].uuid, second.uuid);
    }

    #[tokio::test]
    async fn archive_round_trip_restores_nodes_edges_and_hierarchy() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Original".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let block = create_block(
            &connection,
            Some(page.id),
            None,
            "A [[Linked]] block".into(),
            None,
        )
        .await
        .expect("create block");
        replace_block_refs(&connection, block.id, vec!["Linked".into()], Vec::new())
            .await
            .expect("create reference");
        let archive = export_archive(&connection).await.expect("export archive");

        delete_page(&connection, page.id)
            .await
            .expect("delete page");
        import_archive(&connection, archive)
            .await
            .expect("restore archive");

        let restored = get_node_by_uuid(&connection, block.uuid)
            .await
            .expect("query block")
            .expect("restored block");
        let restored_page = get_node_by_uuid(&connection, page.uuid)
            .await
            .expect("query restored page")
            .expect("restored page");
        assert_eq!(restored.parent_id, Some(restored_page.id));
        let linked = get_page_by_title(&connection, "Linked".into())
            .await
            .expect("query linked page")
            .expect("linked page restored");
        let backlinks = find_backlinks(&connection, linked.id, Some("refs".into()))
            .await
            .expect("query backlinks");
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].uuid, block.uuid);
    }

    #[tokio::test]
    async fn explicit_page_creation_reuses_wikilink_stub_identity() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Source".into()),
            String::new(),
            None,
        )
        .await
        .expect("create source page");
        let block = create_block(
            &connection,
            Some(page.id),
            None,
            "See [[Roadmap]]".into(),
            None,
        )
        .await
        .expect("create linked block");
        let stub = get_page_by_title(&connection, "Roadmap".into())
            .await
            .expect("query stub")
            .expect("stub page");
        let explicit = create_node(
            &connection,
            NodeKind::Page,
            Some("Roadmap".into()),
            String::new(),
            None,
        )
        .await
        .expect("materialize explicit page");
        assert_eq!(explicit.uuid, stub.uuid);
        assert_eq!(
            find_backlinks(&connection, explicit.id, Some("refs".into()))
                .await
                .expect("backlinks")[0]
                .uuid,
            block.uuid
        );
    }

    #[tokio::test]
    async fn renamed_page_does_not_reserve_its_old_title_identity() {
        let (_database, connection) = temporary_database().await;
        let original = create_node(
            &connection,
            NodeKind::Page,
            Some("Old title".into()),
            String::new(),
            None,
        )
        .await
        .expect("create original page");
        update_node(
            &connection,
            original.id,
            Some("New title".into()),
            String::new(),
            None,
        )
        .await
        .expect("rename page");
        let replacement = create_node(
            &connection,
            NodeKind::Page,
            Some("Old title".into()),
            String::new(),
            None,
        )
        .await
        .expect("reuse old title");
        assert_ne!(replacement.uuid, original.uuid);
    }

    #[tokio::test]
    async fn renaming_a_page_does_not_advance_its_content_clock() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Before".into()),
            "keep this content".into(),
            None,
        )
        .await
        .expect("create page");
        let before: (Option<String>, Option<String>) = connection
            .call({
                let uuid = page.uuid;
                move |database| {
                    database.query_row(
                        "SELECT title_hlc, content_hlc FROM nodes WHERE uuid = ?1",
                        [uuid],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                }
            })
            .await
            .expect("read clocks before rename");

        let renamed = rename_page(&connection, page.uuid, Some("After".into()))
            .await
            .expect("rename page");
        let after: (Option<String>, Option<String>) = connection
            .call({
                let uuid = page.uuid;
                move |database| {
                    database.query_row(
                        "SELECT title_hlc, content_hlc FROM nodes WHERE uuid = ?1",
                        [uuid],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                }
            })
            .await
            .expect("read clocks after rename");

        assert_eq!(renamed.title.as_deref(), Some("After"));
        assert_eq!(renamed.content, "keep this content");
        assert_ne!(after.0, before.0);
        assert_eq!(after.1, before.1);
    }

    #[tokio::test]
    async fn create_note_materializes_page_and_initial_block_together() {
        let (_database, connection) = temporary_database().await;
        let note = create_note(&connection).await.expect("create note");

        assert_eq!(note.page.kind, NodeKind::Page);
        assert_eq!(note.page.title, None);
        assert_eq!(note.initial_block.kind, NodeKind::Block);
        assert_eq!(note.initial_block.parent_id, Some(note.page.id));
        let children = list_block_children(&connection, note.page.id)
            .await
            .expect("list initial blocks");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].uuid, note.initial_block.uuid);
    }

    #[tokio::test]
    async fn attachments_follow_their_page_into_deletion() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Page".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let attachment = create_attachment(
            &connection,
            page.id,
            "attachment-hash".into(),
            "photo.png".into(),
            "image/png".into(),
            4,
        )
        .await
        .expect("create attachment");
        assert_eq!(
            list_attachments(&connection, page.id)
                .await
                .expect("list attachments")
                .len(),
            1
        );

        let removed = delete_page(&connection, page.id)
            .await
            .expect("delete page")
            .expect("page existed");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, attachment.id);
        assert!(
            get_node(&connection, attachment.id)
                .await
                .expect("query attachment")
                .is_none()
        );
    }

    #[tokio::test]
    async fn background_queue_controls_report_retry_and_clear_work() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            NodeKind::Page,
            Some("Queued".into()),
            "content".into(),
            None,
        )
        .await
        .expect("create queued page");
        record_embedding_failure(
            &connection,
            vec![page.id],
            FailureKind::Network,
            "connection timed out",
            false,
        )
        .await
        .expect("record embedding failure");
        record_extraction_failure(
            &connection,
            page.id,
            FailureKind::ProviderRequest,
            "model unavailable",
            true,
        )
        .await
        .expect("record extraction failure");
        let status = queue_status(&connection).await.expect("read queue status");
        assert_eq!(status.embeddings_pending, 1);
        assert_eq!(status.embeddings_failed, 1);
        assert_eq!(status.extractions_pending, 1);
        assert_eq!(status.extractions_failed, 1);
        assert_eq!(status.failures.len(), 2);
        assert!(
            status
                .failures
                .iter()
                .any(|failure| failure.terminal && failure.last_error == "model unavailable")
        );

        retry_background_jobs(&connection)
            .await
            .expect("retry queues");
        let status = queue_status(&connection)
            .await
            .expect("read retried queues");
        assert_eq!(status.embeddings_pending, 1);
        assert_eq!(status.embeddings_failed, 0);
        assert_eq!(status.extractions_pending, 1);
        assert_eq!(status.extractions_failed, 0);
        assert!(status.failures.is_empty());

        record_extraction_failure(
            &connection,
            page.id,
            FailureKind::Schema,
            "invalid JSON",
            false,
        )
        .await
        .expect("record first schema mismatch");
        record_extraction_failure(
            &connection,
            page.id,
            FailureKind::Schema,
            "invalid JSON again",
            false,
        )
        .await
        .expect("record second schema mismatch");
        let status = queue_status(&connection)
            .await
            .expect("read terminal schema failure");
        assert!(status.failures[0].terminal);
        assert_eq!(status.failures[0].failure_kind, FailureKind::Schema);

        clear_background_jobs(&connection)
            .await
            .expect("clear queues");
        let status = queue_status(&connection)
            .await
            .expect("read cleared queues");
        assert_eq!(status.embeddings_pending, 0);
        assert_eq!(status.embeddings_failed, 0);
        assert_eq!(status.extractions_pending, 0);
        assert_eq!(status.extractions_failed, 0);
        assert!(status.failures.is_empty());
    }

    #[tokio::test]
    async fn structural_history_undoes_and_redoes_database_changes() {
        let (_database, connection) = temporary_database().await;
        let first = create_node(
            &connection,
            NodeKind::Page,
            Some("First".into()),
            String::new(),
            None,
        )
        .await
        .expect("create first page");
        checkpoint_history(&connection, "create second")
            .await
            .expect("checkpoint");
        let second = create_node(
            &connection,
            NodeKind::Page,
            Some("Second".into()),
            String::new(),
            None,
        )
        .await
        .expect("create second page");
        let before_undo: i64 = connection
            .call(|database| {
                database.query_row("SELECT COUNT(*) FROM applied_ops", [], |row| row.get(0))
            })
            .await
            .expect("count operations before undo");

        assert!(undo_history(&connection).await.expect("undo"));
        let after_undo: i64 = connection
            .call(|database| {
                database.query_row("SELECT COUNT(*) FROM applied_ops", [], |row| row.get(0))
            })
            .await
            .expect("count operations after undo");
        assert!(
            after_undo > before_undo,
            "undo must apply inverse operations"
        );
        assert!(
            get_node(&connection, first.id)
                .await
                .expect("first query")
                .is_some()
        );
        assert!(
            get_node(&connection, second.id)
                .await
                .expect("second query")
                .is_none()
        );
        assert_eq!(
            history_status(&connection).await.expect("undo status"),
            (0, 1)
        );

        assert!(redo_history(&connection).await.expect("redo"));
        let after_redo: i64 = connection
            .call(|database| {
                database.query_row("SELECT COUNT(*) FROM applied_ops", [], |row| row.get(0))
            })
            .await
            .expect("count operations after redo");
        assert!(
            after_redo > after_undo,
            "redo must apply inverse operations"
        );
        assert!(
            get_node(&connection, second.id)
                .await
                .expect("second query")
                .is_some()
        );
        assert_eq!(
            history_status(&connection).await.expect("redo status"),
            (1, 0)
        );
    }
}
