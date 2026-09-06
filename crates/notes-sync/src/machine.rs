use anyhow::{Result, bail};
use async_trait::async_trait;
use notes_core::{
    Connection, Op, acknowledge_server_ops, apply_sequenced_batch, configure_sync, pending_outbox,
    quarantine_outbox_op, sync_cursor,
};
use notes_protocol::SequencedOp;
use std::collections::HashMap;

#[async_trait]
pub trait SyncTransport: Send {
    async fn ops_since(&mut self, since: u64, limit: usize) -> Result<Vec<SequencedOp>>;
    async fn push(&mut self, operations: Vec<Op>) -> Result<Vec<SequencedOp>>;

    async fn prepare_push(&mut self, _operations: &[Op]) -> Result<()> {
        Ok(())
    }

    async fn prepare_pull(&mut self, _operations: &[SequencedOp]) -> Result<()> {
        Ok(())
    }
}

/// Deterministic in-memory transport used before the HTTP/WS server exists.
/// It models idempotent ingest and one gapless sequence shared by all clients.
#[derive(Debug, Clone, Default)]
pub struct LoopbackServer {
    workspace_uuid: Option<uuid::Uuid>,
    log: Vec<SequencedOp>,
    by_op_id: HashMap<uuid::Uuid, u64>,
}

impl LoopbackServer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_log(log: Vec<SequencedOp>) -> Result<Self> {
        let mut server = Self::new();
        for (index, item) in log.into_iter().enumerate() {
            let expected = index as u64 + 1;
            if item.seq != expected {
                bail!(
                    "loopback oplog has a gap: expected {expected}, got {}",
                    item.seq
                );
            }
            if server
                .by_op_id
                .insert(item.envelope.op_id, item.seq)
                .is_some()
            {
                bail!("loopback oplog contains a duplicate op_id");
            }
            match server.workspace_uuid {
                Some(workspace_uuid) if workspace_uuid != item.envelope.workspace_uuid => {
                    bail!("loopback oplog contains operations from multiple workspaces");
                }
                None => server.workspace_uuid = Some(item.envelope.workspace_uuid),
                Some(_) => {}
            }
            server.log.push(item);
        }
        Ok(server)
    }

    pub fn ingest(&mut self, envelopes: impl IntoIterator<Item = Op>) -> Result<Vec<SequencedOp>> {
        envelopes
            .into_iter()
            .map(|envelope| {
                match self.workspace_uuid {
                    Some(workspace_uuid) if workspace_uuid != envelope.workspace_uuid => {
                        bail!(
                            "operation belongs to workspace {}, but the loopback server owns {}",
                            envelope.workspace_uuid,
                            workspace_uuid
                        );
                    }
                    None => self.workspace_uuid = Some(envelope.workspace_uuid),
                    Some(_) => {}
                }
                if let Some(seq) = self.by_op_id.get(&envelope.op_id).copied() {
                    return Ok(self.log[(seq - 1) as usize].clone());
                }
                let seq = self.log.len() as u64 + 1;
                let item = SequencedOp { seq, envelope };
                self.by_op_id.insert(item.envelope.op_id, seq);
                self.log.push(item.clone());
                Ok(item)
            })
            .collect()
    }

    pub fn ops_since(&self, seq: u64, limit: usize) -> Vec<SequencedOp> {
        self.log
            .iter()
            .skip(seq.min(self.log.len() as u64) as usize)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn log(&self) -> &[SequencedOp] {
        &self.log
    }
}

#[async_trait]
impl SyncTransport for LoopbackServer {
    async fn ops_since(&mut self, since: u64, limit: usize) -> Result<Vec<SequencedOp>> {
        Ok(LoopbackServer::ops_since(self, since, limit))
    }

    async fn push(&mut self, operations: Vec<Op>) -> Result<Vec<SequencedOp>> {
        self.ingest(operations)
    }
}

fn quarantine_reason(message: &str) -> String {
    if message.trim().is_empty() {
        "The server rejected this change.".into()
    } else {
        message.trim().to_owned()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            elapsed.as_millis().try_into().unwrap_or(i64::MAX)
        })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncStats {
    pub pushed: usize,
    pub received: usize,
    pub applied: usize,
    pub cursor: u64,
    /// Remote operations that changed this replica during the sync pass.
    /// Hosts use this to invalidate only the affected UI queries.
    pub applied_operations: Vec<AppliedRemoteOperation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppliedRemoteOperation {
    pub operation: Op,
    pub graph_changed: bool,
}

#[derive(Clone)]
pub struct SyncClient {
    conn: Connection,
    batch_size: u32,
}

impl SyncClient {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn,
            batch_size: 256,
        }
    }

    pub async fn enable(conn: Connection, server_url: &str) -> Result<Self> {
        configure_sync(&conn, server_url).await?;
        Ok(Self::new(conn))
    }

    pub fn with_batch_size(mut self, batch_size: u32) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    pub async fn sync_once<T: SyncTransport>(&self, transport: &mut T) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        self.catch_up(transport, &mut stats).await?;

        let outbox = pending_outbox(&self.conn, self.batch_size).await?;
        stats.pushed = outbox.len();
        if !outbox.is_empty() {
            transport.prepare_push(&outbox).await?;
            let accepted = self.push_batch(transport, outbox).await?;
            acknowledge_server_ops(
                &self.conn,
                accepted
                    .iter()
                    .map(|operation| (operation.envelope.op_id, operation.seq))
                    .collect(),
            )
            .await?;
        }

        self.catch_up(transport, &mut stats).await?;
        stats.cursor = sync_cursor(&self.conn).await?;
        Ok(stats)
    }

    pub async fn sync_until_idle<T: SyncTransport>(&self, transport: &mut T) -> Result<SyncStats> {
        let mut total = SyncStats::default();
        loop {
            let stats = self.sync_once(transport).await?;
            total.pushed += stats.pushed;
            total.received += stats.received;
            total.applied += stats.applied;
            total.cursor = stats.cursor;
            total.applied_operations.extend(stats.applied_operations);
            if stats.pushed < self.batch_size as usize
                && transport.ops_since(stats.cursor, 1).await?.is_empty()
            {
                return Ok(total);
            }
        }
    }

    async fn push_batch<T: SyncTransport>(
        &self,
        transport: &mut T,
        outbox: Vec<Op>,
    ) -> Result<Vec<SequencedOp>> {
        match transport.push(outbox.clone()).await {
            Ok(accepted) => Ok(accepted),
            Err(error) if crate::terminal_rejection(&error).is_some() => {
                self.isolate_rejected_batch(transport, outbox).await
            }
            Err(error) => Err(error),
        }
    }

    /// Finds the one operation a refused batch is stuck on and holds only that
    /// one back.
    ///
    /// The server judges a batch as a whole, so a single unacceptable
    /// operation used to block every later change: the client reconnected,
    /// resent the same head, and was refused again forever. Resending the
    /// operations one at a time names the offender; it is quarantined with the
    /// server's own reason — kept, never deleted — and everything after it
    /// reaches the server on this same pass.
    async fn isolate_rejected_batch<T: SyncTransport>(
        &self,
        transport: &mut T,
        outbox: Vec<Op>,
    ) -> Result<Vec<SequencedOp>> {
        let mut accepted = Vec::new();
        let mut progressed = false;
        for operation in outbox {
            match transport.push(vec![operation.clone()]).await {
                Ok(mut ops) => {
                    accepted.append(&mut ops);
                    progressed = true;
                }
                Err(error) => match crate::terminal_rejection(&error) {
                    Some(reason) => {
                        quarantine_outbox_op(
                            &self.conn,
                            operation.op_id,
                            quarantine_reason(&reason),
                            now_ms(),
                        )
                        .await?;
                        progressed = true;
                    }
                    // Transient: a dropped connection is not a rejection. The
                    // rest of the batch stays queued for the next pass.
                    None if progressed => break,
                    None => return Err(error),
                },
            }
        }
        Ok(accepted)
    }

    async fn catch_up<T: SyncTransport>(
        &self,
        transport: &mut T,
        stats: &mut SyncStats,
    ) -> Result<()> {
        loop {
            let cursor = sync_cursor(&self.conn).await?;
            let batch = transport
                .ops_since(cursor, self.batch_size as usize)
                .await?;
            if batch.is_empty() {
                return Ok(());
            }
            let full_batch = batch.len() == self.batch_size as usize;
            transport.prepare_pull(&batch).await?;
            let sequenced = batch
                .iter()
                .map(|item| (item.seq, item.envelope.clone()))
                .collect::<Vec<_>>();
            let outcomes = apply_sequenced_batch(&self.conn, sequenced).await?;
            stats.received += batch.len();
            stats.applied += outcomes.iter().filter(|outcome| outcome.applied).count();
            stats.applied_operations.extend(
                batch
                    .into_iter()
                    .zip(outcomes)
                    .filter(|(_, outcome)| outcome.applied)
                    .map(|(operation, outcome)| AppliedRemoteOperation {
                        graph_changed: outcome.graph_changed,
                        operation: operation.envelope,
                    }),
            );
            if !full_batch {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notes_core::{BlockStyle, db, export_sync_snapshot, import_sync_snapshot};

    /// A loopback server that refuses named operations the way the real one
    /// refuses an incomplete handwriting publication: 409, batch and all.
    struct RejectingServer {
        inner: LoopbackServer,
        rejected: std::collections::HashSet<uuid::Uuid>,
        reason: String,
    }

    #[async_trait]
    impl SyncTransport for RejectingServer {
        async fn ops_since(&mut self, since: u64, limit: usize) -> Result<Vec<SequencedOp>> {
            Ok(self.inner.ops_since(since, limit))
        }

        async fn push(&mut self, operations: Vec<Op>) -> Result<Vec<SequencedOp>> {
            if operations
                .iter()
                .any(|operation| self.rejected.contains(&operation.op_id))
            {
                return Err(crate::TransportError::Conflict(self.reason.clone()).into());
            }
            self.inner.ingest(operations)
        }
    }

    fn ink_patch() -> notes_core::ink::InkDraftPatch {
        let stroke = notes_core::ink::InkStroke {
            id: uuid::Uuid::now_v7(),
            width: 2.,
            points: (0..8)
                .map(|n| notes_core::ink::InkPoint {
                    x: f64::from(n),
                    y: f64::from(n),
                    pressure: 0.5,
                    tilt_x: 0.,
                    tilt_y: 0.,
                    time: f64::from(n),
                })
                .collect(),
        };
        notes_core::ink::InkDraftPatch {
            order: vec![stroke.id],
            upserts: vec![stroke],
            background: notes_core::ink::InkBackground::Plain,
        }
    }

    #[tokio::test]
    async fn a_refused_operation_is_quarantined_so_later_changes_still_reach_the_server() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("notes.db");
        let connection = db::open(&path).await.expect("open database");
        let client = SyncClient::enable(connection.clone(), "loopback://quarantine")
            .await
            .expect("enable sync");

        let note = db::create_handwritten_note_with_ops(&connection, Some("Ink".into()))
            .await
            .expect("handwritten note")
            .value
            .uuid;
        let store = notes_core::ink::Store::new(&path, note);
        store.patch(ink_patch(), None).expect("write a stroke");
        while store.compact().expect("compact") {}
        store
            .publish("Book".into())
            .expect("publish")
            .expect("head");

        let publication = pending_outbox(&connection, 100)
            .await
            .expect("outbox")
            .into_iter()
            .find(|operation| matches!(operation.kind, notes_core::OpKind::InkPublish(_)))
            .expect("a publication is queued");
        let mut server = RejectingServer {
            inner: LoopbackServer::new(),
            rejected: std::collections::HashSet::from([publication.op_id]),
            reason: "Upload handwriting root before publication".into(),
        };

        client
            .sync_until_idle(&mut server)
            .await
            .expect("a refused batch is not a sync failure");

        let held = notes_core::quarantined_outbox(&connection, 100)
            .await
            .expect("quarantine");
        assert_eq!(held.len(), 1, "only the offending operation is held back");
        assert_eq!(held[0].op.op_id, publication.op_id);
        assert_eq!(held[0].reason, "Upload handwriting root before publication");
        assert!(
            pending_outbox(&connection, 100)
                .await
                .expect("outbox")
                .is_empty(),
            "everything else in the batch was accepted"
        );

        // A later, unrelated change is no longer stuck behind the rejection.
        db::create_page(&connection, "Written after".into())
            .await
            .expect("text note");
        client.sync_until_idle(&mut server).await.expect("sync");
        assert!(
            server.inner.log().iter().any(|item| matches!(
                &item.envelope.kind,
                notes_core::OpKind::PageCreate(page)
                    if page.title.as_deref() == Some("Written after")
            )),
            "a text change after the rejection must still reach the server"
        );

        // Once the cause is gone the held operation is sent, not discarded.
        server.rejected.clear();
        assert_eq!(
            notes_core::release_quarantined_outbox(&connection)
                .await
                .expect("release"),
            1
        );
        client.sync_until_idle(&mut server).await.expect("resync");
        assert!(
            notes_core::quarantined_outbox(&connection, 100)
                .await
                .expect("quarantine")
                .is_empty()
        );
        assert!(
            server
                .inner
                .log()
                .iter()
                .any(|item| item.envelope.op_id == publication.op_id),
            "the retried publication reaches the server unchanged"
        );
    }

    /// A dropped connection is not a verdict on the batch.
    #[tokio::test]
    async fn a_transient_failure_never_quarantines_anything() {
        struct Offline;

        #[async_trait]
        impl SyncTransport for Offline {
            async fn ops_since(&mut self, _: u64, _: usize) -> Result<Vec<SequencedOp>> {
                Ok(Vec::new())
            }

            async fn push(&mut self, _: Vec<Op>) -> Result<Vec<SequencedOp>> {
                Err(crate::TransportError::Http {
                    status: 503,
                    code: None,
                    message: "restarting".into(),
                }
                .into())
            }
        }

        let (_directory, connection, client) = client("transient").await;
        db::create_page(&connection, "Kept".into())
            .await
            .expect("edit");
        assert!(client.sync_once(&mut Offline).await.is_err());
        assert!(
            notes_core::quarantined_outbox(&connection, 10)
                .await
                .expect("quarantine")
                .is_empty()
        );
        assert_eq!(
            pending_outbox(&connection, 10).await.expect("outbox").len(),
            1
        );
    }

    async fn client(name: &str) -> (tempfile::TempDir, Connection, SyncClient) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let connection = db::open(directory.path().join("notes.db"))
            .await
            .expect("open database");
        let client = SyncClient::enable(connection.clone(), &format!("loopback://{name}"))
            .await
            .expect("enable sync");
        (directory, connection, client)
    }

    #[tokio::test]
    async fn offline_edits_and_echo_delivery_converge_two_clients() {
        let (_left_dir, left, left_sync) = client("test").await;
        let (_right_dir, right, right_sync) = client("test").await;
        let mut server = LoopbackServer::new();

        let shared_workspace = export_sync_snapshot(&left, 0)
            .await
            .expect("export shared workspace identity");
        import_sync_snapshot(&right, shared_workspace)
            .await
            .expect("adopt shared workspace identity");

        db::create_page(&left, "Left".into())
            .await
            .expect("left edit");
        db::create_page(&right, "Right".into())
            .await
            .expect("right edit");

        let left_stats = left_sync
            .sync_until_idle(&mut server)
            .await
            .expect("sync left");
        assert_eq!(left_stats.pushed, 1);
        let right_stats = right_sync
            .sync_until_idle(&mut server)
            .await
            .expect("sync right");
        assert_eq!(right_stats.pushed, 1);
        left_sync
            .sync_until_idle(&mut server)
            .await
            .expect("catch left up");

        for connection in [&left, &right] {
            let mut titles = db::list_pages(connection, 10)
                .await
                .expect("list pages")
                .into_iter()
                .map(|page| page.title.expect("page title"))
                .collect::<Vec<_>>();
            titles.sort();
            assert_eq!(titles, vec!["Left", "Right"]);
            assert!(
                pending_outbox(connection, 10)
                    .await
                    .expect("outbox")
                    .is_empty()
            );
            assert_eq!(sync_cursor(connection).await.expect("cursor"), 2);
        }
    }

    #[tokio::test]
    async fn restarted_server_keeps_gapless_sequence_and_dedupes_retries() {
        let (_directory, connection, client) = client("restart").await;
        let mut server = LoopbackServer::new();
        db::create_page(&connection, "Before restart".into())
            .await
            .expect("first edit");
        client.sync_once(&mut server).await.expect("first sync");
        let first = server.log()[0].envelope.clone();

        let mut server = LoopbackServer::from_log(server.log().to_vec()).expect("restart server");
        let duplicate = server.ingest([first]).expect("deduplicate retry");
        assert_eq!(duplicate[0].seq, 1);
        db::create_page(&connection, "After restart".into())
            .await
            .expect("second edit");
        client.sync_once(&mut server).await.expect("second sync");
        assert_eq!(
            server.log().iter().map(|item| item.seq).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[tokio::test]
    async fn snapshot_bootstrap_plus_tail_replay_matches_full_log() {
        let (_source_dir, source, source_sync) = client("snapshot").await;
        let mut server = LoopbackServer::new();
        let empty_workspace = export_sync_snapshot(&source, 0)
            .await
            .expect("export empty workspace identity");
        let page = db::create_page(&source, "Snapshot".into())
            .await
            .expect("create page");
        source_sync
            .sync_until_idle(&mut server)
            .await
            .expect("publish snapshot floor");
        let snapshot = export_sync_snapshot(&source, 1)
            .await
            .expect("export sync snapshot");

        db::create_block(
            &source,
            page.uuid,
            None,
            None,
            BlockStyle::Paragraph,
            "tail edit".into(),
        )
        .await
        .expect("create tail block");
        source_sync
            .sync_until_idle(&mut server)
            .await
            .expect("publish tail");

        let (_full_dir, full, full_sync) = client("snapshot").await;
        import_sync_snapshot(&full, empty_workspace)
            .await
            .expect("adopt source workspace identity");
        full_sync
            .sync_until_idle(&mut server)
            .await
            .expect("full replay");

        let (_bootstrap_dir, bootstrap) = database_for_snapshot().await;
        import_sync_snapshot(&bootstrap, snapshot)
            .await
            .expect("import snapshot");
        let bootstrap_sync = SyncClient::enable(bootstrap.clone(), "loopback://snapshot")
            .await
            .expect("enable bootstrapped client");
        bootstrap_sync
            .sync_until_idle(&mut server)
            .await
            .expect("replay snapshot tail");

        assert_eq!(
            export_sync_snapshot(&full, 2).await.expect("full state"),
            export_sync_snapshot(&bootstrap, 2)
                .await
                .expect("bootstrapped state")
        );
    }

    async fn database_for_snapshot() -> (tempfile::TempDir, Connection) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let connection = db::open(directory.path().join("notes.db"))
            .await
            .expect("open snapshot database");
        (directory, connection)
    }
}
