use anyhow::{Result, bail};
use notes_core::{
    Connection, Op, acknowledge_server_op, apply_sequenced, configure_sync, pending_outbox,
    sync_cursor,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SequencedOp {
    pub seq: u64,
    pub envelope: Op,
}

/// Deterministic in-memory transport used before the HTTP/WS server exists.
/// It models idempotent ingest and one gapless sequence shared by all clients.
#[derive(Debug, Clone, Default)]
pub struct LoopbackServer {
    log: Vec<SequencedOp>,
    by_op_id: HashMap<String, u64>,
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
                .insert(item.envelope.op_id.clone(), item.seq)
                .is_some()
            {
                bail!("loopback oplog contains a duplicate op_id");
            }
            server.log.push(item);
        }
        Ok(server)
    }

    pub fn ingest(&mut self, envelopes: impl IntoIterator<Item = Op>) -> Vec<SequencedOp> {
        envelopes
            .into_iter()
            .map(|envelope| {
                if let Some(seq) = self.by_op_id.get(&envelope.op_id).copied() {
                    return self.log[(seq - 1) as usize].clone();
                }
                let seq = self.log.len() as u64 + 1;
                let item = SequencedOp { seq, envelope };
                self.by_op_id.insert(item.envelope.op_id.clone(), seq);
                self.log.push(item.clone());
                item
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub pushed: usize,
    pub received: usize,
    pub applied: usize,
    pub cursor: u64,
}

#[derive(Clone)]
pub struct SyncClient {
    conn: Connection,
    batch_size: u32,
}

impl SyncClient {
    pub async fn enable(conn: Connection, server_url: &str) -> Result<Self> {
        configure_sync(&conn, server_url).await?;
        Ok(Self {
            conn,
            batch_size: 256,
        })
    }

    pub fn with_batch_size(mut self, batch_size: u32) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    pub async fn sync_once(&self, server: &mut LoopbackServer) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        self.catch_up(server, &mut stats).await?;

        let outbox = pending_outbox(&self.conn, self.batch_size).await?;
        stats.pushed = outbox.len();
        for accepted in server.ingest(outbox) {
            // A transport ack prunes the outbox; the server echo remains in
            // the log and exercises idempotent redelivery on catch-up.
            acknowledge_server_op(&self.conn, &accepted.envelope.op_id, accepted.seq).await?;
        }

        self.catch_up(server, &mut stats).await?;
        stats.cursor = sync_cursor(&self.conn).await?;
        Ok(stats)
    }

    pub async fn sync_until_idle(&self, server: &mut LoopbackServer) -> Result<SyncStats> {
        let mut total = SyncStats::default();
        loop {
            let stats = self.sync_once(server).await?;
            total.pushed += stats.pushed;
            total.received += stats.received;
            total.applied += stats.applied;
            total.cursor = stats.cursor;
            if stats.pushed < self.batch_size as usize
                && server.ops_since(stats.cursor, 1).is_empty()
            {
                return Ok(total);
            }
        }
    }

    async fn catch_up(&self, server: &LoopbackServer, stats: &mut SyncStats) -> Result<()> {
        loop {
            let cursor = sync_cursor(&self.conn).await?;
            let batch = server.ops_since(cursor, self.batch_size as usize);
            if batch.is_empty() {
                return Ok(());
            }
            let full_batch = batch.len() == self.batch_size as usize;
            for item in batch {
                let outcome = apply_sequenced(&self.conn, item.seq, &item.envelope).await?;
                stats.received += 1;
                stats.applied += usize::from(outcome.applied);
            }
            if !full_batch {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notes_core::{db, export_sync_snapshot, import_sync_snapshot};

    async fn client(name: &str) -> (tempfile::TempDir, Connection, SyncClient) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let connection = db::open(directory.path().join("notes.db"), "test", 8)
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

        db::create_node(
            &left,
            "page".into(),
            Some("Left".into()),
            "offline left".into(),
            None,
        )
        .await
        .expect("left edit");
        db::create_node(
            &right,
            "page".into(),
            Some("Right".into()),
            "offline right".into(),
            None,
        )
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
                .map(|node| node.title.expect("page title"))
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
        db::create_node(
            &connection,
            "page".into(),
            Some("Before restart".into()),
            String::new(),
            None,
        )
        .await
        .expect("first edit");
        client.sync_once(&mut server).await.expect("first sync");
        let first = server.log()[0].envelope.clone();

        let mut server = LoopbackServer::from_log(server.log().to_vec()).expect("restart server");
        let duplicate = server.ingest([first]);
        assert_eq!(duplicate[0].seq, 1);
        db::create_node(
            &connection,
            "page".into(),
            Some("After restart".into()),
            String::new(),
            None,
        )
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
        let page = db::create_node(
            &source,
            "page".into(),
            Some("Snapshot".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        source_sync
            .sync_until_idle(&mut server)
            .await
            .expect("publish snapshot floor");
        let snapshot = export_sync_snapshot(&source, 1)
            .await
            .expect("export sync snapshot");

        db::create_block(&source, Some(page.id), None, "tail edit".into(), None)
            .await
            .expect("create tail block");
        source_sync
            .sync_until_idle(&mut server)
            .await
            .expect("publish tail");

        let (_full_dir, full, full_sync) = client("snapshot").await;
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
        let connection = db::open(directory.path().join("notes.db"), "test", 8)
            .await
            .expect("open snapshot database");
        (directory, connection)
    }
}
