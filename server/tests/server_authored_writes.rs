//! The server authors operations of its own — today from the chat agent, next
//! from the MCP tools. Those writes take a different route into the oplog than a
//! client's `POST /v1/ops`, and for a while they took no route at all: they
//! landed in the server's materialized database and stopped there, invisible to
//! every device. These tests pin the route down.

mod common;

use common::TOKEN;
use notes_ai::agent::GraphWriter;

#[tokio::test]
async fn a_server_authored_page_is_sequenced_before_the_write_returns() {
    let harness = common::start_server().await;
    let user = harness
        .state
        .registry
        .authenticate(TOKEN)
        .expect("authenticated user");

    // A device that has already joined the workspace, as one would be when the
    // assistant writes: bootstrapped from the server, following the oplog.
    let replica_directory = tempfile::tempdir().expect("replica directory");
    let replica = notes_core::db::open(replica_directory.path().join("notes.db"))
        .await
        .expect("replica database");
    let transport = harness.sync_transport();
    let snapshot = transport.snapshot().await.expect("server snapshot");
    let cursor = snapshot.seq;
    notes_core::import_sync_snapshot(&replica, snapshot)
        .await
        .expect("joining the workspace");

    let page = GraphWriter::create_page(
        &*user,
        "Captured by the assistant".into(),
        "the first paragraph".into(),
    )
    .await
    .expect("server-authored write");

    // No sleep, no waiting on the sweeper: the write is only complete once its
    // operations carry a sequence number, so a device polling the instant it
    // returns already sees them.
    let operations = transport
        .ops_since(cursor, 256)
        .await
        .expect("pulling operations");
    assert_eq!(
        operations.len(),
        2,
        "expected the page and its paragraph to be sequenced, got {operations:?}"
    );

    for operation in &operations {
        notes_core::apply_sequenced(&replica, operation.seq, &operation.envelope)
            .await
            .expect("applying to the replica");
    }

    let mirrored = notes_core::db::get_page(&replica, page.uuid)
        .await
        .expect("reading the replica")
        .expect("the page reached the replica");
    assert_eq!(mirrored.title.as_deref(), Some("Captured by the assistant"));
}

#[tokio::test]
async fn server_authored_operations_leave_nothing_stranded() {
    let harness = common::start_server().await;
    let user = harness
        .state
        .registry
        .authenticate(TOKEN)
        .expect("authenticated user");

    GraphWriter::create_page(&*user, "Stranding check".into(), String::new())
        .await
        .expect("server-authored write");

    // The invariant the server audits on every open: an applied operation is
    // either queued for publication or already sequenced. Anything else is a
    // write the materialized state remembers and the log does not, and it
    // cannot be reconstructed — only the envelope carries the HLC.
    let stranded: i64 = user
        .notes
        .call(|database| {
            database.query_row(
                "SELECT COUNT(*) FROM applied_ops
                 WHERE seq IS NULL AND op_id NOT IN (SELECT op_id FROM sync_outbox)",
                [],
                |row| row.get(0),
            )
        })
        .await
        .expect("auditing the replica");
    assert_eq!(stranded, 0, "server-authored operations were stranded");
}
