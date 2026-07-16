# Sync & Backend Architecture Plan

Status: approved design, ready for implementation.
Audience: implementing agent/developer. This document is self-contained; read it fully before starting. It encodes decisions already made with the project owner — do not re-litigate them, but do flag genuine contradictions you discover in code.

## 1. Context

notes-rs is a graph-native personal knowledge app: Tauri 2 + React frontend, Rust backend, SQLite with FTS5 (`nodes_fts`), sqlite-vec (`vec_nodes`), background workers for embeddings (`src-tauri/src/embed.rs`) and LLM entity extraction (`src-tauri/src/extract.rs`), and an OpenRouter-backed agent (`src-tauri/src/agent.rs`). Notes are an outline of blocks in a `nodes` table (stable `uuid`, `parent_id`, fractional `position REAL`), typed `edges`, attachments as files in app data. Current "sync" is manual snapshot push/pull to a folder.

The app is pre-release with no users and no data to migrate. Breaking changes to storage are acceptable; a schema-version bump with a fresh start is fine.

### Goals

1. **Near-realtime multi-device sync** (Google Keep feel: edit on desktop, visible on phone in well under a second) via a self-hostable server.
2. **Local-first**: the app stays fully functional offline and standalone (editor, outliner, FTS search, graph, attachments, undo). Sync and cloud AI are optional amplifiers.
3. **One AI engine, two hosts**: the same Rust crates run in-process inside the Tauri app (standalone mode) and inside the server binary (connected mode). No duplicated AI/pipeline logic.
4. **Mobile-ready**: a future Tauri mobile client is a thin host — local core + FTS offline, all AI via the server, no API keys on the device, no background polling workers.

### Non-goals (explicitly deferred — do not build now)

- E2E encryption (the op format must not preclude it: an op payload could later be an encrypted blob, so keep payload handling opaque-friendly, but implement plaintext).
- Folder/file transport for Syncthing-style sync (the per-device append-only oplog design keeps it possible later; declare the op format unstable until then).
- CRDT text merge inside a single block (LWW per field is the accepted resolution; see §5).
- Web client, multi-user collaboration, hosted multi-tenant control plane.
- BYOK AI on mobile.
- PostgreSQL anywhere. The owner has a Postgres server available; the decision is to NOT use it. The server materializes per-user SQLite files with the same `notes-core` code as the client (FTS5, sqlite-vec, triggers). A Postgres port would fork the storage layer and destroy the code-reuse premise.

## 2. Target architecture overview

```
┌─────────────── desktop app (Tauri) ───────────────┐
│ React UI                                           │
│ tauri commands (thin)                              │
│   notes-core   ── SQLite (state, FTS, vec, queues) │
│   notes-ai     ── workers in-process (standalone)  │
│   notes-sync   ── outbox, HLC, apply, WS client    │
└──────────────────────┬─────────────────────────────┘
                       │ WebSocket + HTTP (ops, blobs, search, chat)
┌──────────────────────▼─────────────────────────────┐
│ server (axum, single binary, self-hostable)        │
│   per-user oplog: seq assignment + WS fanout       │
│   notes-core   ── per-user SQLite replica          │
│   notes-ai     ── embed/extract workers, agent     │
│   blob store   ── content-addressed attachments    │
│   snapshots    ── bootstrap + log compaction       │
└────────────────────────────────────────────────────┘
```

Data classes (this taxonomy drives everything):

- **Source data** — nodes, user-created edges, attachment references, blobs. The only thing that syncs as ops.
- **Derived data** — `body_stemmed`, FTS index, wikilink/block-ref edges (parsed from content), extracted entities/edges, embeddings. Never synced as ops; recomputed deterministically (refs, FTS) or by pipelines (embeddings, extraction) on whichever side owns them. Embedding vectors may additionally be _distributed_ server→client as a cache (§8).
- **Local-only** — undo/redo history, queues, settings, API keys, device identity. Never leaves the device.

## 3. Workspace layout (Phase 0)

Convert the repo to a Cargo workspace:

```
crates/notes-core/    # domain + storage. From src-tauri: db.rs, sqlite.rs, stem.rs.
                      # NEW: apply-engine (§4). No tauri, no rig dependencies.
crates/notes-ai/      # from src-tauri: embed.rs, extract.rs, agent.rs.
                      # Depends on notes-core. Keeps the `local-models` feature.
                      # Replace tauri AppHandle/Emitter coupling in extract.rs with an
                      # event-callback trait so the server can host the worker too.
crates/notes-sync/    # op format + envelope, HLC, LWW merge rules, client sync
                      # machine (outbox, cursors, WS client), wire types shared
                      # with the server. Depends on notes-core.
src-tauri/            # host #1: wires everything in-process. commands.rs becomes thin.
server/               # host #2: axum binary (§7). Depends on notes-core, notes-ai, notes-sync.
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
- Derived maintenance stays inside apply: FTS triggers fire as today; applying `node_set_content` re-runs wikilink/block-ref parsing (the logic behind `update_block_with_refs`) so `refs` edges are recomputed, not synced; embed/extract queue triggers fire as today.
- Undo/redo (`history_undo`/`history_redo`) remains local snapshot-based, but undo/redo application must itself go through ops (generate inverse ops) so undos propagate to other devices as normal edits.

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

### New client-side tables (schema bump in notes-core)

```sql
sync_outbox   (rowid, op_id TEXT UNIQUE, envelope TEXT, created_at)   -- pruned on server ack
applied_ops   (op_id TEXT PRIMARY KEY, seq INTEGER)                   -- dedupe; prunable below compaction floor
tombstones    (uuid TEXT PRIMARY KEY, deleted_hlc TEXT)
sync_meta     (key TEXT PRIMARY KEY, value TEXT)  -- device_id, last_server_seq, server_url
-- per-field LWW clocks on nodes:
ALTER TABLE nodes ADD COLUMN content_hlc TEXT;    -- covers content+content_json
ALTER TABLE nodes ADD COLUMN title_hlc TEXT;
ALTER TABLE nodes ADD COLUMN structure_hlc TEXT;  -- covers parent+position
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
  users/{user_id}/notes.db      # materialized replica via notes-core (FTS, vec, queues all work)
  users/{user_id}/oplog.db      # envelope log with seq (separate file keeps compaction simple)
  users/{user_id}/snapshots/
  blobs/{aa}/{sha256}
```

- Seq assignment: per-user single-writer task (actor holding the user's connections + db handles); seq = last+1, monotonic, gapless.
- On ingest the server both appends to oplog and applies to its replica (same `apply`, Origin::Remote). The replica is what makes Phase 4 AI trivial.
- Snapshot job: every N ops (e.g. 10k) or on demand, write snapshot (reuse/extend the existing `DataArchive` export in notes-core), then ops below the snapshot floor become prunable once no device cursor is behind it.
- v1 is single-user-capable multi-user-shaped: `user_id` in the path structure from day one, even if the only auth is one token → one user.

### Phase 4 — AI on the server

- Run `notes-ai` workers (embed queue, extract queue) against the replica. The `EmbedderBackend`/`RerankBackend` factory (`make_embedder`/`make_reranker`) is configured by server env — full provider set including `local-models`.
- Endpoints: `POST /v1/search` (query string + limit → hybrid search + rerank on the replica → list of `{uuid, score}`), `POST /v1/chat` (SSE stream mirroring today's `ChatEvent` enum; the agent from `notes-ai` runs against the replica).
- Client behavior when a server is configured: local embed/extract workers are disabled; `search_hybrid`/`search_agentic`/chat commands proxy to the server; `embed_meta` (provider id, ndims) is dictated by the server. When the server is unreachable: search degrades to local FTS + backlink boost (graceful, no error), chat reports offline.
- Standalone (no server configured): exactly today's behavior — in-process workers, BYOK providers, `AI_LOCAL_ONLY` supported.
- Rule to enforce in code: **AI configuration always comes from whoever owns the vector index** (server if configured, else local settings). Never let two devices write the same index with different providers.

## 8. Embedding distribution policy (Phase 5)

- Phone: no vectors on device. Online → server search; offline → FTS.
- Desktop (optional per-device setting): pull vector cache from server (`GET /v1/embeddings/export?since=...`, keyed by `(content_hash, provider_id, model, ndims)`) into local `vec_nodes` for offline semantic search.
- Standalone devices compute their own embeddings as today.

## 9. Phased delivery plan

| Phase | Deliverable                                                                                                                       | Acceptance criteria                                                                                                                                                |
| ----- | --------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 0     | Workspace split into `notes-core` / `notes-ai` / `notes-sync` / hosts                                                             | App unchanged; all tests pass; `notes-core` has no tauri/rig deps                                                                                                  |
| 1     | Apply-engine: all mutations are ops; schema bump (new tables, HLC columns, uuid-based refs in ops); undo emits inverse ops        | All existing frontend flows work; op round-trip unit tests; idempotency tests                                                                                      |
| 2     | Sync machine in `notes-sync` with in-memory/loopback transport; HLC; LWW; tombstones; convergence property tests                  | Property tests green (§10); no network code yet required to be complete                                                                                            |
| 3     | Server v1 (oplog, WS fanout, snapshot, blobs) + client integration + sync UI (status, device settings)                            | Two desktop instances converge realtime (<500 ms online); offline edits on both sides converge on reconnect; new-device bootstrap from snapshot equals full replay |
| 4     | Server AI: workers on replica, `/search`, `/chat` SSE; client proxy mode + FTS fallback                                           | With server configured, phone-profile client does semantic search with zero local vectors and no API keys; standalone mode still fully works                       |
| 5     | Policies & polish: vector-cache pull for desktop, compaction automation, outbox/applied_ops pruning, docs, docker-compose example | Self-host quickstart works end-to-end from README                                                                                                                  |

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
- Existing FTS/graph/queue behavior covered by current tests must keep passing after the apply-engine refactor.

## 11. Known code touchpoints

- `src-tauri/src/db.rs` — schema (v-bump), all mutation functions → op constructors + apply; `DataArchive` reused for snapshots.
- `src-tauri/src/commands.rs` — becomes thin: validate → build op → apply → emit events. Events (`pages:changed` etc.) must also fire on remote-op apply so the UI live-updates during sync (this is the visible realtime feature).
- `src-tauri/src/extract.rs` — replace direct `AppHandle`/`Emitter` use with an injected event sink trait (host provides Tauri emitter or server no-op/WS notify).
- `src-tauri/src/embed.rs` — unchanged logic; moves to `notes-ai`; add a `remote` provider variant later (Phase 4 client) following the existing `VoyageEmbedder` reqwest pattern.
- `src-tauri/src/settings.rs` — add server URL/token; keep secrets local-only (never in ops/snapshots).
- Frontend: sync status indicator, server settings screen, offline search degradation notice. Existing event-driven refresh (`pages:changed`) should make remote updates appear without new UI architecture.

## 12. Open questions (resolve with the owner before the relevant phase, not before starting)

1. Auth hardening timeline (per-device tokens vs pairing flow) — fine to defer past Phase 3.
2. Embedding model/provider default for the server era (Gemini Embedding 2 multimodal vs Voyage 4; see discussion history) — irrelevant until Phase 4; the abstraction covers both.
3. Whether `content_json` merging ever needs to be finer than LWW-with-content — revisit only if real usage shows lost edits.
