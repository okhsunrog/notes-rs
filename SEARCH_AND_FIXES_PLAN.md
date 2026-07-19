# Search Overhaul + Review-Findings Plan

Self-contained handoff plan for an executor agent. Work through the tracks **in order** (A → B → C → D); within a track, one commit per task. Every task lists steps, guardrails, and acceptance criteria. If something doesn't match this plan, or a decision arises that the plan doesn't cover: **stop and report** — do not improvise.

## Context

Tauri 2 note-taking app: Rust core (`crates/notes-core`, `notes-sync`, `notes-ai`, …), Axum server (`server/`), React 19 frontend (`src/`), generated bindings (`src/lib/bindings.ts` — **never edit by hand**; regenerate with `vp run bindings:generate` after Rust command changes). Search today: local SQLite FTS5 (blocks, stemmed EN+RU) + a server-side semantic pipeline (hybrid FTS+vector → RRF k=60 → provider rerank, floor 0.30). The UI currently makes the user pick a mode; this plan replaces that with one progressive unified mode and fixes defects found in a multi-agent code review.

## Global rules

1. Validation gates after every task: `vp check`, `vp test`; for tasks touching Rust also `cargo fmt --all -- --check`, `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings`, `cargo test --workspace --locked --all-features`.
2. Add JS deps with `vp add`, Rust deps with `cargo add`. Never hand-edit package.json/Cargo.toml for adds.
3. Do not reformat or "clean up" code outside the files a task names. Preserve user-facing strings unless a task changes them deliberately.
4. Schema changes go in a new migration (`V002__…`) — never edit `V001__initial.sql`.
5. Commit per task, conventional style (`fix(search): …`, `feat(search): …`, `fix(server): …`).
6. Safe stopping points are the track boundaries. Report progress there.

---

## Track A — Unified search

### A1. Fix prefix search (Rust)

`stem_search_query` (`crates/notes-core/src/stem.rs:83`) strips `*`, so prefix queries silently never match — this breaks `((` block autocomplete (`src/features/outliner/block-node.tsx:944`) and blocks as-you-type.

1. Add a prefix-aware query builder: stem all tokens as today; for the **last** token emit `("raw"* OR "stemmed"*)` (both quoted, `*` outside quotes — valid FTS5). Expose as an explicit option (e.g. `SearchTokenMode::Prefix`) used by autocomplete and (later) as-you-type; the plain path stays unchanged.
2. Keep the sanitization guarantee: all operators stripped from user input, every token quoted. Extend the tests in `stem.rs` with: prefix query matches longer word ("prog" → "programming"), operator injection still inert, empty/`*`-only input.
3. Wire block autocomplete to the prefix path.

**Accept:** typing a partial word in `((` autocomplete matches completions; all stem tests pass.

### A2. Title search via `pages_fts` with substring fallback (Rust) — decision: Variant C

`pages_fts` is created and trigger-maintained (`V001__initial.sql:213`) but never queried; titles use an `instr()` substring scan (`crates/notes-core/src/db/search.rs:3`).

1. Primary: query `pages_fts` with the stemmed+prefix query from A1, ranked by `bm25()`.
2. Fallback order is strict and short-circuiting: run the existing `instr()` scan only when strict FTS yields zero rows (preserves mid-word substring matches); only when both strict FTS and `instr()` yield zero rows, run a relaxed-prefix FTS query. Relaxation applies only to Cyrillic tokens of at least 6 characters, shortens the query stem by at most 2 characters, and never below 4 characters. FTS results always rank above fallback results; dedup by page UUID. This is a deliberate narrow exception for common Russian inflections that Snowball does not reduce to the same stem.
3. Verify what `pages_fts` indexes (stemmed title?) before writing the query; if it indexes raw titles, index the stemmed form via the migration + trigger update (new migration, per global rule 4).
4. Tests: regular Snowball morphology uses a pair that actually shares a stem (for example, "проекта" finds "Проекты"); relaxed morphology separately proves "покупок" finds "Покупки"; a short token is never relaxed; an existing strict hit prevents relaxed results from being queried; word order ("esp32 прошивка" finds "Прошивка ESP32"), mid-word fallback still works, and exact-match still ranks first. The broader `покуп*` family, including "Покупатель", is relevant rather than an error at this fallback tier.

The analogous Russian morphology gap in block search is accepted debt. Do not add relaxed block fallback in this task: the relevance surface is wider there, and the as-you-type prefix path already mitigates the interactive case. Revisit it only with a real-query evaluation harness.

**Accept:** all above tests green; `instr` scan only runs on FTS miss.

### A3. Snippets + honest fusion (Rust)

1. Add `snippet(blocks_fts, …)` output to block hits (ellipses + match markers); surface it through the command payload (extend `SearchHit` — regenerate bindings).
2. Replace the synthetic `1.0/rank` vs `0.9/rank` title/block fusion (`db/search.rs:52`) with real bm25-based ordering: normalize bm25 within each list or interleave by rank with title-priority only on exact title match. Document the chosen rule in a code comment.

**Accept:** search results carry snippets; a strong body match can outrank a weak title match (add a test fixture proving it).

### A4. Unified mode: two-channel state machine (TS)

Remove the mode selector. One query, two channels:

1. Local channel: FTS as-you-type, debounce ~150 ms, request-id/epoch cancellation (reuse the pattern from `block-node.tsx:325`).
2. Server channel: fires only if a server is configured, query ≥ 3 chars, after ~400 ms pause; new input cancels in-flight. Server errors/timeouts are silent (state, not toasts).
3. Pure merge function in a new module, fully unit-tested with **no network mocks**:
   `present({localHits, serverHits, serverState}) → {primary, localExtras}` where:
   - serverHits non-empty → primary = serverHits (order untouched); localExtras = top-5 local hits whose content UUID (page/block, use `contentUuid`) is absent from serverHits;
   - serverHits empty or serverState failed/absent → primary = localHits, localExtras = [];
   - never merge ranks; never reorder server results.
4. UI: primary list, then a visually-secondary "Found locally" section only when localExtras is non-empty; subtle "AI…" indicator while the server channel is pending. No mode select, no submit button (keep Enter working).
5. Anti-scope: **no mixing weights, no client-side RRF** — the server list is authoritative when present.

**Accept:** matrix of 4 states covered by unit tests on the pure function; mode selector gone; `vp test` green.

### A5. Command palette UI (TS)

Restyle the search dialog into a palette (Base UI Dialog stays):

1. Borderless top input (autofocus), results list with type icon (page/block/journal), title, snippet (from A3), parent-page breadcrumb for block hits; footer hints `↑↓ · ⏎ open · ⇧⏎ open beside · esc`.
2. Keyboard: ↑/↓ selection, Enter opens, Shift+Enter opens with `adjacentDisposition` (use `dispositionFromShiftKey`), Esc closes.
3. Empty query → recent pages (reuse existing pages/journals queries) + today's journal entry point. No blank screen.
4. Zero results → action row "Create note "<query>"" wired to `createNewNote` + title prefill (verify what the create flow supports; if title prefill needs a backend change, stop and report instead of hacking it).
5. List stability: once the user presses ↑/↓, freeze list updates until the next keystroke; when the server list replaces the local list, preserve selection by content UUID when possible.
6. The inline/card variants of `SearchCard` on Home keep working (shared logic, different chrome).

**Accept:** palette navigable entirely by keyboard; no layout jumps when server results arrive; existing search tests updated, new tests for freeze/selection-preservation logic (pure helpers, not DOM timing).

### A6. Search settings section (TS + small Rust)

Settings → Search: (a) AI search on/off; (b) AI trigger: as-you-type vs Enter-only (cost control); (c) reranker on/off — add an optional `rerank: bool` (default true) to the search request in `notes-protocol` and thread it through `server/src/ai.rs` → `RetrievalPipeline` (skip rerank stage, keep RRF order); (d) hidden/advanced: per-result source badges (local/server) for debugging. Persist via the existing typed settings snapshot. Anti-scope: no ranking-mechanics knobs.

**Accept:** toggles round-trip through settings; rerank=false verified by a server-side test; bindings regenerated.

---

## Track B — Correctness fixes from review

### B1. Outliner: Enter/paste split must flush in-flight saves (TS)

`onEnter` splits using the overlay base revision without awaiting autosave (`block-node.tsx:414`), racing a just-acknowledged save into a spurious conflict. Make split/paste-split `await flush()` first, exactly like style/indent ops already do (`block-node.tsx` ~289). Add a test that types → immediately Enter → no conflict.

### B2. Chat: stream inactivity timeout (TS)

A hung stream leaves `busy` forever (`use-assistant-controller.ts:47`). Add an inactivity timeout (e.g. 60 s without an event) that dispatches the existing `fail` path with a clear message and clears busy; cancel timer on every event and on completion/cancel.

### B3. Document editor: map caret through external rebase (TS)

External updates clamp selection to doc length (`continuous-document-editor.tsx:251`), teleporting the caret on remote rebase. Map the selection through a minimal diff (common prefix/suffix of old/new text) before clamping. Test: remote insert above caret keeps caret on the same logical line.

### B4. Derive DomainEvents from ops (Rust)

Commands hand-emit events while `sync.rs:428` has a parallel OpKind→event mapping — a new mutation can silently forget to emit. Generalize the sync worker's mapping into one shared `events_for_ops(&[OpKind]) → Vec<DomainEvent>` in a common module; local command paths call it with the ops they just applied (plus any UI-only extras they need). Delete per-command hand-emission where the derived set covers it. Test: every `OpKind` variant maps to a non-empty event set (exhaustive `match` so a new variant fails compilation, not silently).

### B5. Version the persisted op envelope (Rust)

`history_undo.forward_json`, `sync_outbox.envelope`, and server oplog store untagged serde `OpKind`. Add a format-version tag to the envelope (accepting untagged as v1 on read), plus decode-compat tests pinning the current wire format (golden JSON fixtures). Coordinate client/server: both read old+new, write new. No migration of stored rows needed if v1 stays readable.

### B6. Sync-safe undo (Rust)

Inverse ops replay verbatim with no revision check (`crates/notes-core/src/db/history.rs:307`) — undo after a remote edit silently clobbers it. At undo/redo apply time, validate each inverse op's captured per-field HLC against the target's **current** HLC: on mismatch, skip the entry, drop it from the stack, and return a distinct result so the UI can notify "Undo skipped — changed on another device" (extend `HistoryStatus`/result enum; regenerate bindings; frontend shows `notifyInfo`). Tests: undo after remote LWW win is skipped; normal undo unaffected; convergence proptests still green. **This is the riskiest task in the plan — if the history model resists this cleanly, stop and report with findings rather than forcing it.**

### B7 (optional). Reclassify sync sequence-gap as retriable (Rust)

A >1000-op WS backlog produces a sequence-gap `SyncConflict` mapped to permanent `Error` before HTTP catch-up self-heals (`server/src/api.rs:489`, client `is_permanent_failure`). Make sequence-gap retriable so the UI doesn't flap into Error; optionally page WS catch-up. Test with a synthetic gap.

---

## Track C — Server security hardening

_(single-user today; these close the holes before any second token exists)_

### C1. Gate AI provider mutation + probe

`PUT /v1/ai/provider` and `POST /v1/ai/provider/probe` (`server/src/api.rs:262,279`) let any authenticated user repoint provider base-URLs — the server then sends its stored key to the new URL (exfiltration + SSRF). Add an `admin = true` flag to the user entry in server config; require it for these two endpoints (403 otherwise). Never send a _stored_ secret to a base URL that arrived in the same request unless that request also supplied the secret. Tests: non-admin 403; probe with new URL + no key does not attach the stored key.

### C2. Token hashing + constant-time compare

Config accepts `token_sha256 = "…"` per user (legacy plaintext `token` still accepted with a startup warning); lookups compare digests constant-time (`subtle` crate or equivalent) instead of `HashMap` key lookup on the raw token (`server/src/state.rs:78`).

### C3. Blob ownership + quota + close the dedup oracle

`PUT /v1/blobs/{hash}` (`server/src/api.rs:602`): record per-user ownership on upload, enforce a configurable per-user byte quota (413/insufficient-storage on exceed), and return one uniform success status for both new and already-present uploads (verify the client treats 201/204 identically first — `crates/notes-sync/src/transport.rs`; if not, align the client in the same commit). GC of unreferenced blobs is **out of scope**.

---

## Track D — Hygiene

### D1. Lazy-load the KaTeX math pipeline (TS)

Carried over from the retired frontend plan. `markdown-renderer.tsx:1` eagerly imports KaTeX CSS + rehype-katex + remark-math. Mirror `mermaid-runtime.ts`: a `math-runtime.ts` that dynamically imports all three, memoized; cheap `$` detection gates loading; render without math plugins while loading, re-render after. Keep `remark-math-limits` applied whenever math is active. Tests: math renders after async load; non-math input never loads the chunk. **Accept:** `vp build` shows KaTeX in a separate chunk.

### D2. Sanitizer regression tests (TS)

Expand `mermaid-sanitize.test.ts` (URL-attribute vectors, CSS smuggling, nested foreignObject) and add KaTeX-output pinning tests (`\href`, `\includegraphics`, `\htmlData` rejected/inert under `trust: false`). Tests only — no production changes unless a test finds a real hole; if it does, stop and report.

### D3. Repo/docs cleanup

1. Remove the committed release binary payload (`release-server/notes-server`, `notes-server-release.tar.gz`) from the working tree and add ignore rules; note in README that `just package-server` produces it.
2. README "What works": add Logseq import, journals, Document/Live Preview, pane workspace (all shipped). Fix WORKSPACE_ARCHITECTURE.md:3 "Implementation is pending" and SYNC_ARCHITECTURE_PLAN.md stale "pending" claims (CodeMirror/Document/Journal shipped); add `PageAliasSet` to the plan's op table; correct "HTTP/SSE" → "HTTP/WebSocket (SSE is chat-only)".
3. Note the Zustand workspace store + notify layer in README's architecture section (one paragraph).

---

## Explicitly out of scope (do not attempt)

- Outliner virtualization / batched children loading (needs a perf baseline first).
- OS-keychain token storage; blob GC; oplog compaction; device pairing.
- Unifying the text-undo boundary (design work, not a fix).
- Any client-side embedding/vector search.
- Graph view improvements.

## Final verification

`vp install && vp check && vp test && vp build` and the full cargo gate (fmt, clippy, test) — all green; bindings regenerated with no drift (CI check); then report per-task commit hashes and any deviations.
