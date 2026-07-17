use notes_core::db;
use notes_core::operation::{
    BlockCreate, BlockDelete, BlockMove, BlockSetMarkdown, BlockSetStyle, PageCreate, PageSetTitle,
    PageSetView,
};
use notes_core::{
    BlockStyle, Connection, Hlc, Op, OpKind, OrderKey, Origin, PageView, apply, apply_batch,
};
use proptest::prelude::*;

#[derive(Debug, Clone, PartialEq)]
struct SourceState {
    pages: Vec<PageState>,
    blocks: Vec<BlockState>,
    tombstones: Vec<(uuid::Uuid, String, String, Option<uuid::Uuid>)>,
    structures: Vec<(uuid::Uuid, uuid::Uuid, Option<uuid::Uuid>, String, String)>,
}

type PageState = (
    uuid::Uuid,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);
type BlockState = (
    uuid::Uuid,
    uuid::Uuid,
    Option<uuid::Uuid>,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn database() -> (tempfile::TempDir, Connection) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let connection = db::open(directory.path().join("notes.db"))
        .await
        .expect("open test database");
    (directory, connection)
}

fn op(index: usize, wall_ms: u64, kind: OpKind) -> Op {
    let device_id = uuid::Uuid::from_u128(index as u128 + 100);
    Op {
        op_id: uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_OID,
            format!("convergence-op-{index}").as_bytes(),
        ),
        device_id,
        hlc: Hlc::new(wall_ms, 0, device_id),
        format_version: 2,
        kind,
    }
}

async fn source_state(connection: &Connection) -> SourceState {
    connection
        .call(|database| {
            let pages = database
                .prepare(
                    "SELECT uuid, title, default_view, title_hlc, view_hlc
                       FROM pages ORDER BY uuid",
                )?
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let blocks = database
                .prepare(
                    "SELECT uuid, page_uuid, parent_uuid, order_key, style, markdown,
                            markdown_hlc, style_hlc, structure_hlc
                       FROM blocks ORDER BY uuid",
                )?
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let tombstones = database
                .prepare(
                    "SELECT uuid, object_kind, deleted_hlc, root_page_uuid
                       FROM tombstones ORDER BY uuid",
                )?
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let structures = database
                .prepare(
                    "SELECT block_uuid, page_uuid, parent_uuid, order_key, hlc
                       FROM block_structure_lww ORDER BY block_uuid",
                )?
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(SourceState {
                pages,
                blocks,
                tombstones,
                structures,
            })
        })
        .await
        .expect("read source state")
}

fn build_operation(
    index: usize,
    action: u8,
    clock: u8,
    choose_second: bool,
    page: uuid::Uuid,
    first: uuid::Uuid,
    second: uuid::Uuid,
) -> Op {
    let block = if choose_second { second } else { first };
    let wall_ms = 2_000 + u64::from(clock % 8);
    let kind = match action % 7 {
        0 => OpKind::BlockSetMarkdown(BlockSetMarkdown {
            uuid: block,
            markdown: format!("content-{index}"),
        }),
        1 => OpKind::PageSetTitle(PageSetTitle {
            uuid: page,
            title: Some(format!("title-{index}")),
        }),
        2 => OpKind::BlockMove(BlockMove {
            uuid: first,
            page_uuid: page,
            parent_uuid: choose_second.then_some(second),
            order_key: OrderKey::from_ordinal(usize::from(clock % 3) + 1),
        }),
        3 => OpKind::BlockMove(BlockMove {
            uuid: second,
            page_uuid: page,
            parent_uuid: choose_second.then_some(first),
            order_key: OrderKey::from_ordinal(usize::from(clock % 3) + 1),
        }),
        4 => OpKind::BlockSetStyle(BlockSetStyle {
            uuid: block,
            style: if clock.is_multiple_of(2) {
                BlockStyle::Paragraph
            } else {
                BlockStyle::Bullet
            },
        }),
        5 => OpKind::PageSetView(PageSetView {
            uuid: page,
            default_view: if clock.is_multiple_of(2) {
                PageView::Outline
            } else {
                PageView::Document
            },
        }),
        _ => OpKind::BlockDelete(BlockDelete {
            uuid: block,
            page_uuid: page,
        }),
    };
    op(index + 10, wall_ms, kind)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn concurrent_operation_interleavings_converge(
        cases in prop::collection::vec(
            (any::<u8>(), any::<u8>(), any::<bool>(), any::<u16>(), any::<u16>(), any::<bool>()),
            1..32,
        )
    ) {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        runtime.block_on(async {
            let (_left_dir, left) = database().await;
            let (_right_dir, right) = database().await;
            let page = uuid::Uuid::from_u128(1);
            let first = uuid::Uuid::from_u128(2);
            let second = uuid::Uuid::from_u128(3);
            let initial = vec![
                op(0, 1_000, OpKind::PageCreate(PageCreate {
                    uuid: page,
                    title: Some("Root".into()),
                    default_view: PageView::Outline,
                    created_at: 1,
                })),
                op(1, 1_001, OpKind::BlockCreate(BlockCreate {
                    uuid: first,
                    page_uuid: page,
                    parent_uuid: None,
                    order_key: OrderKey::from_ordinal(1),
                    style: BlockStyle::Paragraph,
                    markdown: String::new(),
                    created_at: 1,
                })),
                op(2, 1_002, OpKind::BlockCreate(BlockCreate {
                    uuid: second,
                    page_uuid: page,
                    parent_uuid: None,
                    order_key: OrderKey::from_ordinal(2),
                    style: BlockStyle::Paragraph,
                    markdown: String::new(),
                    created_at: 1,
                })),
            ];
            for connection in [&left, &right] {
                apply_batch(connection, &initial, Origin::Remote)
                    .await
                    .expect("initial state");
            }

            let operations = cases
                .iter()
                .enumerate()
                .map(|(index, (action, clock, choose_second, _, _, _))| {
                    build_operation(index, *action, *clock, *choose_second, page, first, second)
                })
                .collect::<Vec<_>>();
            let mut left_order = (0..operations.len()).collect::<Vec<_>>();
            left_order.sort_by_key(|index| (cases[*index].3, *index));
            let mut right_order = (0..operations.len()).collect::<Vec<_>>();
            right_order.sort_by_key(|index| (cases[*index].4, *index));

            for index in left_order {
                apply(&left, &operations[index], Origin::Remote)
                    .await
                    .expect("left apply");
                if cases[index].5 {
                    apply(&left, &operations[index], Origin::Remote)
                        .await
                        .expect("left duplicate");
                }
            }
            for index in right_order {
                apply(&right, &operations[index], Origin::Remote)
                    .await
                    .expect("right apply");
                if cases[index].5 {
                    apply(&right, &operations[index], Origin::Remote)
                        .await
                        .expect("right duplicate");
                }
            }

            prop_assert_eq!(source_state(&left).await, source_state(&right).await);
            Ok(())
        })?;
    }
}
