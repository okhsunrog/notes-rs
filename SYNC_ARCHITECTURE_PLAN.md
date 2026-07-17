# Sync, Storage, and AI Architecture

Status: implemented architecture and remaining pre-release work. Updated 2026-07-17.

This document describes the current code, not a compatibility target for old builds. The project
has no users or valuable production databases yet, so storage and wire formats may still change
deliberately before the first release.

The accepted editor and page-presentation target is recorded separately in
[`EDITOR_ARCHITECTURE.md`](EDITOR_ARCHITECTURE.md). The durable `PageLayout` versus pane-local
Reading boundary is implemented; CodeMirror, continuous Document editing, and linked preview
sessions remain pending.

General multi-pane composition, adjacent navigation, linked preview, responsive projection, and
the collapsible AI companion are defined in
[`WORKSPACE_ARCHITECTURE.md`](WORKSPACE_ARCHITECTURE.md).

The accepted daily Journal domain, UI surfaces, sync identity, and loss-aware Logseq import boundary
are defined in [`JOURNAL_ARCHITECTURE.md`](JOURNAL_ARCHITECTURE.md).

## 1. Product boundary

notes-rs is a local-first personal knowledge application for desktop and Android. Both clients use
the same React UI, Tauri adapter, Rust domain core, local SQLite database, and sync client.

The main invariants are:

1. Pages and blocks are distinct domain types. There is no generic `Node`, `NodeKind`, or `nodes`
   table.
2. UUID is the durable identity everywhere outside SQLite implementation details. User-created
   domain and operation IDs use UUIDv7. The accepted Journal design deliberately derives a journal
   page UUID deterministically from workspace UUID plus civil date so concurrent offline creation
   converges. UUIDs are stored as 16-byte SQLite values and cross Rust/TypeScript boundaries through
   native Specta UUID support.
3. Editing, attachments, graph navigation, history, and lexical search work from the local
   database without a server.
4. The server owns all AI work and provider credentials. Clients never load sqlite-vec, embedding
   models, entity tables, provider SDKs, or a second retrieval pipeline.
5. Embeddings and extracted entities are disposable derived server data. They are never synced to
   clients and are not part of the client graph.
6. Every source-state mutation is represented by a versioned operation and passes through one
   deterministic apply boundary.

## 2. Runtime layout

```text
desktop / Android
  React
    TanStack Query backend cache
    local editor drafts, caret, dialogs, and navigation state
  generated tauri-specta commands and DomainEvent
  Tauri host
    notes-core -> local notes.db (pages, blocks, refs, FTS, history, outbox)
    notes-sync -> transport-independent sync machine + HTTP/SSE remote API
    src-tauri sync -> foreground WebSocket session + blob orchestration
    no notes-ai dependency
              |
              | authenticated HTTP + WebSocket
              v
Axum server
  per-user notes-core replica -> notes.db
  append-only sequenced oplog  -> oplog.db
  content-addressed blobs
  notes-ai                     -> disposable ai.db + sqlite-vec
  provider/index administration API
```

Workspace ownership follows that boundary:

- `crates/notes-core`: typed content model, SQLite persistence, operations, HLC/LWW apply, local
  FTS, graph derivation, archives, and action history.
- `crates/notes-protocol`: transport-only sync, chat, search, status, and AI administration DTOs.
- `crates/notes-sync`: transport-independent `SyncClient` plus production HTTP/SSE transport.
- `crates/notes-ai`: server-only retrieval, reranking, extraction, agent, workers, and `ai.db`.
- `src-tauri`: thin desktop/mobile adapter over core, sync, settings, and remote server APIs.
- `server`: Axum host, authentication, per-user state, oplog, snapshots, blobs, and AI runtime.

## 3. Typed content model

### Pages

A `Page` owns a title and an ordered tree of blocks. Its persisted `PageLayout` is one of:

- `Outline`: block hierarchy is shown explicitly and bullets are editor chrome;
- `Document`: the same hierarchy is presented as a document.

Changing layout never converts or duplicates content. `PagePresentation = Editing | Reading` is
pane/component-local state and never enters SQLite, operations, archives, snapshots, RPC, or sync.
The current Document renderer still uses the block tree; its continuous CodeMirror adapter remains
an editor migration task.

Side-by-side Split remains a future workspace layout operation and likewise never enters content
operations or sync. See [`EDITOR_ARCHITECTURE.md`](EDITOR_ARCHITECTURE.md) and
[`WORKSPACE_ARCHITECTURE.md`](WORKSPACE_ARCHITECTURE.md).

The accepted Journal target adds a closed `PageKind = Note | Journal { date }` independently of
layout. Journal reuses the ordinary page/block model and defaults to Outline; calendar/timeline is a
workspace surface, not another page kind or editor. Its implementation is pending. See
[`JOURNAL_ARCHITECTURE.md`](JOURNAL_ARCHITECTURE.md).

### Blocks

A `Block` belongs to exactly one page and optionally has a parent block on that page. The typed
page/block tree is durable source state: `BlockStyle` provides block-level document semantics and
the `markdown` field contains the block's textual body independently of outline nesting:

```text
paragraph, bullet, numbered, task,
heading_1, heading_2, heading_3,
quote, code, divider
```

This allows short outline notes and long articles or project documentation to use the same tree.
Outline bullets do not force every block to have Markdown list semantics. Plain Markdown
import/export is a normalized portable representation; archives and sync snapshots are the lossless
representation of UUIDs, parentage, styles, and operation state.

Sibling order uses `OrderKey`, a validated fixed-width 16-character uppercase hexadecimal key.
SQLite text ordering therefore matches numeric ordering. Local structural actions renumber the
affected sibling list with large fixed steps; operations carry the resulting keys, so replicas do
not depend on floating-point positions or local integer IDs.

### References and attachments

References are separate derived tables:

- `page_links`: a source block plus normalized target title and, when resolved, target page UUID;
- `block_refs`: a source block and target block UUID.

They are rebuilt deterministically from `[[Page]]` and `((block-uuid))` Markdown whenever a block
is applied. They are not independent sync operations. Unresolved page links and dangling block
references remain representable and resolve when their target appears.

An `Attachment` has a typed `Page(UUID)` or `Block(UUID)` owner. Metadata is source state and is
synced through attachment operations. Bytes are addressed by SHA-256 and transferred separately
through the blob API. A local file is deleted only after its hash/path is no longer referenced.

### SQLite schema

The current clean baseline is `crates/notes-core/src/db/migrations/V001__initial.sql`:

```text
pages, blocks
page_links, block_refs, attachments
pages_fts, blocks_fts
history_undo, history_redo
sync_outbox, applied_ops, tombstones
block_structure_lww, attachment_lww
sync_meta, local_device
```

The `id INTEGER PRIMARY KEY` columns on `pages` and `blocks` are local FTS row IDs only. Domain
queries, RPCs, operations, refs, graph edges, and sync never expose them.

There is intentionally no migration from the former generic-node schema. While the project is
unreleased, a completed breaking architecture stage may replace accumulated migrations with one
fresh V001 baseline. Development databases must then be deleted and recreated; compatibility
tables, views, conversion code, and reset migration counters are not retained.

## 4. Operation and apply model

The current operation format is version 5. An envelope contains:

```text
op_id: UUIDv7
workspace_uuid: UUID
device_id: UUID
hlc: hybrid logical clock
format_version: 5
kind + typed payload
```

The closed operation set is:

| Domain     | Operations                                                                            |
| ---------- | ------------------------------------------------------------------------------------- |
| Page       | `page_create`, `page_set_title`, `page_set_layout`, `page_delete`                     |
| Block      | `block_create`, `block_set_markdown`, `block_set_style`, `block_move`, `block_delete` |
| Attachment | `attachment_add`, `attachment_remove`                                                 |

`PageCreate` carries `PageLayout`. `BlockCreate` carries page/parent UUIDs, `OrderKey`,
`BlockStyle`, Markdown, and creation time. A block move carries its page, optional parent, and
order key. Attachment operations carry a typed owner rather than a generic content ID.

All local and remote operations use the same apply engine. It provides:

- idempotency through `applied_ops` and operation UUID;
- one global object kind per UUID, enforced both at the apply boundary and by SQLite triggers;
- immutable page kind/date identities retained after deletion;
- per-field HLC/LWW clocks for page title/layout and block Markdown/style/structure;
- page/block tombstones and attachment presence intents;
- incarnation boundaries: a newer `PageCreate` cannot accidentally revive blocks or parent intents
  from an older deleted incarnation, regardless of delivery order;
- deterministic structure reconciliation and cycle breaking;
- canonical reference parsing on every replica;
- atomic batched application of remote operations and cursor advancement;
- an offline outbox for local operations awaiting server acknowledgement.

Undo and redo store forward and inverse operation templates, not database snapshots. Replaying a
history action creates fresh operation IDs and HLC values, so undo/redo is sync-visible and does not
force a full AI reindex. History is device-local and is cleared by whole-workspace archive import.

## 5. Snapshots and sync protocol

`SyncSnapshot` is also typed and UUID-first. It contains the workspace UUID, immutable page
identities, pages, blocks, block-structure intents, typed page/block tombstones, attachment intents,
format version, and server sequence. There is no generic-node compatibility payload. Import
validates UUID uniqueness and kind separation, deterministic Journal identity, page and parent
ownership, structure-intent coherence, attachment identity/ownership, and live-versus-tombstone
exclusivity before replacing any local state.

The server exposes:

```text
GET  /v1/health
GET  /v1/info
GET  /v1/ops?since={seq}&limit={n}
POST /v1/ops
GET  /v1/snapshot
POST /v1/bootstrap
WS   /v1/sync
PUT  /v1/blobs/{sha256}
GET  /v1/blobs/{sha256}
HEAD /v1/blobs/{sha256}
```

`SyncClient` is transport-independent and is used by both loopback tests and the production
client. A sync pass catches up from the current sequence, uploads referenced blobs, pushes an
outbox batch, acknowledges assigned sequences, then catches up again. Remote batches and their
cursor update commit in one SQLite transaction.

The WebSocket path provides near-realtime fanout while the app is running. HTTP catch-up and the
outbox provide recovery after disconnects. Mobile background sync is intentionally not planned;
Android synchronizes while the application is active and catches up on the next launch.

On a new replica:

- non-empty local + empty server bootstraps the server;
- empty local + non-empty server imports the server snapshot and blobs;
- two different non-empty workspaces are rejected rather than merged implicitly.

The server assigns a gapless per-user `seq` in its oplog and materializes the same operations into
its own notes-core replica. Server replay resumes from the materialized replica cursor rather than
reapplying the log from zero.

## 6. Local-first client behavior

Desktop and Android always keep local source state and FTS. Without a configured or reachable
server, these capabilities continue to work:

- create, edit, move, style, and delete pages/blocks;
- Outline, Document, and Reading views;
- normalized page-title search and local FTS5 block search;
- deterministic wikilinks, block refs, backlinks, and the page/block graph;
- attachments already present on the device;
- action undo/redo, archives, import/export, and recovery backups.

Sync status is explicit (`disabled`, `connecting`, `syncing`, `online`, `offline`, or `error`).
Semantic search and chat return an explicit unavailable error when no server is configured; they
do not silently fall back to another AI implementation.

Persisted Rust state is cached by TanStack Query. A generated, typed `DomainEvent` invalidates page,
block, child-tree, graph, backlink, attachment, history, sync, settings, and server-AI query keys.
Editor drafts and caret state remain local to editor components so backend refreshes do not turn
every keystroke into global UI state.

## 7. Server-owned AI

Only the Axum server depends on `notes-ai`. For each configured user it opens a separate disposable
`ai.db` containing:

- embedding generations and their identity metadata;
- embedding and extraction jobs with source hashes and server sequence watermarks;
- generation-to-vector mappings and dynamically created sqlite-vec tables;
- extracted entities and extraction edges;
- runtime switches for automatic embeddings, entity extraction, and query rewriting.

The embedding identity includes endpoint, model, dimensions, and input format. A provider/model or
dimension change creates a new generation instead of mixing incompatible vectors. Worker writes
verify the exact queued source hash transactionally, so a stale provider response cannot remove a
newer job or become the current vector.

Extracted entities are AI-derived server data. They may inform server-side AI behavior, but they
are not page/block operations, are not present in `notes.db`, are not sent to clients, and do not
currently appear in the client graph.

The authenticated AI API is:

```text
GET /v1/ai/status
PUT /v1/ai/status
PUT /v1/ai/provider
POST /v1/ai/provider/probe
POST /v1/ai/reindex
POST /v1/search
POST /v1/chat
```

Settings can inspect generation state, indexed/source document counts, pending and failed jobs,
toggle automatic indexing/extraction/query rewriting, update provider URLs/models, submit
write-only secrets, probe provider capabilities, and request a rebuild. Provider secrets remain
server-side and are never returned to the webview.

The current deployment uses OpenRouter embeddings and reranking. Completion supports OpenAI Chat
Completions-compatible and Anthropic Messages-compatible servers with custom base URLs through the
shared protocol/relay layer.

## 8. Configuration and deployment

Client product configuration lives in the application settings path. It contains the sync server
URL, sync token, and native/borderless window preference; appearance preferences are device-local
UI state. There is no `.env` product-configuration path. `TAURI_DEV_HOST` remains the one build-time
input required by Tauri/Vite for device development.

The server starts from a TOML bootstrap containing listen/storage settings, the static token-to-user
mapping, and initial AI provider configuration. Provider/runtime AI settings are subsequently
manageable through the authenticated app UI. TLS and public routing are owned by the existing
reverse proxy. Deployment produces a static musl binary rather than a container image.

## 9. Implemented status

| Area                                                                   | State                                       |
| ---------------------------------------------------------------------- | ------------------------------------------- |
| Typed `pages` / `blocks` baseline with no generic nodes                | Complete                                    |
| Typed block styles and provisional three-way page view                 | Complete as a prototype                     |
| Accepted continuous editor and pane-local view architecture            | Design complete; implementation pending     |
| Typed Journal/workspace identity and sync/archive invariants           | Complete                                    |
| Journal product surfaces and Logseq conversion boundary                | In progress                                 |
| UUIDv7 operations, HLC/LWW apply, tombstones, deterministic structure  | Complete                                    |
| Action-based inverse-operation undo/redo                               | Complete                                    |
| Local FTS, refs, backlinks, graph, attachments, archives               | Complete                                    |
| Transport-independent batched sync client                              | Complete                                    |
| Axum oplog, HTTP/WS fanout, bootstrap snapshots, blob transfer         | Complete for pre-release use                |
| Desktop and Android shared thin-client architecture                    | Complete; real-device UI validation remains |
| Server-only vector generations, extraction, retrieval, reranking, chat | Complete                                    |
| Remote AI settings, probes, progress, toggles, and reindex             | Complete                                    |
| tauri-specta UUID/enums/commands/domain events and binding drift CI    | Complete                                    |
| Static musl packaging and `cloud-forge` deployment path                | Complete                                    |

## 10. Validation

The workspace tests cover migration from an empty database, typed-storage invariants, UUID-only
contracts, FTS and reference derivation, inverse-operation history, archive round trips, sync
idempotency, batched cursor advancement, snapshots, attachment validation, server restart/replay,
network sync, and randomized replica convergence.

Required validation for architecture changes is:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked --all-features
vp check
vp test
vp build
```

CI also exports `src/lib/bindings.ts` from Rust and fails on binding drift.

## 11. Remaining work

Architecture and correctness:

1. Add a durable device registry, cursor retirement, and a proven compaction floor before deleting
   old oplog or `applied_ops` history.
2. Replace static bootstrap tokens with paired, scoped, revocable per-device credentials.
3. Keep the previous provider runtime and active vector generation serving queries until a
   replacement generation is fully built and activated.
4. Revisit JavaScript-facing 64-bit counters before any value can approach the safe-integer limit.

Product and corpus support:

1. Implement the accepted editor architecture: durable `PageLayout`, pane-local Document views,
   shared Markdown AST rendering, CodeMirror 6 Live Preview, and a continuous Document adapter.
2. Replace the single-content shell with the accepted pane tree, adjacent navigation, linked
   preview, responsive projection, and collapsible Assistant dock.
3. Add server-side retrieval chunking for large blocks/documents. The current index unit is one
   page or block UUID, which is sufficient for the demo but not ideal for long articles.
4. Implement the accepted Journal domain, Today/calendar surfaces, deterministic offline identity,
   and journal-aware search/graph filters.
5. Add Markdown-vault, Obsidian, and the staged Logseq importer, including page properties, block
   UUIDs, nesting, and assets; no legacy notes-rs database importer is planned.
6. Add journal templates, general properties, saved queries, and an extension model.
7. Add drag-and-drop movement, cross-block selection, transclusion, richer Markdown authoring,
   graph filters/layouts, and measured larger-corpus performance work.
8. Add signed production packages and end-to-end UI/accessibility coverage on desktop and real
   Android hardware.

## 12. Explicit non-goals

- client-side or BYOK AI;
- syncing embedding vectors, AI queues, or extracted entities to devices;
- running sqlite-vec or an embedding model on Android/desktop;
- Android background synchronization;
- PostgreSQL or Qdrant before measured SQLite/sqlite-vec limits justify them;
- compatibility migrations for unreleased generic-node databases;
- E2E encryption, a web client, and multi-user collaboration in the current milestone.
