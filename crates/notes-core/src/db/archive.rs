use super::*;
use crate::operation::{
    AttachmentAdd, AttachmentRemove, BlockCreate, BlockDelete, PageAliasSet, PageCreate,
    PageDelete, SnapshotPageAlias, SnapshotPageIdentity,
};
use crate::{
    ExternalImportDigest, ExternalImportFormat, ExternalImportIdentityContext,
    ExternalImportReceipt, PageAlias,
};
use std::collections::{HashMap, HashSet};

pub const ARCHIVE_FORMAT: &str = "tangleaf";
pub const ARCHIVE_VERSION: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveImportStats {
    pub applied_operations: usize,
    pub structure_reconciliations: u32,
    pub reference_projections: u32,
}

pub async fn export_archive(conn: &Connection) -> Result<DataArchive> {
    conn.call_domain(|database| -> Result<DataArchive> {
        let transaction = database.transaction()?;
        let database = &transaction;
        let workspace_uuid = database.query_row(
            "SELECT uuid FROM workspace WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let page_identities = database
            .prepare(
                "SELECT page_uuid, CASE WHEN content_type = 'ink' THEN 'handwriting' ELSE page_kind END, journal_date
                   FROM page_identities ORDER BY page_uuid",
            )?
            .query_map([], |row| {
                Ok(SnapshotPageIdentity {
                    uuid: row.get(0)?,
                    kind: operation::page_kind_from_sql(row.get(1)?, row.get(2)?, 1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let pages = {
            let sql = format!("SELECT {PAGE_COLUMNS} FROM pages ORDER BY uuid");
            database
                .prepare(&sql)?
                .query_map([], row_to_page)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let page_aliases = database
            .prepare(
                "SELECT page_uuid, alias, hlc, present
                   FROM page_alias_lww
                  WHERE present = 1
                  ORDER BY page_uuid, alias",
            )?
            .query_map([], |row| {
                Ok(SnapshotPageAlias {
                    page_uuid: row.get(0)?,
                    alias: row.get(1)?,
                    hlc: operation::sql_hlc(row.get(2)?, 2)?,
                    present: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let blocks = {
            let sql = format!("SELECT {BLOCK_COLUMNS} FROM blocks ORDER BY page_uuid, parent_uuid, order_key, uuid");
            database
                .prepare(&sql)?
                .query_map([], row_to_block)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let attachments = {
            let mut statement = database.prepare(
                "SELECT uuid, page_uuid, block_uuid, blob_hash, filename, mime, size, created_at
                   FROM attachments ORDER BY uuid",
            )?;
            statement
                .query_map([], |row| {
                    let owner = match (
                        row.get::<_, Option<uuid::Uuid>>(1)?,
                        row.get::<_, Option<uuid::Uuid>>(2)?,
                    ) {
                        (Some(uuid), None) => AttachmentOwner::Page(uuid),
                        (None, Some(uuid)) => AttachmentOwner::Block(uuid),
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    };
                    let size = u64::try_from(row.get::<_, i64>(6)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?;
                    Ok(Attachment {
                        uuid: row.get(0)?,
                        owner,
                        blob_hash: row_blob_hash(row, 3)?,
                        filename: row.get(4)?,
                        mime: row.get(5)?,
                        size,
                        created_at: row.get(7)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let external_import_receipts = database
            .prepare(
                "SELECT receipt_uuid, import_format, manifest_digest, plan_digest,
                        planner_version, identity_workspace_uuid, import_namespace_uuid,
                        provenance_json, imported_at
                   FROM external_import_receipts ORDER BY import_format",
            )?
            .query_map([], |row| {
                let format = row
                    .get::<_, String>(1)?
                    .parse::<ExternalImportFormat>()
                    .map_err(|error| {
                        invalid_archive_column(1, rusqlite::types::Type::Text, error)
                    })?;
                let manifest = row.get::<_, Vec<u8>>(2)?;
                let plan = row.get::<_, Vec<u8>>(3)?;
                let provenance_json = row.get::<_, String>(7)?;
                Ok(ExternalImportReceipt {
                    receipt_uuid: row.get(0)?,
                    format,
                    manifest_digest: ExternalImportDigest::from_slice(&manifest)
                        .map_err(|error| {
                            invalid_archive_column(2, rusqlite::types::Type::Blob, error)
                        })?,
                    plan_digest: ExternalImportDigest::from_slice(&plan)
                        .map_err(|error| {
                            invalid_archive_column(3, rusqlite::types::Type::Blob, error)
                        })?,
                    planner_version: row.get(4)?,
                    identity: ExternalImportIdentityContext {
                        workspace_uuid: row.get(5)?,
                        import_namespace_uuid: row.get(6)?,
                    },
                    provenance: serde_json::from_str(&provenance_json)
                        .map_err(|error| {
                            invalid_archive_column(7, rusqlite::types::Type::Text, error)
                        })?,
                    imported_at: row.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let (ink, ink_blobs) = crate::ink::archive::capture(database)?;
        let archive = DataArchive {
            format: ARCHIVE_FORMAT.into(),
            version: ARCHIVE_VERSION,
            workspace_uuid,
            exported_at: chrono::Utc::now().timestamp(),
            page_identities,
            page_aliases,
            pages,
            blocks,
            attachments,
            external_import_receipts,
            ink,
            ink_blobs,
        };
        transaction.commit()?;
        Ok(archive)
    })
    .await
}

pub async fn import_archive(conn: &Connection, archive: DataArchive) -> Result<()> {
    import_archive_with_precommit(conn, archive, || Ok(()))
        .await
        .map(|_| ())
}

/// Restores an archive while running a host-owned publication step inside the
/// validated SQLite transaction immediately before commit.
///
/// Attachment hosts use this boundary to publish already-verified blob bytes
/// only after every archive operation has applied successfully, while still
/// guaranteeing that bytes become durable before their metadata is committed.
pub async fn import_archive_with_precommit<F>(
    conn: &Connection,
    archive: DataArchive,
    precommit: F,
) -> Result<ArchiveImportStats>
where
    F: FnOnce() -> Result<()> + Send + 'static,
{
    if archive.format != ARCHIVE_FORMAT || !matches!(archive.version, 7 | ARCHIVE_VERSION) {
        return Err(crate::CoreError::invalid(format!(
            "unsupported archive format {} version {}",
            archive.format, archive.version
        ))
        .into());
    }
    if archive.workspace_uuid.is_nil() {
        return Err(crate::CoreError::invalid("archive workspace UUID cannot be nil").into());
    }
    let mut identities = HashMap::new();
    let mut journal_dates = HashSet::new();
    for identity in &archive.page_identities {
        operation::validate_page_identity(archive.workspace_uuid, identity.uuid, &identity.kind)?;
        if identities.insert(identity.uuid, &identity.kind).is_some() {
            return Err(crate::CoreError::invalid(format!(
                "archive contains duplicate page identity UUID {}",
                identity.uuid
            ))
            .into());
        }
        if let PageKind::Journal { date } = &identity.kind
            && !journal_dates.insert(date)
        {
            return Err(crate::CoreError::invalid(format!(
                "archive contains duplicate journal date {date}"
            ))
            .into());
        }
    }
    for page in &archive.pages {
        if identities
            .get(&page.uuid)
            .is_none_or(|kind| *kind != &page.kind)
        {
            return Err(crate::CoreError::invalid(format!(
                "archive page {} is missing its matching immutable identity",
                page.uuid
            ))
            .into());
        }
    }
    for alias in &archive.page_aliases {
        if !identities.contains_key(&alias.page_uuid) {
            return Err(crate::CoreError::invalid(format!(
                "archive alias {} references unknown page identity {}",
                alias.alias, alias.page_uuid
            ))
            .into());
        }
    }
    for block in &archive.blocks {
        if identities.contains_key(&block.uuid) {
            return Err(crate::CoreError::invalid(format!(
                "archive UUID {} is reserved by both a page identity and a block",
                block.uuid
            ))
            .into());
        }
    }
    let mut receipt_formats = HashSet::new();
    for receipt in &archive.external_import_receipts {
        if receipt.receipt_uuid.is_nil()
            || receipt.identity.import_namespace_uuid.is_nil()
            || receipt.identity.workspace_uuid != archive.workspace_uuid
            || receipt.planner_version == 0
            || !receipt.provenance.is_object()
            || !receipt_formats.insert(receipt.format)
        {
            return Err(crate::CoreError::invalid(
                "archive contains an invalid or duplicate external import receipt",
            )
            .into());
        }
    }
    let (archive, ink_manifest, ink_graphs) = tokio::task::spawn_blocking(move || {
        let (manifest, graphs) = crate::ink::archive::prepare(&archive)?;
        Ok::<_, crate::CoreError>((archive, manifest, graphs))
    })
    .await??;
    conn.call_domain(move |database| -> Result<ArchiveImportStats> {
        let transaction = database.transaction()?;
        let current_workspace_uuid = transaction_workspace_uuid(&transaction)?;
        if current_workspace_uuid != archive.workspace_uuid {
            let has_state: bool = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM page_identities
                    UNION ALL SELECT 1 FROM tombstones
                    UNION ALL SELECT 1 FROM applied_ops
                    UNION ALL SELECT 1 FROM sync_outbox
                    UNION ALL SELECT 1 FROM external_import_receipts
                 )",
                [],
                |row| row.get(0),
            )?;
            let sync_configured: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_meta WHERE key = 'server_url')",
                [],
                |row| row.get(0),
            )?;
            if has_state || sync_configured {
                return Err(crate::CoreError::conflict(
                    "cannot restore an archive from a different workspace into a non-empty or synchronized replica",
                )
                .into());
            }
        }
        let current_attachments = transaction
            .prepare("SELECT page_uuid, block_uuid, blob_hash FROM attachments ORDER BY uuid")?
            .query_map([], |row| {
                let owner = match (
                    row.get::<_, Option<uuid::Uuid>>(0)?,
                    row.get::<_, Option<uuid::Uuid>>(1)?,
                ) {
                    (Some(uuid), None) => AttachmentOwner::Page(uuid),
                    (None, Some(uuid)) => AttachmentOwner::Block(uuid),
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok((owner, row_blob_hash(row, 2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let current_blocks = transaction
            .prepare("SELECT uuid, page_uuid FROM blocks ORDER BY uuid")?
            .query_map([], |row| {
                Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, uuid::Uuid>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let current_pages = transaction
            .prepare("SELECT uuid FROM pages ORDER BY uuid")?
            .query_map([], |row| row.get::<_, uuid::Uuid>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let current_aliases = transaction
            .prepare("SELECT page_uuid, alias FROM page_alias_lww WHERE present = 1")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(uuid::Uuid, PageAlias)>>>()?;

        let mut restore_kinds = current_attachments
            .into_iter()
            .map(|(owner, blob_hash)| {
                OpKind::AttachmentRemove(AttachmentRemove { owner, blob_hash })
            })
            .collect::<Vec<_>>();
        restore_kinds.extend(
            current_aliases.into_iter().map(|(uuid, alias)| {
                OpKind::PageAliasSet(PageAliasSet {
                    uuid,
                    alias,
                    present: false,
                })
            }),
        );
        restore_kinds.extend(
            current_blocks
                .into_iter()
                .map(|(uuid, page_uuid)| OpKind::BlockDelete(BlockDelete { uuid, page_uuid })),
        );
        restore_kinds.extend(
            current_pages
                .into_iter()
                .map(|uuid| OpKind::PageDelete(PageDelete { uuid })),
        );
        if current_workspace_uuid != archive.workspace_uuid {
            debug_assert!(restore_kinds.is_empty());
            transaction.execute(
                "UPDATE workspace SET uuid = ?1 WHERE singleton = 1",
                [archive.workspace_uuid],
            )?;
        }
        for identity in &archive.page_identities {
            operation::ensure_page_identity(&transaction, identity.uuid, &identity.kind)?;
        }

        restore_kinds.extend(archive.pages.into_iter().map(|page| {
            OpKind::PageCreate(PageCreate {
                uuid: page.uuid,
                kind: page.kind,
                title: page.title,
                layout: page.layout,
                created_at: page.created_at,
            })
        }));
        restore_kinds.extend(archive.page_aliases.into_iter().map(|alias| {
            OpKind::PageAliasSet(PageAliasSet {
                uuid: alias.page_uuid,
                alias: alias.alias,
                present: alias.present,
            })
        }));
        restore_kinds.extend(archive.blocks.into_iter().map(|block| {
            OpKind::BlockCreate(BlockCreate {
                uuid: block.uuid,
                page_uuid: block.page_uuid,
                parent_uuid: block.parent_uuid,
                order_key: block.order_key,
                style: block.style,
                markdown: block.markdown,
                created_at: block.created_at,
            })
        }));
        restore_kinds.extend(archive.attachments.into_iter().map(|attachment| {
            OpKind::AttachmentAdd(AttachmentAdd {
                owner: attachment.owner,
                blob_hash: attachment.blob_hash,
                filename: attachment.filename,
                mime: attachment.mime,
                size: attachment.size,
            })
        }));
        // Read before the page deletes below purge them: the restored version
        // must descend from what this replica already published.
        let prior_ink_heads =
            crate::ink::archive::heads_before_restore(&transaction, &ink_manifest)?;
        let applied = operation::apply_local_kinds_deferred_in_transaction(
            &transaction,
            restore_kinds,
        )?;
        let ink_operations = crate::ink::archive::restore(
            &transaction,
            ink_manifest,
            ink_graphs,
            &prior_ink_heads,
        )?;
        let stats = ArchiveImportStats {
            applied_operations: applied.operations.len() + ink_operations,
            structure_reconciliations: applied.stats.structure_reconciliations,
            reference_projections: applied.stats.reference_projections,
        };
        drop(applied);
        transaction.execute("DELETE FROM external_import_receipts", [])?;
        for receipt in archive.external_import_receipts {
            transaction.execute(
                "INSERT INTO external_import_receipts(
                    receipt_uuid, import_format, manifest_digest, plan_digest,
                    planner_version, identity_workspace_uuid, import_namespace_uuid,
                    provenance_json, imported_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    receipt.receipt_uuid,
                    receipt.format.as_str(),
                    receipt.manifest_digest.as_bytes().as_slice(),
                    receipt.plan_digest.as_bytes().as_slice(),
                    receipt.planner_version,
                    receipt.identity.workspace_uuid,
                    receipt.identity.import_namespace_uuid,
                    serde_json::to_string(&receipt.provenance).map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                    })?,
                    receipt.imported_at,
                ],
            )?;
        }
        transaction.execute("DELETE FROM history_undo", [])?;
        transaction.execute("DELETE FROM history_redo", [])?;
        precommit()?;
        transaction.commit()?;
        Ok(stats)
    })
    .await
}

fn invalid_archive_column(
    column: usize,
    data_type: rusqlite::types::Type,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, data_type, Box::new(error))
}
