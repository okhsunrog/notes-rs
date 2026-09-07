# Review Findings Backlog (parking lot)

Findings from the 2026-07-19 multi-agent review that are **deliberately not in any active plan** (`../history/search-and-fixes.md`, `../history/embeddings.md`, `../history/markdown-import-export.md`). Each entry names its trigger — the event that should promote it into a plan. Do not implement from this file directly.

## Correctness / robustness

- **`http://` server URLs accepted end-to-end** (`src-tauri/src/settings.rs:234`, `crates/notes-sync/src/transport.rs:425`): bearer token travels plaintext if the reverse proxy is misconfigured. _Trigger: cheap — require https unless host is localhost/LAN, or show a persistent warning badge; fold into the next server/security batch (Track C follow-up)._
- **Startup `expect` panics before the friendly startup-error path exists** (`src-tauri/src/lib.rs:142-143`). _Trigger: first report of a blank-window crash; convert to the managed startup-error state._
- **Server writes snapshot files nothing reads** (`server` snapshots/{seq}.json; `GET /v1/snapshot` always exports fresh). _Trigger: next server touch — either wire bootstrap to them or delete the write path._
- **Server upgrade crash-loops on legacy replicas** (observed 2026-07-20 deploying V002 onto the Jul-17 server data: `V002 … no such table: pages` → systemd restart loop; resolved by the planned wipe). Pre-V002 server user replicas apparently existed in a state the fresh migration chain does not expect. Harmless while wipes are free; must be understood before the first non-wipe production upgrade. _Trigger: before importing data that cannot be re-synced from a client, or before any second user._

## Performance (post-import watch list)

- **Outliner: per-parent RPC fan-out + no virtualization** (`block-tree.tsx:32`; violates EDITOR*ARCHITECTURE.md:230). Hundreds of blocks fine, thousands hurt. \_Trigger: measure on the real imported corpus (17k blocks); fix = one batched children RPC + list virtualization. Deliberately needs a perf baseline first.*
- **Single serialized DB connection on the client** (`notes-core/src/sqlite.rs`): bulk import/export and long queries block interactive reads. _Trigger: if the Logseq import or big-page work feels frozen; fix = small read-only WAL connection pool._
- **Client search limit hardcoded at 20** with no load-more (UI). _Trigger: first time a real search needs result 21; fold into palette follow-up._

## Security / multi-user (single-user today)

- **Client sync token in plaintext `settings.json`** (0600) instead of OS keychain (`settings.rs:268-282`). _Trigger: before any shared-machine use; Android keystore is the hard part._
- **Blob GC + per-user story beyond quota** (C3 adds ownership/quota; GC of unreferenced blobs remains). _Trigger: server disk growth or second user._
- Upstream-known (README "remaining work", not review findings): oplog compaction, device registration/pairing, scoped credentials.

## UX / product

- **Enabling sync from disabled-at-startup requires an app restart**: `spawn_worker` runs once at startup only when credentials exist; `sync_settings_changed()` bumps a watch nobody loops on in that state (observed during B8 review — pre-existing, not a regression; B8's park/resume works for an already-running worker). _Trigger: first time it annoys during real use; fix = spawn the worker lazily on credential save._

- **RU/EN translated near-duplicates** both surface in semantic results (no translation-dedup anywhere). _Trigger: eval harness shows it hurting real queries; otherwise accept._
- **No sub-space/namespace filtering**: repo docs + blog + personal notes share one retrieval space; dense technical pages will dominate related queries. _Trigger: after MD import lands and pollution is felt; ties into the roadmap's properties/saved-queries plans._
- **`outliner.collapsed.<uuid>` localStorage keys accumulate forever** (`block-node.tsx:110-111`), surviving page deletion. _Trigger: trivial janitor task any time — prune keys whose UUID no longer exists at startup._

## Approved, awaiting scheduling

- **Rewrite-on-rename** (approved 2026-07-19): renaming a page rewrites all inbound `[[old title]]` links to the new title (find them via the `page_links` index on `target_title`; ordinary synced block-edit ops), behind a confirmation showing the link count. This frees old names safely (no dangling links, no silent capture when the old name is reused) and makes auto-aliasing unnecessary. Manual aliases stay as-is — they are for deliberate synonyms, not rename history. Journals unaffected (no titles). _Trigger: promote into a plan before rename sees real use — i.e. shortly after the Logseq import._

## History/undo polish (B6F4 verification leftovers, 2026-07-19 — all minor, no data-loss paths)

- Subset-apply of destructive undo scopes is shadowed by B6F2 field guards (disappeared content Skips conservatively instead of applying) — add a code comment and a test pinning whichever semantics is intended.
- Redo-direction and recursion-depth scope scenarios verified by review probes but have no committed tests.
- Theoretical downgrade hole: pre-B6F4 binaries silently drop `scope_guards` on read (same accepted class as the legacy-v1 single unguarded apply). _Trigger for all three: next time anyone touches `history.rs`._

## Design debt (needs a design session, not a task)

- **Undo boundary is inconsistent**: document edits and structure are in persistent history; outline typing undo is editor-local and dies on blur/restart (`db/blocks.rs:198` bypasses `record_action`). _Trigger: after B6 (sync-safe undo) lands — same subsystem, decide the model once._
- **`reconcileRemoteDraft` is nearly vestigial** (`editor-sync.ts`) — real reconciliation lives in the page-session registry. _Trigger: next outliner refactor; fold/remove._
- **Graph view is prototype-grade** (fixed ellipse layout, refetch-on-click). _Trigger: when the graph becomes a daily tool; consciously parked._

## Handwriting integration (2026-09-06 review, accepted for now)

- **Second pane of the same handwritten note stays read-only** after the editing pane closes; editor ownership is acquired once per mount (`handwriting-note-view.tsx`). _Trigger: first time two panes of one handwritten note are used on desktop; fix = re-acquire ownership on `pages_changed`/focus._
- **Title editing hook duplicates `PageView` logic** (`src/features/pages/use-page-title-editor.ts` vs `page-view.tsx`) — extracted without touching the text page. _Trigger: next `PageView` title change; switch `PageView` to the hook._
- **Two Back buttons on a handwritten note in compact layout**: the pane frame's Back and the editor's own Back (flush-then-leave). The frame's Back bypasses the editor lifecycle and lands on the unmount path, which also flushes and completes. _Trigger: e-ink profile UI pass (Track C of `HANDWRITING_UI_PLAN.md`); hide the frame's Back for handwriting panes or route it through the editor's `leave`._
- **`removePage` gives no cancellation signal** (`use-notes-workspace.ts`), so the editor cannot mark itself as leaving before delete; harmless today because a `not_found` completion is ignored. _Trigger: when another caller needs to know whether a delete happened; return `boolean`._

## 2026-09-07 review, deferred

Findings verified during the review that produced Track E of `HANDWRITING_UI_PLAN.md`,
deliberately left out of it.

- **Per-gesture ink I/O is O(document), and `ink_root_refs` grows with it** (`crates/notes-core/src/ink/storage.rs`, patch path): every completed gesture rewrites the whole root record and re-indexes every object it references, so the cost of one stroke scales with the note rather than with the stroke. _Trigger: the first busy-timeout save failure, or a sheet of roughly 50k points feeling slow on the tablet; fix = incremental root/ref maintenance, which needs its own design._
- **Blob ownership is never released and there is no blob GC** (`server/src/blob_ownership.rs`): a user's quota only ever grows, and blobs no operation references any more stay on disk forever. _Trigger: the first quota warning, or a second user._
- **No rate limiting on the hashing endpoints** (`PUT /v1/blobs/{hash}`, `POST /v1/ink/upload`): an authenticated caller can spend server CPU on SHA-256 without bound. _Trigger: a second user, or exposing the server publicly._
- **OAuth client registration is unbounded** (`server/src/oauth`): anyone reaching the endpoint can register clients without limit. _Trigger: same as above._
- **The floating Onyx keyboard is not handled** (`plugins/.../OnyxInk.kt`): the pause registry sees the ordinary IME through window insets, but a floating keyboard does not resize anything and reports no inset. The stock app subscribes to `onyx.action.kime.status.changed` and feeds its `floatingWindowRectList` to `setExcludeRect` instead of pausing. _Trigger: the first use of the floating keyboard over a sheet._
- **`pastey` appears twice in `Cargo.lock`** (two semver-incompatible versions pulled in transitively). Cosmetic: build time and binary size only. _Trigger: next dependency sweep._

## E-ink panel behaviour outside the app (2026-09-07)

- **Full-panel flash on every finger touch** on the Note Air 4C, in every screen and in both display profiles (Standard and E-ink). Verified with a DOM mutation observer: the WebView repaints nothing on a canvas tap, and the plugin receives no call; the firmware refreshes on touch by itself. Not controllable from the app; the per-app EinkWise refresh settings are the user-side knob. _Trigger: if a documented SDK call turns out to suppress it (compare with Chrome and the stock Notes app first)._

## Recently resolved elsewhere (for context, keep list short)

- 2026-07-19 frontend review findings → fixed in `f1c7dc3`.
- Promoted 2026-07-19: journal-date search → SEARCH_AND_FIXES_PLAN A5.7; permanent-WorkspaceConflict parking → SEARCH_AND_FIXES_PLAN B8; visible HTML blocks → MD_IMPORT_EXPORT_PLAN M6.
- Search/correctness/security/hygiene batches → `../history/search-and-fixes.md` Tracks A–D (in execution).
- Embedding composition/chunking/eval → `../history/embeddings.md` (pending execution).
- MD import/export + code-fence language + block stats → `../history/markdown-import-export.md` (pending execution).
