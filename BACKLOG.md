# Review Findings Backlog (parking lot)

Findings from the 2026-07-19 multi-agent review that are **deliberately not in any active plan** (`SEARCH_AND_FIXES_PLAN.md`, `EMBEDDINGS_PLAN.md`, `MD_IMPORT_EXPORT_PLAN.md`). Each entry names its trigger — the event that should promote it into a plan. Do not implement from this file directly.

## Correctness / robustness

- **`http://` server URLs accepted end-to-end** (`src-tauri/src/settings.rs:234`, `crates/notes-sync/src/transport.rs:425`): bearer token travels plaintext if the reverse proxy is misconfigured. _Trigger: cheap — require https unless host is localhost/LAN, or show a persistent warning badge; fold into the next server/security batch (Track C follow-up)._
- **Startup `expect` panics before the friendly startup-error path exists** (`src-tauri/src/lib.rs:142-143`). _Trigger: first report of a blank-window crash; convert to the managed startup-error state._
- **Server writes snapshot files nothing reads** (`server` snapshots/{seq}.json; `GET /v1/snapshot` always exports fresh). _Trigger: next server touch — either wire bootstrap to them or delete the write path._

## Performance (post-import watch list)

- **Outliner: per-parent RPC fan-out + no virtualization** (`block-tree.tsx:32`; violates EDITOR*ARCHITECTURE.md:230). Hundreds of blocks fine, thousands hurt. \_Trigger: measure on the real imported corpus (17k blocks); fix = one batched children RPC + list virtualization. Deliberately needs a perf baseline first.*
- **Single serialized DB connection on the client** (`notes-core/src/sqlite.rs`): bulk import/export and long queries block interactive reads. _Trigger: if the Logseq import or big-page work feels frozen; fix = small read-only WAL connection pool._
- **Client search limit hardcoded at 20** with no load-more (UI). _Trigger: first time a real search needs result 21; fold into palette follow-up._

## Security / multi-user (single-user today)

- **Client sync token in plaintext `settings.json`** (0600) instead of OS keychain (`settings.rs:268-282`). _Trigger: before any shared-machine use; Android keystore is the hard part._
- **Blob GC + per-user story beyond quota** (C3 adds ownership/quota; GC of unreferenced blobs remains). _Trigger: server disk growth or second user._
- Upstream-known (README "remaining work", not review findings): oplog compaction, device registration/pairing, scoped credentials.

## UX / product

- **RU/EN translated near-duplicates** both surface in semantic results (no translation-dedup anywhere). _Trigger: eval harness shows it hurting real queries; otherwise accept._
- **No sub-space/namespace filtering**: repo docs + blog + personal notes share one retrieval space; dense technical pages will dominate related queries. _Trigger: after MD import lands and pollution is felt; ties into the roadmap's properties/saved-queries plans._
- **`outliner.collapsed.<uuid>` localStorage keys accumulate forever** (`block-node.tsx:110-111`), surviving page deletion. _Trigger: trivial janitor task any time — prune keys whose UUID no longer exists at startup._

## Approved, awaiting scheduling

- **Rewrite-on-rename** (approved 2026-07-19): renaming a page rewrites all inbound `[[old title]]` links to the new title (find them via the `page_links` index on `target_title`; ordinary synced block-edit ops), behind a confirmation showing the link count. This frees old names safely (no dangling links, no silent capture when the old name is reused) and makes auto-aliasing unnecessary. Manual aliases stay as-is — they are for deliberate synonyms, not rename history. Journals unaffected (no titles). _Trigger: promote into a plan before rename sees real use — i.e. shortly after the Logseq import._

## Design debt (needs a design session, not a task)

- **Undo boundary is inconsistent**: document edits and structure are in persistent history; outline typing undo is editor-local and dies on blur/restart (`db/blocks.rs:198` bypasses `record_action`). _Trigger: after B6 (sync-safe undo) lands — same subsystem, decide the model once._
- **`reconcileRemoteDraft` is nearly vestigial** (`editor-sync.ts`) — real reconciliation lives in the page-session registry. _Trigger: next outliner refactor; fold/remove._
- **Graph view is prototype-grade** (fixed ellipse layout, refetch-on-click). _Trigger: when the graph becomes a daily tool; consciously parked._

## Recently resolved elsewhere (for context, keep list short)

- 2026-07-19 frontend review findings → fixed in `f1c7dc3`.
- Promoted 2026-07-19: journal-date search → SEARCH_AND_FIXES_PLAN A5.7; permanent-WorkspaceConflict parking → SEARCH_AND_FIXES_PLAN B8; visible HTML blocks → MD_IMPORT_EXPORT_PLAN M6.
- Search/correctness/security/hygiene batches → `SEARCH_AND_FIXES_PLAN.md` Tracks A–D (in execution).
- Embedding composition/chunking/eval → `EMBEDDINGS_PLAN.md` (pending execution).
- MD import/export + code-fence language + block stats → `MD_IMPORT_EXPORT_PLAN.md` (pending execution).
