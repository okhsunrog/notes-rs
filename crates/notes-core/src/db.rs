use crate::model::{NodeKind, ReorderDirection};
use crate::operation::{
    self, AttachmentAdd, AttachmentRemove, EdgeAdd, EdgeRemove, NodeCreate, NodeDelete, NodeMove,
    NodeSetContent, NodeSetTitle, OpKind, Origin,
};
use crate::sqlite::Connection;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;

mod archive;
mod attachments;
mod blocks;
mod graph;
mod history;
mod migrations;
mod nodes;
mod search;

pub use archive::{export_archive, import_archive};
pub use attachments::{
    attachment_path_ref_count, create_attachment, delete_attachment, list_attachments,
};
pub use blocks::{
    create_block, delete_block, get_node_by_uuid, get_nodes_by_uuids, get_or_create_page_by_title,
    get_page_by_title, indent_block, move_block, move_block_in_direction, outdent_block,
    reorder_block,
};
pub use graph::{
    find_backlinks, find_tagged, graph_snapshot, link_nodes, neighbors, read_ancestors,
    read_subtree,
};
pub(crate) use graph::{page_uuid, replace_block_refs_tx_at, stable_node_id};
pub use history::{history_status, redo_history, undo_history};
pub use nodes::{
    BlockContent, CreatedNote, DeletedPage, create_node, create_note, create_page, delete_page,
    get_containing_page, get_node, list_block_children, list_entities, list_pages,
    node_uuids_for_ids, rename_page, set_block_content, split_block, update_block_with_refs,
    update_node,
};
pub use search::{search_blocks_fts, search_fts, search_pages_by_title};

pub async fn open(path: impl AsRef<Path>) -> Result<Connection> {
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
    migrations::migrate(&conn).await?;
    Ok(conn)
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

async fn apply_local_action(conn: &Connection, action: &str, kinds: Vec<OpKind>) -> Result<()> {
    if kinds.is_empty() {
        return Ok(());
    }
    let inverse = history::capture_inverse_kinds(conn, &kinds).await?;
    apply_local(conn, kinds.clone()).await?;
    history::record_action(conn, action, kinds, inverse).await
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
    pub files: std::collections::BTreeMap<String, String>,
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
        let connection = open(database.path())
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
                block_uuids: Vec::new(),
            },
            BlockContent {
                content: "second".into(),
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
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
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
        assert_eq!(removed.attachments.len(), 1);
        assert_eq!(removed.attachments[0].id, attachment.id);
        assert!(
            get_node(&connection, attachment.id)
                .await
                .expect("query attachment")
                .is_none()
        );
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
        let second = create_page(&connection, "Second".into())
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

    #[tokio::test]
    async fn deleting_a_page_restores_its_subtree_with_operation_history() {
        let (_database, connection) = temporary_database().await;
        let note = create_note(&connection).await.expect("create note");
        let child = create_block(
            &connection,
            Some(note.initial_block.id),
            None,
            "nested".into(),
            None,
        )
        .await
        .expect("create nested block");
        let history_before_delete = history_status(&connection).await.expect("history status").0;

        assert!(
            delete_page(&connection, note.page.id)
                .await
                .expect("delete page")
                .is_some()
        );
        assert!(
            get_node_by_uuid(&connection, child.uuid)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            history_status(&connection).await.expect("history status").0,
            history_before_delete + 1
        );

        assert!(undo_history(&connection).await.expect("undo page delete"));
        let restored_page = get_node_by_uuid(&connection, note.page.uuid)
            .await
            .expect("read page")
            .expect("restored page");
        let restored_parent = get_node_by_uuid(&connection, note.initial_block.uuid)
            .await
            .expect("read parent")
            .expect("restored parent");
        let restored_child = get_node_by_uuid(&connection, child.uuid)
            .await
            .expect("read child")
            .expect("restored child");
        assert_eq!(restored_parent.parent_id, Some(restored_page.id));
        assert_eq!(restored_child.parent_id, Some(restored_parent.id));

        let history_columns = connection
            .call(|database| -> rusqlite::Result<Vec<String>> {
                let mut statement = database.prepare("PRAGMA table_info(history_redo)")?;
                statement
                    .query_map([], |row| row.get(1))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
            .expect("history columns");
        assert!(
            history_columns
                .iter()
                .any(|column| column == "inverse_json")
        );
        assert!(
            !history_columns
                .iter()
                .any(|column| column == "archive_json")
        );
    }
}
