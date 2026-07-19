# Embeddings Polish Plan

Self-contained handoff plan. Goal: bring the embedding/indexing pipeline to its target state **before** the bulk Logseq import, so the corpus is embedded once against a frozen input format, and quality becomes measurable instead of guessed.

**Sequencing:** run this AFTER the current `SEARCH_AND_FIXES_PLAN.md` execution finishes — both touch `crates/notes-ai` (A6 adds a rerank flag in `retrieval.rs`). One commit per task, in order. If anything contradicts this plan: stop and report.

## Global rules

1. Gates after every task: `cargo fmt --all -- --check`, `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings`, `cargo test --workspace --locked --all-features`; run `vp check`/`vp test` too when any TS changes (none are expected except possibly settings copy).
2. All work is server/crate-side (`crates/notes-ai`, `server/`). No client AI, no `src/lib/bindings.ts` hand-edits.
3. Schema changes to `ai.db` are cheap by design (it is disposable and generation-scoped) — still keep them in the store's setup code, versioned.
4. `INPUT_FORMAT_VERSION` (`crates/notes-ai/src/store.rs:13`) must be bumped exactly once in this plan (task E1's commit), not once per task — coordinate: E1–E3 all change the input format; land them, then bump in the final E3 commit if E1's bump hasn't happened yet. A bump triggers a full re-embed by design; that is expected and cheap (< $1 for this corpus).

---

## E1. Fix composed input format

`compose_text` (`crates/notes-ai/src/store.rs:1089`) has a dead branch: the ancestor breadcrumb filters `depth > 0 && title.is_some()`, but the recursive CTE (`store.rs:988`) emits `NULL AS title` for every `depth > 0` row — the breadcrumb never renders, and `depth >= 2` chain rows are fetched but unused.

New format, in order:

```
<page title | journal pseudo-title>
<ancestor excerpts, root → parent, ~150 chars each, word-boundary truncated>
<block markdown>
```

1. Page title first. For journal pages (NULL title), synthesize a pseudo-title from the journal date: `Journal 2026-07-19` (both ISO and the app's display form if cheap to obtain).
2. Ancestor excerpts: use the chain rows the CTE already fetches (all depths, root first), ~150 chars per level, truncated at a word boundary (fix the current `.chars().take(200)` mid-word cut).
3. Skip content-free blocks entirely (dividers, empty, pure markup/punctuation) — no job, no vector. Keep short-but-meaningful blocks: with the title+breadcrumb header they embed fine.
4. **Section context for document-layout pages:** document blocks are flat siblings (headings do not parent the paragraphs that follow), so the ancestor breadcrumb is empty for them. For blocks on document-layout pages, include the nearest **preceding** `heading_*` block (walk backwards in `order_key` order on the same page/parent) as one extra header line after the page title. Do not change the block parenting model to achieve this — it is a composition-time lookup only.
5. Unit tests: journal pseudo-title present; deep chain renders root→parent order; document-layout block picks up its nearest preceding heading (and none when no heading precedes it); word-boundary truncation; content-free block skipped; composition golden test (fixture in → exact string out) so format changes are always deliberate.

## E2. Length safety + per-item degradation

1. Hard cap the composed text sent to providers (~8,000 chars per embed input), with an explicit truncation marker appended when applied. (After E3 the cap applies per sub-chunk and should rarely trigger.)
2. Voyage: pass `truncation: true` explicitly in the request body (`crates/notes-ai/src/embed.rs:362`) — silent-by-default today; make it a decision, not an accident.
3. Batch failure degradation: when a 16-job batch fails with a provider error, retry the jobs **individually** before recording failures, so one poison input (oversized/rejected) doesn't send 15 healthy neighbors into backoff. Terminal-failure classification stays per item.
4. Tests: oversized input gets capped+marked; simulated batch failure embeds the healthy 15 and fails only the poison one.

## E3. Two-tier chunking (blocks are chunks; only the tail splits)

Facts this encodes: blocks are author-drawn semantic units (outline bullets; document-codec units = paragraphs/headings/fences; Logseq import preserves bullet granularity). Average block in the target corpus is ~80 chars; the long tail is code fences and pasted walls of text. Policy:

1. **Tier 1:** composed text ≤ ~2,000 chars → one vector (current behavior).
2. **Tier 2:** composed text > ~2,000 chars → deterministic splitter cuts the block body at natural boundaries, in preference order: blank lines → table rows / lines inside code fences → any line boundary → (only for a single enormous line) hard character cut. Target ~1,200–1,600 chars per sub-chunk, hard cap from E2. A block with no blank lines at all — a large GFM table is the canonical real case (this repo's ROADMAP.md contains a 36 KB table) — must still split, never silently truncate. **Every sub-chunk gets the same E1 header** (title + breadcrumb). No overlap — boundary-aware splits plus the shared header replace it.
3. Storage: `generation_vectors` mapping gains `chunk_index` (1:N per content_uuid). `embedding_jobs` stays one row per content_uuid holding the full composed text; the worker splits just before the provider call. `input_hash` still covers the full composition — any change re-embeds all chunks of that block.
4. Retrieval: KNN may return several chunks of one block — dedup by `content_uuid` keeping the best distance **before** the RRF/rerank pool is built; the rerank stage receives the matched chunk's text (not the whole block).
5. The retrieval unit stays the block: results, UUIDs, and open-behavior are unchanged.
6. Tests: splitter golden tests (fence boundaries, a large table with no blank lines splits at row boundaries, unicode, blank-line runs, exactly-at-threshold); dedup-keeps-best-distance; a Tier-2 block round-trips through reconcile → embed → query.
7. Thresholds (2,000 / 1,200–1,600 / 8,000) are constants in one place with a comment that E7's eval is the tool for tuning them — do not spread magic numbers.

## E4. Page-level vector

One additional embedding per page: title + first N blocks (~1,000 chars budget), stored under the page UUID. Makes title-only pages findable semantically and pages retrievable as entities. Content kind in results is already page-or-block (`SearchHit`), so no client change should be needed — verify and stop if that assumption breaks.

## E5. Seq-gate reconcile

`tick` (`crates/notes-ai/src/embed.rs:695`) runs `index_documents` — a full-corpus recursive CTE + in-memory text build — on every wake, every 30 s idle loop, and again after each batch. Store the last reconciled sync cursor; skip the full rebuild when the cursor hasn't moved (the post-batch race-guard reconcile stays, but also seq-gated: the guard matters only if content changed mid-flight, which moves the cursor). Test: two consecutive ticks with no oplog movement run `index_documents` once.

## E6. Index inspector (dry-run) CLI

Server binary subcommand (e.g. `notes-server inspect-index --db <path>`): runs composition WITHOUT embedding and prints: document count, composed-length histogram, top-20 longest with UUIDs, estimated token count and provider cost, count of skipped content-free blocks and Tier-2 blocks; `--uuid <id>` prints the exact composed text (and sub-chunks) for one block. Purpose: eyeball real inputs before spending on the corpus.

## E7. Eval harness

Server subcommand `notes-server eval --queries <file>`: reads a TOML golden set (`[[query]] text = "..." expect = ["uuid", …]`), runs each through the full retrieval pipeline (configurable: with/without rerank), reports recall@5/@10 and MRR overall and per query, and lists misses with what ranked above them. No provider mocking — it runs against the real configured pipeline; document that it costs a few cents per run. This is the tool that turns every future tuning question (model choice, thresholds, rerank floor) into a number.

The golden-set template (ship an example `eval-queries.example.toml`) must include a **morphology-debt section**: 3–4 Russian queries using inflected forms whose Snowball stems diverge from the indexed form (fleeting-vowel genitives like «покупок» → «Покупки», and similar irregular pairs), tagged `group = "morphology"`. Context: SEARCH_AND_FIXES_PLAN.md Track A2 deliberately fixed this only for title search and left block-level Russian morphology as accepted debt. A per-group score line in the eval output makes that debt visible on every run — if the morphology group scores poorly on real queries, that is the trigger to revisit relaxed matching for blocks; if it scores fine (semantic channel covering it), the debt stays parked with evidence.

---

## E8 — review findings (2026-07-20; E8.1 GATES THE IMPORT, do first)

### E8.1. Splitter must use line boundaries below min_chars before hard-cutting (moderate — import blocker)

`chunking.rs:107` filters boundaries to `(min_chars..=max_chars)`, discarding every line boundary below 1200 — a body of ~899-char lines splits mid-line/mid-word (empirically confirmed: 700/199/500-char fragments with clean boundaries available). Chunk boundaries are frozen format: fixing this later re-embeds nothing (hash covers the composition, not chunks) and would force another INPUT_FORMAT_VERSION bump + full paid re-embed. Fix now, before anything is embedded: when no boundary lands in `[min, max]`, take the **largest boundary ≤ max of the best available kind** before falling back to a hard cut. In the same commit fix the sibling defect: `natural_boundaries` resets `in_code_fence = false` for each `remaining` tail, inverting fence parity after an in-fence split (and blank lines inside fences classify as top-preference BlankLine). Golden tests: the 899-char-lines case splits at line boundaries; fence parity survives an in-fence split. NO version bump needed — nothing is embedded yet.

### E8.2. Gate the status path (efficiency — hot in daily use)

`server/src/ai.rs:460` `status_for` → `source_document_count` runs a full ungated `index_documents` (recursive CTE + full composition) just for `.len()`, and the settings panel polls it every 2 s — with settings open, a full-corpus scan every 2 seconds, defeating E5. Fix: read the cached `index_generations.source_documents` (or apply the same composite-cursor gate).

### E8.3. Don't fan out on transient batch errors

`embed.rs:732` calls `embed_individually` on ANY batch error; a 429/outage turns 1 failed call into 1+N sequential failing calls against a rate-limiter. Gate the fan-out on `provider_failure_is_terminal(&batch_error)` (isolate only when the error looks per-item); add the missing test that a transient batch error does NOT fan out. Also stop embedding a job's remaining chunks after one of its chunks failed (cost-only nit from the E3 review).

### E8.4. KNN pool: count unique blocks, not chunks

`retrieval.rs:60` fetches `limit*4` chunk matches then dedups — multi-chunk blocks shrink the effective RRF/rerank pool. Over-fetch (or dedup in SQL) so the post-dedup pool targets `limit*4` unique content UUIDs. Retrieval-side only, no re-embed.

### E8.5. Test gaps

Positive reconcile re-open: a local edit between ticks bumps `reconcile_scan_count` to 2 (the E5 gate's other direction).

### E8.6. Decision record — section heading does not cross parent boundaries

`store.rs:1177` matches headings among same-parent siblings only; a block nested in a list on a document page misses the top-level section heading. Default (recommended, pending user confirmation): KEEP this semantics — nested blocks already carry ancestor excerpts, and sibling-scope is simpler. Add a code comment documenting the choice either way. Do NOT change behavior without explicit approval.

### E8.7 (done, ef90ac4 — implemented reviewer-side). Fallback boundary: distance over kind

Follow-up to E8.1 from its verification: the fallback preferred boundary _kind_ over distance, so a BlankLine at 300 chars beat a plain Line at 1100 (emitting a 299-char chunk), and a lone fence opener produced a 3-char chunk. Fixed: the fallback takes the furthest natural boundary ≤ max regardless of kind (char_index is strictly increasing, so kinds cannot tie), and boundaries below `FALLBACK_MIN_CHUNK_CHARS` (64) are rejected in favor of a hard cut. The primary [min, max] window keeps kind-first preference unchanged. Fence-parity golden test extended to three consecutive in-fence splits. No version bump (nothing embedded yet).

## Out of scope (do not attempt)

- Overlap chunking, semantic/embedding-based chunkers, chunks spanning block boundaries.
- Client-side embeddings or any client AI.
- Per-layout (outline vs document) special cases — post-codec blocks are uniform by design.
- Lemmatization / morphology work (FTS-side concern, tracked separately).
- Incremental (per-op) reconcile — E5's seq-gate is enough at current corpus size.

## Operator runbook (for the human, after the plan lands)

1. Keep automatic indexing OFF (Settings toggle) → run the Logseq import.
2. `inspect-index`: check histogram, eyeball ~20 composed texts (journal block, deep-nested, code fence, document-page block).
3. Write the golden set from real memory of the corpus (20–30 queries).
4. Enable indexing, watch queue progress in Settings; expect < $1.
5. `eval` → tune (model / rerank / floor / thresholds) → re-run eval. Every change is now a measured delta.

## Final verification

Full cargo gate green; `vp check`/`vp test` green if TS touched; then report per-task commits, the INPUT_FORMAT_VERSION bump commit, and any deviations.
