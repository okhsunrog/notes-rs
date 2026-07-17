# notes-rs Implementation Roadmap

Status: active planning document. Created 2026-07-17.

This roadmap turns the accepted architecture records into an executable sequence. It does not
replace them:

- [`SYNC_ARCHITECTURE_PLAN.md`](SYNC_ARCHITECTURE_PLAN.md) defines the implemented source/sync/AI
  boundary;
- [`EDITOR_ARCHITECTURE.md`](EDITOR_ARCHITECTURE.md) defines Outline, Document, Reading, and the
  CodeMirror boundary;
- [`WORKSPACE_ARCHITECTURE.md`](WORKSPACE_ARCHITECTURE.md) defines panes and the Assistant dock;
- [`JOURNAL_ARCHITECTURE.md`](JOURNAL_ARCHITECTURE.md) defines Journal identity and the Logseq
  conversion boundary.

The purpose of this file is to record ordering, concrete deliverables, validation gates, commit
boundaries, and safe stopping points for long autonomous implementation sessions.

## 1. Verified baseline

At `e76b569` on 2026-07-17:

- the worktree is clean and `main` is 83 commits ahead of `origin/main`;
- typed pages/blocks, UUID/HLC/LWW operations, generated Specta bindings, TanStack Query, typed
  domain-event invalidation, local FTS, sync, server-owned AI, and Android scaffolding exist;
- persisted `PageView = Outline | Document | Reading` is still the current implementation;
- Journal, `PageKind`, `JournalDate`, and `workspace_uuid` are design-only;
- the authored-note editor is still a `textarea` and the inactive-block renderer is regex-based;
- Document is not yet a continuous `DocumentCodec`/CodeMirror session;
- the workspace still has one central page surface rather than a pane tree.

The green validation baseline is:

```text
vp check                                           pass
vp test                                            17 tests pass
cargo test --workspace --locked --all-features     76 tests pass
```

`vp build`, workspace Clippy, binding drift, desktop runtime inspection, and Android device checks
remain mandatory at the relevant implementation gates below.

## 2. Real Logseq migration target

The source graph is `/home/okhsunrog/Documents/notes`. It is always treated as read-only source
data, never as a test fixture directory that implementation code may modify.

The 2026-07-17 audit found:

- 400 normal page files and 724 journal files, of which 723 journals are non-empty;
- 1,124 Markdown files, approximately 1.38 MB total, with a largest file of approximately 49 KB;
- 16,902 parsed structural blocks, tabs in 581 files, and 1,033 files without a final newline;
- 134 task-marker blocks and 131 property lines, including 60 valid unique `id::` UUIDs;
- 117 files containing fenced code, primarily plain fences plus `sh` and `diff`;
- 366 wikilinks, one block reference, 72 Markdown images, tables, LaTeX, macros, headings,
  hashtags, logbooks, and ordinary Markdown links;
- 91 image assets (approximately 48 MB): all 60 distinct local references exist, ten references
  are remote HTTPS images, two are embedded base64 PNGs, and 31 local assets are currently
  unreferenced;
- five Excalidraw JSON files: four referenced drawings and one empty orphan. The referenced content
  is 578 free-draw elements and one text element, so a one-time SVG conversion is practical;
- `:file/name-format :triple-lowbar`, eight namespace page titles, and no decoded title collision;
- journals use strict `YYYY_MM_DD.md` filenames covering 2022-12-29 through 2026-07-17.

Private source files are never copied wholesale into the repository. Tests use minimal sanitized
fixtures extracted from representative constructs plus synthetic security and malformed-input
cases. A local-only corpus audit may read the real graph and compare counts, but CI never depends
on it.

## 3. Non-negotiable constraints

### Data and compatibility

1. The Logseq graph is never modified.
2. Logseq import is a dedicated pipeline, not `import_archive()` and not an archive compatibility
   mode.
3. The project is unreleased, so breaking source/wire/schema changes update the clean V001 baseline
   instead of adding legacy conversion code.
4. A development database may be recreated only after its disposable status is verified. Remote
   server databases are not reset or deployed from an autonomous coding session unless that action
   was explicitly included before the session.
5. Unsupported source is preserved or reported. It is never silently dropped or guessed into a
   different meaning.
6. Imported pages default to Outline. The importer never guesses that a long page should be a
   Document.

### AI cost safety

1. Import/parser/renderer tests run without a configured server and without provider credentials.
2. `notes-import` must not depend on `notes-ai`, `llm-relay`, Rig, or Tauri.
3. Both automatic embeddings and automatic entity extraction are disabled before any real imported
   corpus is synchronized to a server. Disabling embeddings alone is insufficient because entity
   extraction also makes paid provider calls.
4. First-start defaults and examples use background AI disabled. Enabling indexing is an explicit
   user action after reviewing imported-source counts.
5. Fake-provider tests fail if parsing, dry-run, local commit, or renderer validation performs any
   network/provider call.
6. Query rewriting may stay configured because it runs only for an explicit semantic query/chat;
   no such query is part of import validation.

The initial implementation uses the existing durable server AI switches rather than adding a
second import-specific state machine. If future unattended sync can bypass those switches, a
separate maintenance gate is added then, backed by a failing regression test rather than by
speculation.

### Engineering process

1. Preserve unrelated work and never rewrite user changes.
2. Commit only a coherent, formatted, tested boundary.
3. Regenerate bindings in the same commit as the Rust contract that changed them.
4. Do not retain old aliases, fallback paths, compatibility enums, or migrations for replaced
   pre-release contracts.
5. If a phase cannot reach its gate, stop at the previous green commit. Do not start another
   dependent phase on top of a broken boundary.
6. Independent work may be parallelized only across non-overlapping files/contracts. The primary
   integrator owns final validation and commits.
7. No push, release, production deployment, server reset, or mutation of the real Logseq graph is
   implied by an implementation session.

## 4. Dependency order

```text
AI cost guard and green baseline
             |
             v
PageLayout + local PagePresentation
             |
             v
workspace_uuid + PageKind::Journal + JournalDate
             |
             +---------------------------+
             |                           |
             v                           v
pure Logseq scanner/parser        shared Markdown dialect/renderer
             |                           |
             v                           v
dry-run + provenance              attachments/images/math/code/tasks
             |                           |
             +-------------+-------------+
                           v
                 atomic empty-workspace import
                           |
                           v
                import UI + real corpus audit
                           |
          +----------------+----------------+
          v                                 v
 CodeMirror Outline spike          pane-aware workspace/Assistant
          |                                 |
          +----------------+----------------+
                           v
              PageSession + linked Reading pane
                           |
                           v
                continuous DocumentCodec editor
```

The importer must not commit Journal pages before Journal identity exists. The renderer can be
developed in parallel with the pure parser once the Markdown/attachment URI contract is fixed.
Workspace panes are not a prerequisite for correct import; they are a prerequisite for linked live
preview and the final Reading UX.

## 5. Phase A: AI safety and durable layout boundary

### A1. Disable unattended paid work by default

Deliverables:

- default `automatic_embeddings` and `entity_extraction` to `false` in server and AI-store
  initialization;
- update the example/bootstrap configuration and deployment defaults consistently;
- preserve the existing Settings toggles and index-status UI;
- verify the running server settings separately before any real corpus sync;
- add tests proving a disabled worker cannot call its provider after queue wake or reconciliation.

Suggested commit:

```text
chore(ai): default background indexing to off
```

### A2. Replace durable `PageView` with `PageLayout`

Deliverables:

- `PageLayout = Outline | Document` in Rust, SQLite, operations, snapshots, archives, sync, RPC,
  generated bindings, and frontend types;
- remove `Reading` from durable state and remove every `PageView`, `PageSetView`, `default_view`,
  `view_hlc`, and serialized `reading` path;
- add pane/component-local `PagePresentation = Editing | Reading` without exposing it through RPC;
- keep Reading reachable for Document through a local command while the full pane system is still
  pending;
- bump the operation/snapshot/archive contracts where required and recreate disposable development
  databases from the updated V001 baseline.

Suggested commit:

```text
refactor(core): separate page layout from presentation
```

Gate:

```sh
rg -n 'PageView|PageSetView|default_view|view_hlc' crates src-tauri server src
vp run bindings:generate
vp check
vp test
cargo test --workspace --locked --all-features
```

The search must return no compatibility implementation. Documentation references describing the
old current state are updated rather than hidden behind aliases.

## 6. Phase B: Journal and import-ready source model

### B1. Journal identity

Deliverables:

- durable `workspace_uuid` included in snapshot/bootstrap/archive identity;
- validated `JournalDate` newtype using strict civil ISO dates;
- closed `PageKind = Note | Journal { date }` Specta type;
- normalized `journal_pages` baseline table and immutable kind/date apply invariants;
- deterministic UUIDv5 journal page identity from `(workspace_uuid, JournalDate)`;
- UUIDv7 remains the identity for ordinary Notes, Blocks, operations, and actions;
- convergence tests for two offline replicas creating/capturing the same day.

Suggested commit:

```text
feat(core): add deterministic journal page identity
```

### B2. Journal services and minimum UI

Deliverables:

- atomic `ensure_journal`, `get_journal`, `list_journals`, and `append_to_journal` services;
- typed commands/events and generated bindings;
- normal Notes listing excludes Journal pages by typed filter while FTS/backlinks include them;
- Today and date navigation open an ordinary page editor;
- browsing an empty date creates no source data until first edit/capture.

Full calendar/timeline, templates, journal graph filters, and temporal semantic search are later
features. They do not block import.

Suggested commit:

```text
feat(journal): add daily pages and quick capture
```

### B3. Import-critical typed semantics

Before converting the corpus, add only semantics that would otherwise be lost:

- a closed `TaskState` capable of representing the Logseq states present in the corpus;
- explicit task-state update behavior and rendering;
- a stable internal attachment reference syntax/resolver;
- a documented decision for `id::`: preserve its UUID as block identity and omit only that identity
  property from visible Markdown;
- preserve other page/block properties, logbooks, macros, malformed tables, and unknown constructs
  as recoverable source plus diagnostics until their own typed model is accepted.

Do not invent a full arbitrary property/query system inside the importer.

Suggested commit:

```text
feat(core): preserve imported task and attachment semantics
```

Implemented as `def5b97` for the typed task workflow. The stable
`notes-attachment:<uuid>` renderer contract and blocked-by-default resolution policy landed earlier
in F2 (`87ac7d0`); authorized blob resolution and imported attachment ownership remain D1 work.

## 7. Phase C: Pure Logseq import pipeline

### C1. Pure crate, contracts, and deterministic scan

Create `crates/notes-import` as a pure Rust crate. It owns source-format concerns and produces a
validated intermediate plan; it does not own SQLite, Tauri UI, sync, or AI.

Core types:

```text
LogseqSource
PreparedImport
ImportManifest
ImportPage / ImportBlock / ImportAttachment
ImportDiagnostic { severity, code, path, range, remediation }
ImportReport
ImportProvenance
```

Diagnostics never log note bodies, macro arguments, credentials, or arbitrary huge lines.

Scanner deliverables:

- validate a selected graph root and refuse symlink/path traversal escapes;
- parse only whitelisted literal Logseq config keys without evaluating EDN functions;
- respect configured pages/journals/assets directories and `triple-lowbar` filename decoding;
- ignore recycle, backup, version-history, `.obsidian`, and unrelated hidden data;
- sort every input deterministically and hash every relevant file into an immutable manifest;
- rehash at commit to detect source changes between dry-run and apply.

### C2. Loss-aware structural parser

Deliverables:

- distinguish structural bullets from bullet-looking text inside fenced code;
- support tabs, mixed indentation diagnostics, continuation lines, preamble content, missing final
  newlines, long physical lines, headings, tasks, properties, logbooks, tables, and macros;
- convert indentation to parent relationships and sibling order;
- strip the structural leading bullet without changing nested Markdown inside the block;
- use Logseq/mldoc behavior as a development oracle for golden fixtures, not as a runtime
  dependency;
- preserve unsupported source verbatim and attach a typed warning.

### C3. Stable identities and references

Mapping policy:

- Journal pages use their deterministic workspace/date UUID;
- normal pages use UUIDv5 from import namespace plus canonical decoded source identity;
- valid collision-free explicit block `id::` UUIDs are preserved;
- blocks without source UUIDs use deterministic import UUIDs and provenance mapping;
- `OrderKey::from_ordinal` is assigned independently per sibling list;
- wikilinks remain source Markdown while date aliases and page identities resolve through the
  prepared map;
- unresolved wikilinks/block refs remain raw and are reported rather than deleted.

The first importer supports an empty notes-rs workspace or an exact already-committed manifest
no-op. It refuses arbitrary merge/update of a changed Logseq graph. Incremental matching is a later
feature and must not be approximated with mutable line numbers.

Suggested commits:

```text
feat(import): add deterministic Logseq scanner
feat(import): parse Logseq pages journals and blocks
feat(import): resolve identities references and provenance
```

## 8. Phase D: Assets and drawings

### Attachments

- resolve relative assets without allowing path escape;
- report missing source references if found, while the audited corpus currently has none; do not
  import the 31 unreferenced assets by default;
- externalize base64 images into content-addressed blobs;
- preserve remote images as blocked-by-default links that require a user privacy decision;
- stage immutable blobs before the database transaction; failed DB commit may leave only
  garbage-collectable unreferenced blobs, never a committed missing attachment;
- use Block ownership for inline assets and Page ownership for preamble assets.

### Excalidraw

Native drawing editing/rendering is out of scope. For the current graph:

- retain the original Excalidraw JSON in the import provenance/staging output;
- convert the four referenced version-2 drawings to ordinary SVG images using a pinned one-time
  conversion helper based on Excalidraw's official export API;
- import the SVG as a normal attachment and rewrite the visible drawing reference to that image;
- treat conversion failure as a warning with the original reference preserved;
- do not ship the Excalidraw editor or add a permanent drawing domain to notes-rs.

Suggested commit:

```text
feat(import): stage assets and convert legacy drawings
```

## 9. Phase E: Atomic apply, dry-run UI, and corpus validation

### Bulk apply

Creating approximately 14,000 blocks through the current one-operation-at-a-time structure
reconciliation is not acceptable. Add a local bulk-operation boundary that:

- validates the entire prepared import before mutation;
- emits normal versioned source operations and sync outbox records;
- applies the batch in one SQLite transaction;
- defers structure reconciliation and reference resolution until the batch boundary;
- creates one recovery backup but does not store a multi-megabyte interactive undo action;
- records import run/item provenance;
- leaves history in a coherent state and makes exact-manifest rerun a no-op.

Do not use snapshot import as a shortcut if it would bypass the operation/outbox invariant.

### Tauri/UI flow

Desktop first:

1. choose a Logseq graph folder;
2. scan and show progress;
3. display counts, missing assets, unresolved refs, unsupported constructs, and blockers;
4. require an empty workspace and explicit confirmation;
5. revalidate the manifest and commit;
6. show an import receipt and keep background AI paused/off;
7. offer to open the imported Today page or first selected page.

Android does not need direct folder import. A desktop import synchronizes ordinary pages, blocks,
and attachments to Android later.

### Validation

- golden parser/AST fixtures;
- deterministic manifest and UUID snapshots;
- property/no-panic tests for arbitrary UTF-8 and malformed indentation/fences;
- missing/path-traversal/symlink/TOCTOU tests;
- failpoints before asset install, during DB apply, and after apply;
- two-replica convergence after imported operations synchronize;
- exact rerun no-op and changed-source refusal;
- fake AI providers observe zero calls;
- local-only full corpus dry-run and import into a disposable database;
- page/journal/block/attachment counts and a sample of deeply nested pages match the report;
- the real source graph hash and file mtimes are unchanged after testing.

Suggested commits:

```text
perf(core): apply external import operations in one transaction
feat(tauri): add Logseq import dry run and commit flow
test(import): validate the real corpus without mutating it
```

## 10. Phase F: Shared Markdown dialect and Reading renderer

### One renderer

Replace the regex authored-note renderer and the separate AI Markdown path with one AST-based
`MarkdownRenderer` used by:

- inactive Outline blocks;
- Document Reading;
- linked preview panes;
- AI responses;
- block/page hover previews in an explicitly limited context.

Recommended stack:

- CommonMark through unified/remark;
- `remark-gfm`;
- notes-rs extensions for wikilinks, block refs, attachment references, and supported macros;
- `remark-math` plus locally bundled KaTeX with `trust: false`;
- fine-grained Shiki languages/themes with plain-text fallback;
- `rehype-sanitize` and no raw HTML execution;
- Mermaid as a later isolated renderer, strict and sanitized, because it is absent from the current
  corpus and has the largest weight/security surface.

### Initial feature order

1. CommonMark links, emphasis, inline code, paragraphs, headings, quotes, and fences;
2. wikilinks, block references, safe external-link policy, and navigation intents;
3. GFM tables, strikethrough, and task lists;
4. local attachments/images, size metadata, base64 externalization, and remote-image privacy UI;
5. task state and collapsed property/logbook presentation;
6. KaTeX;
7. syntax-highlighted code with copy affordance;
8. supported video/macro link cards;
9. Mermaid diagrams.

Malformed or unsupported input falls back to visible/recoverable source. Rendering never rewrites
stored Markdown.

### Security and performance

- no `file://` access and no arbitrary Tauri asset scope;
- internal attachment URLs resolve through a typed backend policy;
- external navigation uses a checked URL intent rather than WebView navigation;
- remote images are blocked by default;
- no CDN, runtime grammar download, or remote font dependency;
- cache parsing/rendering by source hash plus dialect version;
- mobile tables scroll horizontally and images decode lazily with size limits;
- test malicious URLs, HTML, SVG, KaTeX, and Mermaid payloads;
- retain the 1-2 MB renderer/editor performance gate even though the current corpus pages are
  smaller.

Suggested commits:

```text
feat(markdown): add a shared semantic renderer
feat(markdown): render attachments math and highlighted code
feat(markdown): add safe Mermaid diagrams
```

The F2 renderer boundary accepts only a backend-authorized `MarkdownImageResolver`. Supplying that
resolver with imported attachment metadata/blob URLs belongs to D1; until then attachment syntax
renders an intentional safe placeholder rather than exposing an arbitrary filesystem path.

## 11. Phase G: CodeMirror, Document, and workspace completion

These are later dependent stages, not part of the import foundation:

1. CodeMirror Outline spike: one editor for the active block, existing autocomplete/autosave,
   dirty-draft conflict handling, parity with the semantic renderer, and real Android IME tests.
2. Source/Live Preview configuration of the same EditorState; widgets reveal source under the
   caret and render only outside the active range.
3. Window-local pane tree, typed `open_target`, adjacent navigation, graph pane, responsive compact
   projection, and two-pane UI limit.
4. Assistant `Hidden | Rail | Open` controller whose input/stream/session survives collapse and
   compact projection.
5. One writable `PageSession` per page/window and a linked Reading pane consuming its exact
   in-memory draft.
6. Versioned `DocumentCodec`, continuous CodeMirror buffer, block source maps, deterministic
   split/merge/identity fixtures, remote transactions, and undo ownership.

Each is a separate green migration. CodeMirror is not introduced merely to render imported pages.

## 12. First 5-6 hour autonomous execution slice

The first long session prioritizes source-model correctness and executable import progress over
workspace polish.

### Target order

```text
0:00-0:25  preflight, AI-off defaults, tests
0:25-1:45  PageView -> PageLayout and local Reading presentation
1:45-3:15  workspace_uuid, PageKind, JournalDate, journal schema/convergence
2:00-4:15  parallel pure Logseq scanner/parser/fixtures in new files
2:00-4:15  parallel shared renderer foundation in isolated frontend files
4:15-5:15  integrate whichever parallel boundary is complete; bindings and UI types
5:15-6:00  full gates, runtime inspection where applicable, status/roadmap update
```

This is an ambition order, not permission to commit incomplete work. Expected minimum complete
result:

- paid background AI defaults are off;
- durable Reading has been removed correctly;
- Journal identity exists with convergence tests;
- at least the deterministic Logseq scan/dry-run parser boundary is implemented and tested.

Stretch result:

- shared CommonMark/GFM renderer replaces regex and AI duplication;
- importer reaches an empty-workspace atomic apply into a disposable database.

If the model/schema migration consumes the full session, stop after its full green gate. Do not
claim that import or renderer work is complete merely because scaffolding files exist.

## 13. Validation and commit policy

After every logical Rust/domain stage:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked --all-features
vp run bindings:generate
git diff --exit-code -- src/lib/bindings.ts  # after committing generated contract
```

After every frontend stage:

```sh
vp check
vp test
vp build
```

After UI-visible stages:

```sh
vp run desktop:dev
vp exec tauri-mcp driver-session start --port 9223
vp exec tauri-mcp webview-dom-snapshot --type accessibility
vp exec tauri-mcp webview-screenshot --file <checkpoint>.png
```

Inspect wide and compact sizes, light and dark themes, keyboard focus, and absence of runtime
errors. Real Android Gboard/IME and safe-area validation remain a named manual gate when the user
has a device available.

Before each commit:

- inspect `git diff` and `git status`;
- include only the logical boundary;
- do not amend or squash earlier user-visible checkpoints unless asked;
- record unfinished follow-up honestly in this roadmap.

Before ending an autonomous session, run the full applicable gate, leave a clean worktree or a
clearly reported uncommitted blocker, and report exact commits, tests, completed items, and next
starting point.

## 14. Defaults for the first session

Unless explicitly changed before execution, use these decisions:

- work on current `main`, create logical local commits, and do not push;
- do not deploy or reset the remote server;
- recreate only disposable local development databases required by the new V001 baseline;
- disable both automatic embeddings and entity extraction before any real imported content is
  allowed to sync;
- import the real graph only into a temporary/disposable local database for validation;
- support empty-workspace import and exact-manifest no-op, not arbitrary merge;
- preserve unsupported properties/macros/logbooks as source plus diagnostics;
- add typed task state now because otherwise the real corpus loses meaning;
- convert the four referenced Excalidraw files once to SVG images without shipping drawing support;
- block remote images by default;
- prioritize parser/data fidelity over Mermaid and visual editor polish;
- stop on the last fully green commit if time or context becomes constrained.

## 15. Development timeline and effort ledger

This section is updated during implementation, not reconstructed from memory at the end. All
timestamps use ISO 8601 with the local `Europe/Moscow` offset.

For each feature boundary record:

- `started_at`: immediately before the first implementation edit;
- `finished_at`: after the complete validation gate and logical commit;
- `elapsed`: wall-clock difference between start and finish, including builds, tests, debugging,
  review, and integration;
- `agent_time`: sum of active intervals across the primary agent and parallel agents. This may be
  greater than elapsed time when independent work ran concurrently;
- exact commit, validation commands/results, and any scope moved to a later phase.

An interruption longer than 15 minutes is recorded as a pause interval and excluded from
`agent_time`, but not hidden from elapsed time. A failed or abandoned attempt remains in the log
with status `blocked` or `superseded`; its time is not reassigned to the successful replacement.
Planning estimates stay separate from actual duration.

### Feature ledger

| ID  | Feature boundary                             | Status   | Planned elapsed | Started at                | Finished at               | Elapsed  | Agent time | Commit                           | Validation                                       |
| --- | -------------------------------------------- | -------- | --------------- | ------------------------- | ------------------------- | -------- | ---------- | -------------------------------- | ------------------------------------------------ |
| R0  | Roadmap, corpus, and architecture audit      | complete | 00:40           | 2026-07-17T17:14:04+03:00 | 2026-07-17T17:41:01+03:00 | 00:26:57 | >=00:26:57 | `ca35c95`                        | `vp check`, 17 frontend and 76 Rust tests        |
| A1  | Background AI defaults off                   | complete | 00:25           | 2026-07-17T17:42:27+03:00 | 2026-07-17T17:44:51+03:00 | 00:02:24 | 00:02:24   | `37d82ca`, cloud-forge `c46bd2f` | 19 AI and 13 server tests; scoped Clippy         |
| A2  | `PageView` to `PageLayout`                   | complete | 01:20           | 2026-07-17T17:45:44+03:00 | 2026-07-17T17:54:47+03:00 | 00:09:03 | 00:09:03   | `6d66fb1`                        | 29 frontend; 89 Rust; Clippy; build              |
| B1  | Workspace and Journal identity               | complete | 01:30           | 2026-07-17T17:56:37+03:00 | 2026-07-17T18:24:14+03:00 | 00:27:37 | 00:32:02   | `8008f5e`                        | 113 Rust; 35 frontend; Clippy; build; bindings   |
| B2  | Journal services and minimum UI              | complete | 01:15           | 2026-07-17T18:27:09+03:00 | 2026-07-17T18:53:14+03:00 | 00:26:05 | 00:46:32   | `f0698ee`                        | 131 Rust; 61 frontend; Clippy; bindings; WebView |
| B3  | Typed task workflow state                    | complete | 01:00           | 2026-07-17T18:54:16+03:00 | 2026-07-17T19:10:56+03:00 | 00:16:40 | 00:19:39   | `def5b97`                        | 53 core; 84 frontend; scoped Clippy; build       |
| C1  | Logseq scanner/config/manifest               | complete | 01:15           | 2026-07-17T17:45:43+03:00 | 2026-07-17T18:25:57+03:00 | 00:40:14 | 00:22:43   | `64a612e`                        | 24 tests; strict Clippy; real corpus exact smoke |
| C2  | Logseq structural parser and fixtures        | complete | 02:00           | 2026-07-17T18:27:42+03:00 | 2026-07-17T19:01:19+03:00 | 00:33:37 | 00:32:33   | `5e33209`                        | 46 tests; Clippy; 1,124-file lossless smoke      |
| C3  | Import identity/reference/provenance mapping | complete | 01:30           | 2026-07-17T19:02:09+03:00 | 2026-07-17T19:33:02+03:00 | 00:30:53 | 00:30:53   | `69e39eb`                        | 67 crate; 167 workspace; Clippy; corpus dry-run  |
| D1  | Asset staging and attachment rewrite         | planned  | 01:30           | —                         | —                         | —        | —          | —                                | —                                                |
| D2  | One-time Excalidraw to SVG conversion        | planned  | 01:00           | —                         | —                         | —        | —          | —                                | —                                                |
| E1  | Atomic bulk apply and convergence            | planned  | 02:00           | —                         | —                         | —        | —          | —                                | —                                                |
| E2  | Desktop dry-run/commit UI                    | planned  | 01:30           | —                         | —                         | —        | —          | —                                | —                                                |
| E3  | Disposable full-corpus validation            | planned  | 01:00           | —                         | —                         | —        | —          | —                                | —                                                |
| F1  | Shared CommonMark/GFM semantic renderer      | complete | 02:00           | 2026-07-17T17:45:48+03:00 | 2026-07-17T18:15:21+03:00 | 00:29:33 | 00:12:23   | `bc31b52`                        | `vp check`; 35 tests; production build           |
| F2  | Image policy, KaTeX, and highlighted code    | complete | 02:00           | 2026-07-17T18:27:47+03:00 | 2026-07-17T18:58:39+03:00 | 00:30:52 | 00:29:49   | `87ac7d0`                        | 78 frontend; build; 1.917 MB main JS             |
| F3  | Safe Mermaid rendering                       | planned  | 01:15           | —                         | —                         | —        | —          | —                                | —                                                |
| G1  | CodeMirror active Outline block              | complete | 02:30           | 2026-07-17T19:12:31+03:00 | 2026-07-17T19:24:13+03:00 | 00:11:42 | 00:11:42   | `2a24308`                        | 92 frontend; live WebView; build 2.419 MB        |
| G2  | Workspace panes and Assistant controller     | planned  | 03:00           | —                         | —                         | —        | —          | —                                | —                                                |
| G3  | PageSession and linked live Reading pane     | planned  | 02:00           | —                         | —                         | —        | —          | —                                | —                                                |
| G4  | Continuous DocumentCodec editor              | planned  | 04:00+          | —                         | —                         | —        | —          | —                                | —                                                |

The estimates are scheduling aids, not deadlines. Any row is split into smaller logical rows if it
cannot be completed and validated as one commit.

### Work intervals

Use this table when a feature spans multiple sessions, pauses, or parallel assignments. `Ended at`
is recorded as soon as an interval stops; the feature-ledger totals are calculated from these rows.

| Feature ID | Actor  | Started at                | Ended at                  | Active duration | Result/notes                                                                                             |
| ---------- | ------ | ------------------------- | ------------------------- | --------------- | -------------------------------------------------------------------------------------------------------- |
| R0         | root   | 2026-07-17T17:14:04+03:00 | 2026-07-17T17:41:01+03:00 | 00:26:57        | Baseline verification, corpus audits, and roadmap design; parallel audit intervals were not instrumented |
| A1         | root   | 2026-07-17T17:42:27+03:00 | 2026-07-17T17:44:51+03:00 | 00:02:24        | Disabled unattended embeddings and entity extraction defaults; updated deployment bootstrap              |
| A2         | root   | 2026-07-17T17:45:44+03:00 | 2026-07-17T17:54:47+03:00 | 00:09:03        | Replaced durable PageView with PageLayout and local Reading presentation; binding drift remained clean   |
| B1         | root   | 2026-07-17T17:56:37+03:00 | 2026-07-17T18:24:14+03:00 | 00:27:37        | Workspace identity, typed Journal domain, sync/archive invariants, and regression tests                  |
| B1         | review | 2026-07-17T18:00:45+03:00 | 2026-07-17T18:05:10+03:00 | 00:04:25        | Found workspace bootstrap, operation scoping, archive, UUID reservation, and Journal ensure races        |
| B2         | root   | 2026-07-17T18:27:09+03:00 | 2026-07-17T18:53:14+03:00 | 00:26:05        | Integrated atomic Journal services, ephemeral missing-day UI, AI/graph filters, bindings, and commit     |
| B2         | agent  | 2026-07-17T18:27:26+03:00 | 2026-07-17T18:36:10+03:00 | 00:08:44        | Implemented bounded Journal queries, atomic capture, O(1) append ordering, events, and backend tests     |
| B2         | review | 2026-07-17T18:42:39+03:00 | 2026-07-17T18:51:31+03:00 | 00:08:52        | Fixed stale navigation, per-date draft reuse, cache invalidation, busy UX, and mobile autofocus races    |
| B2         | root   | 2026-07-17T19:12:56+03:00 | 2026-07-17T19:15:47+03:00 | 00:02:51        | Live WebView proved empty-date browsing is non-mutating, first capture atomic, and next day ephemeral    |
| B3         | agent  | 2026-07-17T18:54:16+03:00 | 2026-07-17T19:08:24+03:00 | 00:14:08        | Added typed task state across SQLite, LWW operations, history/archive, RPC, and semantic UI              |
| B3         | root   | 2026-07-17T19:05:25+03:00 | 2026-07-17T19:10:56+03:00 | 00:05:31        | Fixed strict wire decoding, reviewed read-only/empty-task UX, ran integration gates, and committed       |
| C1         | agent  | 2026-07-17T17:45:43+03:00 | 2026-07-17T17:59:41+03:00 | 00:13:58        | Pure scanner/config/manifest foundation and read-only real-corpus smoke                                  |
| C1         | agent  | 2026-07-17T18:13:08+03:00 | 2026-07-17T18:20:10+03:00 | 00:07:02        | Resource limits and explicit recovery-directory exclusion hardening                                      |
| C1         | root   | 2026-07-17T18:24:14+03:00 | 2026-07-17T18:25:57+03:00 | 00:01:43        | Reviewed scanner invariants, reran focused gates, and committed the isolated crate                       |
| C2         | agent  | 2026-07-17T18:27:42+03:00 | 2026-07-17T18:41:27+03:00 | 00:13:45        | Added loss-aware structural AST, Journal source classification, typed constructs, fixtures, and tests    |
| C2         | review | 2026-07-17T18:42:31+03:00 | 2026-07-17T19:00:07+03:00 | 00:17:36        | Fixed CommonMark fences, Unicode complexity, property ranges, BOM/extensions, and ran full-corpus smoke  |
| C2         | root   | 2026-07-17T19:00:07+03:00 | 2026-07-17T19:01:19+03:00 | 00:01:12        | Reviewed source preservation, reran strict crate gates, removed fixture whitespace, and committed C2     |
| C3         | agent  | 2026-07-17T19:02:09+03:00 | 2026-07-17T19:31:55+03:00 | 00:29:46        | Added stable identities, task/reference maps, provenance, preparation limits, and real-corpus dry-run    |
| C3         | root   | 2026-07-17T19:31:55+03:00 | 2026-07-17T19:33:02+03:00 | 00:01:07        | Reviewed contracts, ran strict crate gates, and committed the isolated pure import boundary              |
| F1         | agent  | 2026-07-17T17:45:48+03:00 | 2026-07-17T17:51:51+03:00 | 00:06:03        | CommonMark/GFM renderer, typed URL policy, and focused security tests                                    |
| F1         | agent  | 2026-07-17T17:55:21+03:00 | 2026-07-17T18:00:50+03:00 | 00:05:29        | Integrated shared renderer into authored blocks, AI responses, and typed navigation                      |
| F1         | root   | 2026-07-17T18:14:30+03:00 | 2026-07-17T18:15:21+03:00 | 00:00:51        | Security review, encoded-control hardening, full frontend gate, and commit                               |
| F2         | agent  | 2026-07-17T18:27:47+03:00 | 2026-07-17T18:44:12+03:00 | 00:16:25        | Added local KaTeX/Shiki, typed attachment image policy, code copy, tables, and security tests            |
| F2         | review | 2026-07-17T18:45:13+03:00 | 2026-07-17T18:57:58+03:00 | 00:12:45        | Hardened image origins, HTML shape, Shiki workload, KaTeX aggregate limits, and package deduplication    |
| F2         | root   | 2026-07-17T18:58:00+03:00 | 2026-07-17T18:58:39+03:00 | 00:00:39        | Reviewed security boundaries, staged isolated renderer files, and committed the green production build   |
| G1         | agent  | 2026-07-17T19:12:31+03:00 | 2026-07-17T19:23:17+03:00 | 00:10:46        | Replaced active textarea with CM6, preserved outliner/IME behavior, added model tests, and inspected live |
| G1         | root   | 2026-07-17T19:23:17+03:00 | 2026-07-17T19:24:13+03:00 | 00:00:56        | Reviewed editor ownership and shortcuts, reran 92 frontend tests, accepted visible 2 MB bundle warning    |

### Session summaries

At the end of every autonomous session append one row. The summary references feature IDs rather
than replacing their detailed timing.

| Session     | Started at                | Finished at               | Elapsed  | Completed feature IDs | Commits   | Final gate                                | Next start                |
| ----------- | ------------------------- | ------------------------- | -------- | --------------------- | --------- | ----------------------------------------- | ------------------------- |
| S0 planning | 2026-07-17T17:14:04+03:00 | 2026-07-17T17:41:01+03:00 | 00:26:57 | R0                    | `ca35c95` | `vp check`, 17 frontend and 76 Rust tests | A1 background AI defaults |
