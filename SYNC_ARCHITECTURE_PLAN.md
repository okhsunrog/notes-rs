# Sync & Backend Architecture Plan

Status: approved design, ready for implementation.
Audience: implementing agent/developer. This document is self-contained; read it fully before starting. It encodes decisions already made with the project owner — do not re-litigate them, but do flag genuine contradictions you discover in code.

## 1. Context

notes-rs is a graph-native personal knowledge app: Tauri 2 + React clients, a Rust/Axum server, local SQLite with FTS5, and a server-owned AI subsystem with sqlite-vec, background embedding/entity-extraction workers, and an agent. Notes are an outline of blocks in a `nodes` table (stable `uuid`, local `parent_id`, fractional `position REAL`), typed `edges`, and content-addressed attachments.

The app is pre-release with no users and no data to migrate. Breaking changes to storage are acceptable; a schema-version bump with a fresh start is fine.

### Goals

1. **Near-realtime multi-device sync** (Google Keep feel: edit on desktop, visible on phone in well under a second) via a self-hostable server.
2. **Local-first notes**: editor, outliner, FTS search, graph, attachments, history, and an outbox remain functional offline. Sync and AI resume when the server is reachable.
3. **One AI owner**: AI runs only in the server. Clients never contain provider SDKs, API keys, vector tables, embedding/extraction workers, or a second retrieval pipeline.
4. **One client architecture**: desktop and Android use the same local core + FTS + sync/API client. Platform differences stay at the Tauri integration boundary.
5. **Remotely managed server**: authenticated client settings expose server health, AI configuration, indexing progress, and operational controls without exposing stored secrets.

### Non-goals (explicitly deferred — do not build now)

- E2E encryption (the op format must not preclude it: an op payload could later be an encrypted blob, so keep payload handling opaque-friendly, but implement plaintext).
- Folder/file transport for Syncthing-style sync (the per-device append-only oplog design keeps it possible later; declare the op format unstable until then).
- CRDT text merge inside a single block (LWW per field is the accepted resolution; see §5).
- Web client, multi-user collaboration, hosted multi-tenant control plane.
- Client-side/BYOK AI on any platform.
- Distribution of embedding vectors to clients.
- PostgreSQL for the current single-user deployment. Durable notes and oplog state remain SQLite. The vector store is a replaceable server-only derived-data adapter; sqlite-vec is the initial implementation and Qdrant is a future option only after measured need.

## 2. Target architecture overview

```
┌────────────── desktop / Android (Tauri) ──────────┐
│ React UI                                          │
│ thin typed Tauri commands                         │
│   notes-core  ── SQLite (source state + FTS)      │
│   notes-sync  ── outbox, HLC, apply, WS client   │
│   no vectors, AI workers, provider keys, or rig   │
└──────────────────────┬────────────────────────────┘
                       │ typed HTTP + WebSocket protocol
┌──────────────────────▼─────────────────────────────┐
│ server (axum, single binary, self-hostable)        │
│   per-user oplog: seq assignment + WS fanout       │
│   notes-core  ── per-user SQLite replica + FTS     │
│   notes-ai    ── ai.db + VectorStore + workers     │
│   blob store   ── content-addressed attachments    │
│   snapshots    ── bootstrap + log compaction       │
│   admin API    ── settings, health, index progress │
└────────────────────────────────────────────────────┘
```

Data classes (this taxonomy drives everything):

- **Source data** — nodes, user-created edges, attachment references, blobs. The only thing that syncs as ops.
- **Deterministic client/server derived data** — `body_stemmed`, FTS index, and wikilink/block-ref edges. Recomputed from source data on every replica.
- **Server-derived domain data** — extracted entities/edges. The server writes them as ordinary server-authored ops so clients receive the visible graph result, not the extraction queue or prompts.
- **Server-only disposable AI data** — embeddings, vector generations, indexing queues, content hashes, and worker statistics. Never synced and safe to rebuild from the server replica.
- **Device-local data** — undo/redo history, UI settings, sync credentials, and device identity. Never leaves the device.

## 3. Workspace layout (Phase 0)

Convert the repo to a Cargo workspace:

```
crates/notes-core/    # domain + storage. From src-tauri: db.rs, sqlite.rs, stem.rs.
                      # NEW: apply-engine (§4). No tauri, no rig dependencies.
crates/notes-ai/      # server-only AI runtime and ai.db persistence.
                      # Owns sqlite-vec, VectorStore, workers, retrieval, and agent.
crates/notes-sync/    # op format + envelope, HLC, LWW merge rules, client sync
                      # machine (outbox, cursors, WS client). Depends on notes-core.
crates/notes-protocol/# transport DTOs for sync, search, chat, status, and admin APIs.
src-tauri/            # desktop/mobile client host; never depends on notes-ai/llm-relay/rig.
server/               # the only AI host; depends on all domain/server crates.
```

Acceptance for Phase 0: app builds and behaves identically; `cargo test` and `vp check`/`vp test` pass; no functional change.

## 4. Apply-engine (Phase 1) — the load-bearing refactor

Every state mutation goes through a single function in `notes-core`:

```rust
pub async fn apply(conn: &Connection, op: &Op, origin: Origin) -> Result<ApplyOutcome>
// Origin: Local (user action on this device) | Remote (came from sync)
```

Rules:

- Local user actions (today's `create_block`, `update_block_with_refs`, `split_block`, `move_block`, `reorder_block`, `delete_block`, `link_nodes`, page CRUD, agent write-tools, import) construct ops, call `apply`, and — when sync is configured — enqueue the op into `sync_outbox`. Local apply is synchronous and immediate: UI latency must not change.
- `apply` is **idempotent**: an `op_id` already recorded in `applied_ops` is a no-op success.
- `apply` is **deterministic**: given the same starting state and the same set of ops (in any delivery order of concurrent ops), the resulting SQLite state is byte-identical in the source tables. This is the core testable property (§10).
- Deterministic derived maintenance stays inside apply: FTS triggers fire as today; applying `node_set_content` re-runs wikilink/block-ref parsing so refs edges are recomputed, not synced. Client core contains no AI queue triggers.
- Undo/redo is action-based, never snapshot-based. A local action stores forward/inverse operation templates; undo and redo materialize fresh ops with new op IDs and HLCs so the result propagates normally. Preconditions prevent an old undo from silently overwriting a newer concurrent value.

### Op envelope and kinds

```json
{
  "op_id": "uuid-v4",
  "device_id": "uuid-v4",
  "hlc": "0189f3a2b4c8-0003-d1e2f3a4",   // sortable: wall_ms hex - counter hex - device suffix
  "format_version": 1,
  "kind": "node_set_content",
  "payload": { ... }
}
```

`seq` (u64) is assigned by the server on ingest and is NOT part of the client-created envelope.

All node references in payloads use `uuid`, never the local integer `id`. Kinds and payloads:

| kind                | payload                                                                                  |
| ------------------- | ---------------------------------------------------------------------------------------- |
| `node_create`       | `{uuid, node_kind, title?, content, content_json?, parent_uuid?, position?, created_at}` |
| `node_set_content`  | `{uuid, content, content_json?}`                                                         |
| `node_set_title`    | `{uuid, title?}`                                                                         |
| `node_move`         | `{uuid, parent_uuid?, position}`                                                         |
| `node_delete`       | `{uuid}` (subtree delete = one op per node, batched)                                     |
| `edge_add`          | `{src_uuid, dst_uuid, edge_kind, weight}` — manual/agent edges only                      |
| `edge_remove`       | `{src_uuid, dst_uuid, edge_kind}`                                                        |
| `attachment_add`    | `{node_uuid, blob_hash, filename, mime, size}`                                           |
| `attachment_remove` | `{node_uuid, blob_hash}`                                                                 |

Not ops: anything derived (refs edges, extracted edges/entities, embeddings, FTS), settings, history.

### Client-side tables (notes-core)

```sql
sync_outbox   (rowid, op_id TEXT UNIQUE, envelope TEXT, created_at)   -- pruned on server ack
applied_ops   (op_id TEXT PRIMARY KEY, seq INTEGER)                   -- dedupe; prunable below compaction floor
tombstones    (uuid TEXT PRIMARY KEY, deleted_hlc TEXT)
sync_meta     (key TEXT PRIMARY KEY, value TEXT)  -- device_id, last_server_seq, server_url
-- per-field LWW clocks on nodes:
ALTER TABLE nodes ADD COLUMN content_hlc TEXT;    -- covers content+content_json
ALTER TABLE nodes ADD COLUMN title_hlc TEXT;
ALTER TABLE nodes ADD COLUMN structure_hlc TEXT;  -- covers parent+position
history_actions (id, action_uuid, label, forward_json, inverse_json, created_at)
```

## 5. Conflict resolution (fixed decisions)

- **HLC** (hybrid logical clock): `(wall_ms, counter, device_id)`, encoded as a lexicographically sortable string. Standard HLC update rules on send/receive; guards against clock skew.
- **Per-field LWW**: an incoming op wins iff its `hlc` > the stored field HLC. Fields are independent: a title edit on device A and a content edit on device B to the same node both survive.
- **Moves**: LWW on `structure_hlc`. Concurrent inserts under one parent need no resolution — fractional `position REAL` interleaves naturally. On the rare exact position tie, order by `uuid` for determinism.
- **Delete vs edit**: tombstone wins over any later edit to that uuid (edits to tombstoned nodes are dropped). Deleting a node whose children have concurrent new ops: children are deleted too (subtree delete emits per-node ops); a concurrently _created_ child under a deleted parent is re-parented to the page root rather than lost. Document this in code; test it (§10).
- **Cycles**: a concurrent pair of moves can create a parent cycle. After applying a remote `node_move`, run a cycle check; if a cycle exists, break it by re-parenting the node with the lower HLC move to the page root. Deterministic on all devices.

## 6. Sync protocol

Transport: HTTP + WebSocket, JSON bodies (serde). Auth v1: static bearer token per device, configured on the server (env/config file) and entered in app settings. Token pairing UX can improve later.

```
GET  /v1/health
GET  /v1/snapshot                      → latest snapshot archive + its seq
GET  /v1/ops?since={seq}&limit={n}     → ordered ops after seq (catch-up)
POST /v1/ops                           → batch of envelopes; returns assigned seqs
WS   /v1/sync                          → bidirectional: client sends envelopes,
                                          server pushes {seq, envelope} to all of the
                                          user's live connections (including echo;
                                          clients dedupe by op_id)
PUT  /v1/blobs/{sha256}                → content-addressed upload (idempotent)
GET  /v1/blobs/{sha256}                → download; HEAD to probe
```

Client loop (in `notes-sync`): on connect → `GET /ops?since=last_server_seq` catch-up → apply each (Remote) → stream via WS; outbox drains through WS (or POST fallback); on ack, prune outbox and advance `last_server_seq`. Offline: outbox accumulates; UI shows sync state. New device bootstrap: `GET /snapshot`, import via notes-core, then catch up from snapshot seq.

Blobs: `attachment_add` op carries only the hash; the blob uploads lazily in the background. Receivers download on first access (or eagerly on desktop policy). Attachment files on disk move to content-addressed names.

Realtime target: op visible on a second online device < 500 ms after local apply on typical home internet.

## 7. Server (Phase 3)

Stack: **axum + tokio + rusqlite** (same pinned versions as the app where possible), single binary, config via env/file. TLS is the reverse-proxy's job. Storage layout:

```
data_dir/
  users/{user_id}/notes.db      # authoritative materialized source replica + FTS
  users/{user_id}/oplog.db      # envelope log with seq (separate file keeps compaction simple)
  users/{user_id}/ai.db         # disposable queues, metadata, generations, sqlite-vec
  users/{user_id}/snapshots/
  blobs/{aa}/{sha256}
```

- Seq assignment: per-user single-writer task (actor holding the user's connections + db handles); seq = last+1, monotonic, gapless.
- On ingest the server both appends to oplog and applies to its replica (same `apply`, Origin::Remote). The replica is what makes Phase 4 AI trivial.
- Snapshot job: every N ops (e.g. 10k) or on demand, write snapshot (reuse/extend the existing `DataArchive` export in notes-core), then ops below the snapshot floor become prunable once no device cursor is behind it.
- v1 is single-user-capable multi-user-shaped: `user_id` in the path structure from day one, even if the only auth is one token → one user.

### Phase 4 — server-only AI and remote administration

- Run all `notes-ai` workers against each user's server replica. The server owns provider credentials, embedding identity, vector generations, extraction, retrieval, and chat.
- `POST /v1/search` returns hits plus requested/effective mode, execution owner, degradation reason, and indexing watermark. Clients never label an FTS fallback as semantic search.
- `POST /v1/chat` streams typed `ChatEvent` values from `notes-protocol`.
- Authenticated admin endpoints expose server AI settings, secret presence, provider probes, indexing policy, progress, failures, pause/resume/rebuild, and runtime statistics. Secrets are accepted write-only.
- When the server is absent or unreachable, editing and local FTS continue. Semantic search, extraction, and chat are explicitly unavailable; there is no hidden client AI fallback.
- `notes-ai` owns a replaceable `VectorStore`. The initial `SqliteVectorStore` lives in `ai.db`; a future Qdrant adapter is justified only by measured corpus size/latency.

## 8. AI index lifecycle

- Queue rows contain node UUID, exact composed-input hash, embedding identity fingerprint, and source server seq. A completed provider request is committed only if those values still match transactionally.
- Embedding identity includes provider endpoint identity, model, dimensions, distance/normalization policy, and input-format version.
- Model/config changes build a new generation while queries continue using the active generation. The server atomically activates the new generation after completion and deletes the previous one later.
- Client Settings show server-owned indexing progress and whether the semantic index is current with the synchronized replica.

## 9. Phased delivery plan

| Phase | Deliverable                                                                                                                | Acceptance criteria                                                                                                                                                |
| ----- | -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 0     | Workspace split into `notes-core` / `notes-ai` / `notes-sync` / hosts                                                      | App unchanged; all tests pass; `notes-core` has no tauri/rig deps                                                                                                  |
| 1     | Apply-engine: all mutations are ops; schema bump (new tables, HLC columns, uuid-based refs in ops); undo emits inverse ops | All existing frontend flows work; op round-trip unit tests; idempotency tests                                                                                      |
| 2     | Sync machine in `notes-sync` with in-memory/loopback transport; HLC; LWW; tombstones; convergence property tests           | Property tests green (§10); no network code yet required to be complete                                                                                            |
| 3     | Server v1 (oplog, WS fanout, snapshot, blobs) + client integration + sync UI (status, device settings)                     | Two desktop instances converge realtime (<500 ms online); offline edits on both sides converge on reconnect; new-device bootstrap from snapshot equals full replay |
| 4     | Server-only AI store/workers, `/search`, `/chat`; remove all client AI/vector code                                         | Both clients contain zero local vectors/provider keys; offline FTS remains honest and functional                                                                   |
| 5     | Remote admin settings/status UI, index generations, compaction, monitoring, and deployment polish                          | Provider configuration and indexing lifecycle are safely manageable from the app; self-host quickstart works end-to-end                                            |

Keep phases mergeable: each phase lands green on `main` behind the absence-of-config (no server configured → nothing changes for a standalone user).

## 10. Testing strategy (non-negotiable before Phase 3)

- **Convergence property test** (proptest): generate random op sequences from N simulated devices with random interleavings/duplications/reorderings of _concurrent_ ops (causal order per device preserved); assert all replicas reach identical source-table state, including the server replica.
- Idempotent redelivery; echo delivery (own op back from server).
- Tombstone cases: delete vs concurrent edit; delete parent vs concurrent create-child; resurrection must not happen.
- Concurrent sibling inserts at equal positions → deterministic order (uuid tiebreak).
- Concurrent moves creating a cycle → deterministic break (§5).
- HLC skew: device with clock hours ahead/behind still converges; HLC monotonicity maintained.
- Snapshot bootstrap ≡ full log replay (state equality).
- Server restart mid-stream: no seq gaps or duplicates observed by clients.
- Existing FTS/graph behavior must keep passing. AI queue/index tests live in `notes-ai` and run only against the server-side AI store.

## 11. Known code touchpoints

- `notes-core` — no sqlite-vec, queues, extraction state, provider metadata, or JavaScript-derived integer identity. UUID is the replicated identity; integer row IDs are local implementation details.
- `notes-ai` — server-only `ai.db`, VectorStore, typed errors, supervised workers, one retrieval pipeline.
- `notes-sync` — one transport-independent client used by loopback tests and production HTTP/WS transport; batch apply/cursor updates are atomic.
- `notes-protocol` — typed transport DTOs and capability/error enums, with no implementation dependencies.
- `src-tauri` — thin local domain/sync/API adapter; settings contain server connection and UI/device preferences only.
- Frontend — explicit sync/AI/index capabilities, narrow domain-event invalidation, remote server administration, and honest offline degradation.

## 12. Open questions (resolve with the owner before the relevant phase, not before starting)

1. Auth hardening timeline (per-device tokens vs pairing flow). The protocol uses token scopes (`sync`, `user`, `admin`) now even if the first deployment grants all scopes to one token.
2. When measured vector count/latency justifies a Qdrant adapter. Do not add the service speculatively.
3. Whether `content_json` merging ever needs to be finer than LWW-with-content — revisit only if real usage shows lost edits.
