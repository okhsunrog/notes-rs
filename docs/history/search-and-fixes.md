# Search Overhaul + Review-Findings Plan

> Historical development record. Task ordering and completion claims reflect the
> original investigation, not a current execution plan. Verify against current code
> before using outstanding items. Source paths are relative to the repository root.

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
2. Fallback order is strict and short-circuiting: run the existing `instr()` scan only when strict FTS yields zero rows (preserves mid-word substring matches when no strict match exists — a coexisting strict hit intentionally suppresses infix-only matches, reviewed and accepted 2026-07-19); only when both strict FTS and `instr()` yield zero rows, run a relaxed-prefix FTS query. Relaxation applies only to Cyrillic tokens of at least 6 characters, shortens the query stem by at most 2 characters, and never below 4 characters. FTS results always rank above fallback results; dedup by page UUID. This is a deliberate narrow exception for common Russian inflections that Snowball does not reduce to the same stem.
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
4. Zero results → action row "Create note "<query>"". **Approved backend change (2026-07-19, rev. 2 — supersedes the earlier "no dedup" wording, which contradicted the title-uniqueness pillar):** unique titles stay authoritative (they are what makes `[[title]]` links resolve unambiguously — do not touch the unique index, rename conflicts, or LWW title ownership). Extend `create_note` to `create_note(title: Option<String>)` with **create-or-open** semantics, atomic in one transaction: title free → create page + initial block, return `Created(note)`; title taken (normalized match) → change nothing, return `Existing(page)` as a typed result variant (not an error). Trim the title; empty/whitespace = `None`; same title validation as the rename path; regenerate bindings; backend tests for Created, Existing, and the `None` arm. Palette UX: on `Existing` → open that page; label the action row "Open "<query>"" when current results contain an exact title match, "Create "<query>"" otherwise (the atomic command covers the race either way). When a title was prefilled and the note was Created, skip title focus and place the caret in the first block (`autoFocusTitle` only for untitled notes). Ctrl+N keeps passing `None`.
5. Date-like queries open journals: when the query parses as a journal date (reuse `parseJournalDate` from `src/features/journal/journal-date.ts`; also accept ISO `YYYY-MM-DD`), show an "Open journal <date>" action row above results, routed via the controller's `openJournal`. This is additive — normal search still runs. Unit-test the query→date detection as a pure function.
6. List stability: once the user presses ↑/↓, freeze list updates until the next keystroke; when the server list replaces the local list, preserve selection by content UUID when possible.
7. The inline/card variants of `SearchCard` on Home keep working (shared logic, different chrome).

**Accept:** palette navigable entirely by keyboard; no layout jumps when server results arrive; existing search tests updated, new tests for freeze/selection-preservation logic (pure helpers, not DOM timing).

### A6. Search settings section (TS + small Rust)

Settings → Search: (a) AI search on/off; (b) AI trigger: as-you-type vs Enter-only (cost control); (c) reranker on/off — add an optional `rerank: bool` (default true) to the search request in `notes-protocol` and thread it through `server/src/ai.rs` → `RetrievalPipeline` (skip rerank stage, keep RRF order); (d) hidden/advanced: per-result source badges (local/server) for debugging. Persist via the existing typed settings snapshot. Anti-scope: no ranking-mechanics knobs.

**Accept:** toggles round-trip through settings; rerank=false verified by a server-side test; bindings regenerated.

---

## Track A-fix — findings from the Track A review checkpoint (execute BEFORE B1)

Multi-agent review of `f1c7dc3..562d687` (2026-07-19): 9 confirmed defects + hygiene + one approved ranking amendment. Same global rules and gates. One commit per task, in this order. Verifier-supplied fix directions below are starting points, not straitjackets — but scope stays per-task.

### AF1. create_note must resolve aliases (Rust — data integrity, highest priority)

`create_note`'s existing-title probe (`crates/notes-core/src/db/pages.rs:230`) is a raw `WHERE normalized_title = ?1` query; every other title path goes through `operation::resolve_page_alias` (see pages.rs:158), which also matches durable aliases. Creating a note titled with a renamed page's old title therefore creates a duplicate page AND makes `resolve_page_alias` ambiguous (2 candidates → `None`), breaking every existing `[[X]]` link.

Fix: replace the probe with `operation::resolve_page_alias(&transaction, &normalized_title)` (drop-in; `Transaction` derefs to `Connection`), returning `Existing` with the resolved page. Ambiguous (`None` with candidates) falls through to create — consistent with navigation semantics. Tests: alias-owned title returns `Existing` (add beside `note_creation.rs:71`); current-title case still covered.

### AF2. Snippets from real markdown, not the stemmed column (Rust)

`snippet(blocks_fts, 0, …)` (`crates/notes-core/src/db/search.rs:136`) reads `body_stemmed` — users see mangled stemmed text ("заметк о программирован"). FTS5 snippet offsets are computed against the stemmed text and cannot be mapped back. Build the display snippet Rust-side from `block.markdown` (already selected via `QUALIFIED_BLOCK_COLUMNS`): tokenize the raw markdown with offsets, stem each token, mark tokens whose stem matches any stemmed query token, window ±~60 chars around the first match at word boundaries, wrap matches in `<mark>` (keep the existing `<mark>`/`…` contract with the TS renderer). Tests MUST use stem-variant fixtures (e.g. block "системах программирования", query "система" → snippet shows "систем**ах**…" raw text, not the stem).

### AF3. Prefix mode for the as-you-type block channel (Rust)

`search_fts` passes `SearchTokenMode::Plain` to `search_blocks_ranked` (`db/search.rs:160`) while the page channel hardcodes `Prefix` — partial trailing words match titles but zero blocks. Fix: use `Prefix` for the block channel too (matching `((` autocomplete). Test: partial word ("prog") matches a block containing "programming" via the `search_fts` path.

### AF4. Delete the dead hits plumbing; restore live updates inside the card (TS)

The workspace `hits`/`setHits` channel is fully dead (SearchCard owns and republishes its own state; nothing renders `workspace.hits`; even the seed is wiped on mount). Consequences: rename no longer updates titles in displayed results, delete leaves stale hits, and the publish effect re-renders the whole Workbench twice per keystroke behind the modal.

Fix: remove `hits`/`setHits` from `useNotesWorkspace` (state, all 8 mutation sites, return), the props through App.tsx → workbench.tsx → home-view.tsx, and SearchCard's seed + publish effect. Restore live updates inside SearchCard: subscribe to the pages list query (`queryKeys.pages`, staleTime is Infinity so this is push-driven by domain events) and re-run the local channel when it updates while a non-empty query is active — this covers both rename and delete. Tests: rename/delete while results displayed refreshes the list (component-level, pure where possible).

### AF5. Pending-Enter during the debounce window (TS)

Enter within ~150–300 ms of typing is a silent no-op (selection nulled, rows empty, create/open rows settled-gated). Implement a pending-enter flag: in the Enter branch, when query is non-empty and search not settled, `preventDefault` and set the flag (capture `shiftKey` alongside); an effect fires `activateRow(rows[0], capturedShift)` once settled and rows exist; any further input clears the flag. Test the helper logic as pure functions where possible.

### AF6. Enter-only mode must survive a failed request (TS)

`triggerEnterOnlySearch` requires `serverState === "idle"`; failure sets `"failed"` and nothing transitions back without editing the query — one transient timeout permanently kills AI retry for that query, invisibly. Fix: allow triggering from `"failed"` too, and render a visible failed hint near the input ("AI search failed — press Enter to retry"). Test both.

### AF7. Local-channel errors must not masquerade as empty results (TS)

`runLocal`'s bare `catch` swallows FTS command failures; the settled-empty state then shows the `Create "<query>"` row — an IO failure invites creating a duplicate-intent note. Fix both halves: restore `notifyError("search", err)` in the catch, AND track a distinct local-error state that suppresses the Create/Open action rows until a successful (possibly empty) result. Plan rule A4.2's silence applies to the server channel only.

### AF8. Wire compatibility for the rerank field (Rust)

`SearchRequest.rerank` is always serialized; old servers use `deny_unknown_fields` → 400 on every semantic search during app-before-server version skew, silently. Fix: omit the field when it equals the default (`skip_serializing_if` with an is-true helper on `crates/notes-protocol/src/lib.rs:113`); add the new-client→old-server shape to `wire_contract.rs`; one README line documenting server-before-client deploy order.

### AF9. Stop paying rerank for abandoned prefixes (TS + copy)

Superseded semantic requests run the full pipeline to completion (no cancellation path; `withTimeout` doesn't abort), each billing a provider rerank over up to 80 docs — and as-you-type + rerank is the default config. Accepted product tradeoff for now: **as-you-type server requests always send `rerank: false`** (cheap hybrid results live), and full reranking applies when AI search fires via the Enter-only trigger; update the Settings copy for the rerank toggle accordingly ("applies when AI search runs on Enter"). If this tradeoff looks wrong mid-implementation, stop and report. Full request cancellation (à la `cancel_chat`) is explicitly out of scope for this task.

### AF10. Test hygiene (TS + Rust)

1. `search-palette.test.ts` and `search-presentation.test.ts`: import from `"vite-plus/test"` (repo convention, 42/45 files; `vitest` is an undeclared transitive dependency).
2. Add one operator-laden test for `relaxed_stem_prefix_search_query` (e.g. input `покупок OR NEAR`) asserting FTS5 keywords are quoted — the relaxed path currently has zero operator coverage while its sibling does.

### AF11. Navigational-title ranking tier (Rust — approved amendment to A3)

The blocks-first interleave plus exact-only title priority ranks the A2 morphology case ("проекта" → page «Проекты») below ANY block containing "проект". Approved fix: promote **stemmed-token-set equality** matches into the priority tier — if the stemmed token set of the query equals the stemmed token set of the page title (order-insensitive), treat it like `exact_title` in the partition. This preserves the existing guarantees: "Rust scratchpad" for query "rust" stays non-priority ({rust} ≠ {rust, scratchpad}), so the strong-body-beats-weak-title test is unaffected. Tests: «проекта» ranks page «Проекты» above a block containing "проект"; existing exact-title and strong-body fixtures unchanged.

### AF12. Cleanup batch

In one commit, in `search-card.tsx` unless noted: collapse the six `frozen*` states into one nullable snapshot object set atomically on first arrow key; merge the four copy-pasted row-render map blocks into one indexed map (drop the O(n²) `rows.indexOf`); have `presentSearchResults` return `primarySource` instead of the mirrored condition in the card; replace raw timer refs with the existing `DebouncedAction` (`src/lib/debounced-action.ts`); simplify `blockPageUuids` to a single flatMap; in `db/search.rs` remove (or re-justify with a comment) the dead cross-tier dedup `retain` in `search_pages_ranked`.

### AF13. Follow-ups from the A-fix spot-check (2026-07-19; do before or between B tasks, one commit)

1. **Enter-only AI trigger vs pending-Enter ordering** (`search-card.tsx:402-408`, medium-high): in enter-only mode, Enter pressed inside the 150 ms local-debounce window is captured as pending-Enter and later activates the top row — the AI search the mode exists for silently never fires; behavior is timing-dependent. Fix: when enter-only mode can fire (query non-empty, state idle/failed), `triggerEnterOnlySearch` takes precedence over the pending-Enter capture. Add a test that presses Enter WITHOUT advancing timers past the debounce (the current helper settles first, which is why this escaped).
2. **Freeze-preserving refresh** (`search-card.tsx:226`): the pages-change refresh unconditionally `setResultsFrozen(false)`, dropping the user's arrow-selection mid-navigation when a background sync/domain event lands. Refresh the underlying data while keeping the freeze: update the frozen snapshot in place instead of unfreezing.
3. **AF2 fallback cap** (`db/search.rs:192`): when the raw re-tokenizer finds no match (e.g. digit-adjacent tokens like `abc123`), the fallback returns the whole markdown unbounded. Cap it to the same window length (truncate at a word boundary + `…`), no marks.
4. _(optional, may defer with a note)_: the pages-query subscription doesn't cover `blocks_changed`/`blocks_deleted`, so an edited/deleted block stays stale in open results (pre-existing limitation, not a regression). If a clean signal exists (e.g. re-run on history-status change), wire it; if it gets ugly, leave a code comment and report.

Accepted as-is from the spot-check (no action): AF1 ambiguous-alias fallthrough can duplicate a `normalized_title` (explicitly sanctioned by spec; no unique index exists); literal `<mark>` in note content mis-toggles snippet highlighting (cosmetic, contrived).

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

### B8. Park permanent workspace conflicts instead of retrying forever (Rust)

The sync loop keeps retrying a permanent `WorkspaceConflict` (divergent-workspace bootstrap rejection) indefinitely (`src-tauri/src/sync.rs:152-192`) — endless error/retry flapping with no exit. Make it park: stop the retry loop, surface a distinct `SyncStatus` state (e.g. `conflict` with an action-required message shown by the existing sync badge), and resume only when sync settings change or the user explicitly retries from Settings. Do not auto-resolve or reset anything. Test with a synthetic workspace conflict: loop parks, status is `conflict`, settings change resumes.

---

## B6-fix — CRITICAL, interrupt Track C and do this first

Adversarial review of 48b8621 (2026-07-19, verified by repro tests): the guard mechanism, all-or-nothing entry semantics, pipeline integration (outbox/HLC/effects), and redo symmetry are all sound — but two defects ship-block:

### B6F1. Sibling-guard refresh (CRITICAL)

Applying an undo writes a fresh HLC to the field, but guards of the remaining entries in both stacks are never refreshed (`move_history` only rewrites the moved entry's own opposite payload, `history.rs:601-607`). Confirmed repro: `set_block_style` → `set_task_state` → undo (Applied) → undo → **Skipped**, entry deleted, "changed on another device" toast with sync disabled. Any same-field action sequence kills the rest of the stack after one undo.

Fix: after a successful apply in `move_history`, refresh the guards of all remaining entries (both stacks) whose guarded fields overlap the fields just written (`next_guards`) to the new HLCs, in the same transaction. Tests: consecutive undo of two same-field actions both apply; three-deep stack; undo→redo→undo cycles.

### B6F2. Guard the inverse's dependencies, not just the forward's fields (HIGH)

`capture_history_guards(transaction, &forward)` guards forward-op fields only; a `BlockDelete` entry carries just `BlockExistence{uuid}`, but its inverse `BlockCreate` also requires the _page_ to be alive. Confirmed repro: local block delete → remote `PageDelete` → undo returns **Applied** while `BlockCreate` writes a tombstone (operation.rs:1643-1652) — nothing restored, fake success toast, plus a redo entry whose forward re-deletes a nonexistent block. Fix: capture guards for the inverse ops' dependencies (page existence for block restore; audit the other inverse kinds for analogous dependencies). Test: the repro above must return Skipped.

### B6F3. Test batch + copy

Add the missing coverage named by the review: redo-after-remote-skip; multi-op entry where only one field changed remotely (all-or-nothing pinned); structural guard (`BlockMove`); undo-of-delete-after-remote-parent-delete; a legacy v1 entry facing a real remote conflict. Also neutralize the skip message ("Undo skipped — the content changed since" instead of unconditionally blaming another device). Accepted as documented tradeoffs, no action: legacy v1 entries get one unguarded apply; one-skip-per-keypress UX.

### B6F4. Guard destructive-scope inverses on content (MEDIUM — final B6 follow-up, 2026-07-19 verification)

Repro'd gap of the B6F2 class: inverse `PageDelete` (undo of note creation) guards the page's own fields but not its **contents**. Local create-note → remote `BlockCreate` on that page → undo returns Applied, deletes the page including the remote block, and the local tombstones out-HLC the remote op so the loss propagates on sync. Same class: undo of a block-create after a remote child was added under it.

Fix: for destructive inverse ops (`PageDelete`/`BlockDelete` appearing as inverses of creates), capture a **scope guard** at record time (post-forward-apply, same transaction): the set of child/descendant block uuids the destructive inverse is entitled to remove. At undo time, skip the entry if the target now contains blocks outside the captured set. Tests: the two repros above return Skipped; normal create→undo (no remote additions) still Applies; a legacy v1 entry stays on its documented single unguarded apply.

Accepted as documented semantics, no action (from the same verification): mid-history guard laundering by sibling refresh (linear-undo semantics — add one code comment); dependency-field over-refresh (no data-loss path constructed); ~200 bounded JSON decodes per move.

## B-polish — after B6-fix, one commit

1. **B4 graph gating**: `BlockSetMarkdown` now unconditionally emits GraphChanged → `graphRoot`+`backlinksRoot` refetch on every autosave while typing. Restore gating on actual reference change (the old local path used the DB's `graph_changed` signal; thread that through `events_for_ops` input rather than re-diffing).
2. **append_to_journal**: subsequent captures no longer emit PagesChanged — verify nothing depends on the `journals` query refetch (journal list previews); if something does, emit it; if not, leave a comment.
3. Remove orphaned `stablePaletteItems` (production references gone after AF12; delete with its test).

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
2. README "What works": add Logseq import, journals, Document/Live Preview, pane workspace (all shipped). Fix ../architecture/workspace.md:3 "Implementation is pending" and ../architecture/sync-storage-ai.md stale "pending" claims (CodeMirror/Document/Journal shipped); add `PageAliasSet` to the plan's op table; correct "HTTP/SSE" → "HTTP/WebSocket (SSE is chat-only)".
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
