//! The MCP endpoint, exercised by a real MCP client over HTTP.
//!
//! What matters here is the seam, not the tool bodies: that the endpoint speaks
//! the protocol, that it refuses callers without the server's bearer token, and
//! that a write arriving this way is sequenced into the oplog like any other —
//! so the note is on the user's devices, not just in the server's database.

mod common;

use common::connect_mcp as connect;
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;

fn arguments(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().expect("an object of arguments")
}

async fn call(
    client: &RunningService<RoleClient, ()>,
    tool: &'static str,
    args: serde_json::Value,
) -> serde_json::Value {
    let result = client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments(args)))
        .await
        .unwrap_or_else(|error| panic!("calling {tool}: {error}"));
    assert_ne!(result.is_error, Some(true), "{tool} failed: {result:?}");
    result
        .structured_content
        .unwrap_or_else(|| panic!("{tool} returned no structured content"))
}

#[tokio::test]
async fn exposes_tools_and_writes_through_the_oplog() {
    let harness = common::start_server().await;

    // A device already following the workspace, so we can prove the note
    // reaches it rather than only landing in the server's own database.
    let replica_directory = tempfile::tempdir().expect("replica directory");
    let replica = notes_core::db::open(replica_directory.path().join("notes.db"))
        .await
        .expect("replica database");
    let sync = harness.sync_transport();
    let snapshot = sync.snapshot().await.expect("server snapshot");
    let cursor = snapshot.seq;
    notes_core::import_sync_snapshot(&replica, snapshot)
        .await
        .expect("joining the workspace");

    let client = connect(&harness, common::TOKEN)
        .await
        .expect("MCP handshake");

    let tools = client.list_all_tools().await.expect("listing tools");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    for expected in [
        "create_note",
        "append_to_journal",
        "append_block",
        "set_block_content",
        "rename_page",
        "search",
        "search_fulltext",
        "list_pages",
        "get_content",
        "read_subtree",
        "read_ancestors",
        "find_backlinks",
        "neighbors",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} is missing from {names:?}"
        );
    }

    let result = client
        .call_tool(
            CallToolRequestParams::new("create_note").with_arguments(arguments(
                serde_json::json!({
                    "title": "Written over MCP",
                    "markdown": "from an assistant",
                }),
            )),
        )
        .await
        .expect("calling create_note");
    assert_ne!(
        result.is_error,
        Some(true),
        "create_note failed: {result:?}"
    );

    let operations = sync
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

    let titles: Vec<String> = notes_core::db::list_pages(&replica, 10)
        .await
        .expect("replica pages")
        .into_iter()
        .filter_map(|page| page.title)
        .collect();
    assert!(
        titles.iter().any(|title| title == "Written over MCP"),
        "the note did not reach the device: {titles:?}"
    );

    client.cancel().await.expect("closing the client");
}

#[tokio::test]
async fn rejects_callers_without_the_servers_token() {
    let harness = common::start_server().await;
    let outcome = connect(&harness, "not-the-configured-token").await;
    assert!(
        outcome.is_err(),
        "the endpoint accepted a client with the wrong token"
    );
}

/// Editing is guarded by revisions, and the guard is the whole point: an
/// assistant working from text it read a minute ago must not silently overwrite
/// what the user typed on their phone in between.
#[tokio::test]
async fn edits_require_the_revision_the_text_was_read_at() {
    let harness = common::start_server().await;
    let client = connect(&harness, common::TOKEN)
        .await
        .expect("MCP handshake");

    let created = call(
        &client,
        "create_note",
        serde_json::json!({ "title": "Draft", "markdown": "first version" }),
    )
    .await;
    let page_uuid = created["uuid"].as_str().expect("page uuid").to_owned();

    let subtree = call(
        &client,
        "read_subtree",
        serde_json::json!({ "uuid": page_uuid, "depth": 1 }),
    )
    .await;
    let block = subtree
        .as_array()
        .expect("a subtree array")
        .iter()
        .find(|node| node["kind"] == "block")
        .expect("the paragraph")
        .clone();
    let block_uuid = block["uuid"].as_str().expect("block uuid").to_owned();
    let first_revision = block["revision"].as_str().expect("revision").to_owned();

    let edit = call(
        &client,
        "set_block_content",
        serde_json::json!({
            "uuid": block_uuid,
            "markdown": "second version",
            "expected_revision": first_revision,
        }),
    )
    .await;
    assert_eq!(edit["changed"], serde_json::json!(true));
    assert_eq!(
        edit["node"]["markdown"],
        serde_json::json!("second version")
    );
    let second_revision = edit["node"]["revision"]
        .as_str()
        .expect("new revision")
        .to_owned();
    assert_ne!(second_revision, first_revision, "the revision must move");

    // Someone else got there first: this caller is still holding the revision
    // from before its own edit.
    let stale = client
        .call_tool(
            CallToolRequestParams::new("set_block_content").with_arguments(arguments(
                serde_json::json!({
                    "uuid": block_uuid,
                    "markdown": "third version, built on stale text",
                    "expected_revision": first_revision,
                }),
            )),
        )
        .await;
    let message = match stale {
        Err(rmcp::ServiceError::McpError(error)) => error.message.to_string(),
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(result) => {
            assert_eq!(result.is_error, Some(true), "the stale write was accepted");
            format!("{result:?}")
        }
    };
    assert!(
        message.contains("changed since editing began"),
        "the refusal should say what happened: {message}"
    );

    // And the block still holds the edit that was made against a fresh
    // revision, not the one built on stale text.
    let current = call(
        &client,
        "get_content",
        serde_json::json!({ "uuid": block_uuid }),
    )
    .await;
    assert_eq!(current["markdown"], serde_json::json!("second version"));

    // The positive control: the same rewrite, offered with the revision the
    // block actually holds, goes through. So the refusal above was about stale
    // text and nothing incidental.
    let retried = call(
        &client,
        "set_block_content",
        serde_json::json!({
            "uuid": block_uuid,
            "markdown": "third version, built on stale text",
            "expected_revision": second_revision,
        }),
    )
    .await;
    assert_eq!(retried["changed"], serde_json::json!(true));

    // Rewriting with the text already there is success without a change.
    let idempotent = call(
        &client,
        "set_block_content",
        serde_json::json!({
            "uuid": block_uuid,
            "markdown": "third version, built on stale text",
            "expected_revision": first_revision,
        }),
    )
    .await;
    assert_eq!(idempotent["changed"], serde_json::json!(false));

    client.cancel().await.expect("closing the client");
}
