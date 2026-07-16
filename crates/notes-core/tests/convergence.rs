use notes_core::db;
use notes_core::operation::{
    EdgeAdd, EdgeRemove, NodeCreate, NodeDelete, NodeMove, NodeSetContent, NodeSetTitle,
};
use notes_core::{Connection, Hlc, Op, OpKind, Origin, apply, apply_batch};
use proptest::prelude::*;

type NodeState = (
    i64,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
);

#[derive(Debug, Clone, PartialEq)]
struct SourceState {
    nodes: Vec<NodeState>,
    edges: Vec<(String, String, String, i64, i64)>,
    tombstones: Vec<(String, String, Option<String>)>,
    edge_lww: Vec<(String, String, String, String, bool, i64)>,
}

async fn database() -> (tempfile::TempDir, Connection) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let connection = db::open(directory.path().join("notes.db"), "test", 8)
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
        )
        .to_string(),
        device_id: device_id.to_string(),
        hlc: Hlc::new(wall_ms, 0, device_id),
        format_version: 1,
        kind,
    }
}

fn create(
    index: usize,
    uuid: &str,
    kind: &str,
    parent_uuid: Option<&str>,
    position: Option<f64>,
) -> Op {
    op(
        index,
        1_000 + index as u64,
        OpKind::NodeCreate(NodeCreate {
            uuid: uuid.into(),
            node_kind: kind.parse().expect("valid test node kind"),
            title: (kind == "page").then(|| "Root".into()),
            content: String::new(),
            content_json: None,
            parent_uuid: parent_uuid.map(str::to_owned),
            position,
            created_at: 1,
        }),
    )
}

async fn source_state(connection: &Connection) -> SourceState {
    connection
        .call(|database| {
            let nodes = {
                let mut statement = database.prepare(
                    "SELECT n.id, n.uuid, n.kind, n.title, n.content, parent.uuid,
                            CAST(n.position * 1000 AS INTEGER),
                            n.content_hlc, n.title_hlc, n.structure_hlc,
                            n.created_at, n.updated_at
                       FROM nodes n LEFT JOIN nodes parent ON parent.id = n.parent_id
                      WHERE n.kind IN ('page', 'block') ORDER BY n.uuid",
                )?;
                statement
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
                            row.get(9)?,
                            row.get(10)?,
                            row.get(11)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let edges = {
                let mut statement = database.prepare(
                    "SELECT src.uuid, dst.uuid, e.kind, CAST(e.weight * 1000 AS INTEGER),
                            e.created_at
                       FROM edges e JOIN nodes src ON src.id = e.src
                       JOIN nodes dst ON dst.id = e.dst
                      WHERE e.kind NOT IN ('refs', 'attachment')
                      ORDER BY src.uuid, dst.uuid, e.kind",
                )?;
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let tombstones = {
                let mut statement = database
                    .prepare("SELECT uuid, deleted_hlc, root_uuid FROM tombstones ORDER BY uuid")?;
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let edge_lww = {
                let mut statement = database.prepare(
                    "SELECT src_uuid, dst_uuid, kind, hlc, present,
                            CAST(weight * 1000 AS INTEGER)
                       FROM edge_lww ORDER BY src_uuid, dst_uuid, kind",
                )?;
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(SourceState {
                nodes,
                edges,
                tombstones,
                edge_lww,
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
    page: &str,
    first: &str,
    second: &str,
) -> Op {
    let node = if choose_second { second } else { first };
    let wall_ms = 2_000 + u64::from(clock % 8);
    let kind = match action % 7 {
        0 => OpKind::NodeSetContent(NodeSetContent {
            uuid: node.into(),
            content: format!("content-{index}"),
            content_json: None,
        }),
        1 => OpKind::NodeSetTitle(NodeSetTitle {
            uuid: page.into(),
            title: Some(format!("title-{index}")),
        }),
        2 => OpKind::NodeMove(NodeMove {
            uuid: first.into(),
            parent_uuid: Some(if choose_second { second } else { page }.into()),
            position: f64::from(clock % 3),
        }),
        3 => OpKind::NodeMove(NodeMove {
            uuid: second.into(),
            parent_uuid: Some(if choose_second { first } else { page }.into()),
            position: f64::from(clock % 3),
        }),
        4 => OpKind::EdgeAdd(EdgeAdd {
            src_uuid: first.into(),
            dst_uuid: second.into(),
            edge_kind: "relates_to".into(),
            weight: f64::from(clock % 10) / 10.0,
        }),
        5 => OpKind::EdgeRemove(EdgeRemove {
            src_uuid: first.into(),
            dst_uuid: second.into(),
            edge_kind: "relates_to".into(),
        }),
        _ => OpKind::NodeDelete(NodeDelete { uuid: node.into() }),
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
            let page = uuid::Uuid::from_u128(1).to_string();
            let first = uuid::Uuid::from_u128(2).to_string();
            let second = uuid::Uuid::from_u128(3).to_string();
            let initial = vec![
                create(0, &page, "page", None, None),
                create(1, &first, "block", Some(&page), Some(1.0)),
                create(2, &second, "block", Some(&page), Some(2.0)),
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
                    build_operation(index, *action, *clock, *choose_second, &page, &first, &second)
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
