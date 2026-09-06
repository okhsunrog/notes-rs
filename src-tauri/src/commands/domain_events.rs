use super::*;
use notes_core::OpKind;
use std::collections::BTreeSet;

pub(crate) fn events_for_ops(
    operations: &[OpKind],
    graph_changed_content: &[uuid::Uuid],
) -> Vec<DomainEvent> {
    let mut changed_pages = BTreeSet::new();
    let mut deleted_pages = BTreeSet::new();
    let mut changed_blocks = BTreeSet::new();
    let mut deleted_blocks = BTreeSet::new();
    let mut containers = BTreeSet::new();
    let mut structure = BTreeSet::new();
    let mut graph = graph_changed_content
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut attachment_owners = BTreeSet::new();

    for operation in operations {
        match operation {
            OpKind::InkPublish(payload) => {
                changed_pages.insert(payload.page_uuid);
            }
            OpKind::PageCreate(payload) => {
                changed_pages.insert(payload.uuid);
                // A canonical page may adopt an existing unresolved wikilink stub.
                graph.insert(payload.uuid);
            }
            OpKind::PageAliasSet(payload) => {
                changed_pages.insert(payload.uuid);
                graph.insert(payload.uuid);
            }
            OpKind::PageSetTitle(payload) => {
                changed_pages.insert(payload.uuid);
                graph.insert(payload.uuid);
            }
            OpKind::PageSetLayout(payload) => {
                changed_pages.insert(payload.uuid);
            }
            OpKind::PageDelete(payload) => {
                deleted_pages.insert(payload.uuid);
                graph.insert(payload.uuid);
            }
            OpKind::BlockCreate(payload) => {
                changed_blocks.insert(payload.uuid);
                containers.insert(payload.parent_uuid.unwrap_or(payload.page_uuid));
                if notes_core::content_references_changed("", &payload.markdown) {
                    graph.insert(payload.uuid);
                }
            }
            OpKind::BlockSetMarkdown(payload) => {
                changed_blocks.insert(payload.uuid);
            }
            OpKind::BlockSetStyle(payload) => {
                changed_blocks.insert(payload.uuid);
            }
            OpKind::BlockMove(payload) => {
                changed_blocks.insert(payload.uuid);
                containers.insert(payload.parent_uuid.unwrap_or(payload.page_uuid));
                // The old parent is absent from the envelope, so invalidate the structure family.
                structure.insert(payload.uuid);
            }
            OpKind::BlockDelete(payload) => {
                deleted_blocks.insert(payload.uuid);
                containers.insert(payload.page_uuid);
                structure.insert(payload.uuid);
                graph.insert(payload.uuid);
            }
            OpKind::AttachmentAdd(payload) => {
                attachment_owners.insert(payload.owner.uuid());
            }
            OpKind::AttachmentRemove(payload) => {
                attachment_owners.insert(payload.owner.uuid());
            }
        }
    }

    changed_pages.retain(|uuid| !deleted_pages.contains(uuid));
    changed_blocks.retain(|uuid| !deleted_blocks.contains(uuid));

    let mut events = Vec::new();
    push_nonempty(
        &mut events,
        DomainEvent::PagesChanged {
            page_uuids: changed_pages.into_iter().collect(),
        },
    );
    push_nonempty(
        &mut events,
        DomainEvent::BlocksChanged {
            block_uuids: changed_blocks.into_iter().collect(),
            container_uuids: containers.iter().copied().collect(),
        },
    );
    push_nonempty(
        &mut events,
        DomainEvent::PagesDeleted {
            page_uuids: deleted_pages.into_iter().collect(),
        },
    );
    push_nonempty(
        &mut events,
        DomainEvent::BlocksDeleted {
            block_uuids: deleted_blocks.into_iter().collect(),
            container_uuids: containers.into_iter().collect(),
        },
    );
    push_nonempty(
        &mut events,
        DomainEvent::StructureChanged {
            block_uuids: structure.into_iter().collect(),
        },
    );
    push_nonempty(
        &mut events,
        DomainEvent::GraphChanged {
            content_uuids: graph.into_iter().collect(),
        },
    );
    push_nonempty(
        &mut events,
        DomainEvent::AttachmentsChanged {
            owner_uuids: attachment_owners.into_iter().collect(),
        },
    );
    events
}

pub(crate) async fn emit_events_for_ops(
    app: &AppHandle,
    connection: &Connection,
    operations: &[OpKind],
    graph_changed_content: &[uuid::Uuid],
) {
    for event in events_for_ops(operations, graph_changed_content) {
        let event = enrich_changed_block_containers(connection, event).await;
        if !event_is_empty(&event) {
            emit_domain(app, event);
        }
    }
}

async fn enrich_changed_block_containers(
    connection: &Connection,
    event: DomainEvent,
) -> DomainEvent {
    let DomainEvent::BlocksChanged {
        block_uuids,
        container_uuids,
    } = event
    else {
        return event;
    };
    match db::get_blocks(connection, block_uuids.clone()).await {
        Ok(blocks) => DomainEvent::BlocksChanged {
            block_uuids: blocks.iter().map(|block| block.uuid).collect(),
            container_uuids: blocks
                .iter()
                .map(|block| block.parent_uuid.unwrap_or(block.page_uuid))
                .chain(container_uuids)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        },
        Err(error) => {
            tracing::warn!(%error, "resolving changed block containers failed");
            DomainEvent::BlocksChanged {
                block_uuids,
                container_uuids,
            }
        }
    }
}

fn push_nonempty(events: &mut Vec<DomainEvent>, event: DomainEvent) {
    if !event_is_empty(&event) {
        events.push(event);
    }
}

fn event_is_empty(event: &DomainEvent) -> bool {
    match event {
        DomainEvent::PagesChanged { page_uuids } | DomainEvent::PagesDeleted { page_uuids } => {
            page_uuids.is_empty()
        }
        DomainEvent::BlocksChanged { block_uuids, .. }
        | DomainEvent::BlocksDeleted { block_uuids, .. }
        | DomainEvent::StructureChanged { block_uuids } => block_uuids.is_empty(),
        DomainEvent::GraphChanged { content_uuids } => content_uuids.is_empty(),
        DomainEvent::AttachmentsChanged { owner_uuids } => owner_uuids.is_empty(),
        DomainEvent::HistoryChanged
        | DomainEvent::SettingsChanged
        | DomainEvent::SyncStatusChanged
        | DomainEvent::ServerAiChanged
        | DomainEvent::WorkspaceChanged => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notes_core::{
        AttachmentAdd, AttachmentOwner, AttachmentRemove, BlobHash, BlockCreate, BlockDelete,
        BlockMove, BlockSetMarkdown, BlockSetStyle, OrderKey, PageAlias, PageCreate, PageDelete,
        PageKind, PageSetLayout, PageSetTitle,
    };

    #[test]
    fn every_operation_kind_maps_to_at_least_one_domain_event() {
        let uuid = uuid::Uuid::from_u128(1);
        let owner = AttachmentOwner::Page(uuid);
        let kinds = vec![
            OpKind::PageCreate(PageCreate {
                uuid,
                kind: PageKind::Note,
                title: Some("Page".into()),
                layout: PageLayout::Outline,
                created_at: 1,
            }),
            OpKind::PageAliasSet(notes_core::PageAliasSet {
                uuid,
                alias: PageAlias::new("alias").unwrap(),
                present: true,
            }),
            OpKind::PageSetTitle(PageSetTitle {
                uuid,
                title: Some("Renamed".into()),
            }),
            OpKind::PageSetLayout(PageSetLayout {
                uuid,
                layout: PageLayout::Document,
            }),
            OpKind::PageDelete(PageDelete { uuid }),
            OpKind::BlockCreate(BlockCreate {
                uuid,
                page_uuid: uuid,
                parent_uuid: None,
                order_key: OrderKey::first(),
                style: BlockStyle::Paragraph,
                markdown: String::new(),
                created_at: 1,
            }),
            OpKind::BlockSetMarkdown(BlockSetMarkdown {
                uuid,
                markdown: "text".into(),
            }),
            OpKind::BlockSetStyle(BlockSetStyle {
                uuid,
                style: BlockStyle::Bullet,
            }),
            OpKind::BlockMove(BlockMove {
                uuid,
                page_uuid: uuid,
                parent_uuid: None,
                order_key: OrderKey::first(),
            }),
            OpKind::BlockDelete(BlockDelete {
                uuid,
                page_uuid: uuid,
            }),
            OpKind::AttachmentAdd(AttachmentAdd {
                owner,
                blob_hash: BlobHash::digest(b"attachment"),
                filename: "attachment.txt".into(),
                mime: "text/plain".into(),
                size: 10,
            }),
            OpKind::AttachmentRemove(AttachmentRemove {
                owner,
                blob_hash: BlobHash::digest(b"attachment"),
            }),
        ];

        for kind in kinds {
            assert!(!events_for_ops(std::slice::from_ref(&kind), &[]).is_empty());
        }
    }

    #[test]
    fn markdown_only_invalidates_graph_when_application_reports_reference_changes() {
        let uuid = uuid::Uuid::from_u128(1);
        let operation = OpKind::BlockSetMarkdown(BlockSetMarkdown {
            uuid,
            markdown: "edited text".into(),
        });

        assert!(
            !events_for_ops(std::slice::from_ref(&operation), &[])
                .iter()
                .any(|event| matches!(event, DomainEvent::GraphChanged { .. }))
        );
        assert!(events_for_ops(&[operation], &[uuid]).iter().any(|event| {
            matches!(
                event,
                DomainEvent::GraphChanged { content_uuids } if content_uuids == &[uuid]
            )
        }));
    }
}
