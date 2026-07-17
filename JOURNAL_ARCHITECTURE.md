# Journal Domain, UI, and Import Architecture

Status: accepted on 2026-07-17. Implementation is pending.

This decision defines daily journals as first-class notes-rs domain content and defines the
loss-aware conversion boundary for the existing Logseq graph. It intentionally adopts the useful
semantic identity of a journal day without copying Logseq's filename conventions or making Journal
a separate editor.

## 1. Core decision

A journal entry is an ordinary typed page with a special immutable identity:

```text
PageKind =
  Note
  | Journal { date: JournalDate }

Page {
  uuid,
  kind,
  layout: PageLayout,
  blocks,
  ...
}
```

`JournalDate` is a strong validated civil-calendar newtype, serialized canonically as ISO
`YYYY-MM-DD`. Domain APIs do not accept an unchecked `String`. It is not a UTC timestamp and is
never inferred by the server from an operation timestamp.

Journal reuses every normal page capability:

- the same page/block tree and UUID references;
- the same Outline and Document layouts;
- the same editor, attachments, local FTS, backlinks, graph, history, archive, and sync machinery;
- the same server-side retrieval pipeline, with journal date available as metadata/filter context.

The default layout for a new journal page is Outline, but layout is orthogonal to page kind. A user
may choose Document for a particular day without converting its content or identity. Journal is not
a special Outline mode and does not add another editor engine.

## 2. Typed storage model

Rust exposes `PageKind` as a closed tagged enum through Specta. It must not be represented as an
unvalidated `kind: string` plus an optional date.

SQLite stores the subtype in a normalized relation rather than nullable journal fields on every
page:

```sql
CREATE TABLE journal_pages (
  page_uuid BLOB PRIMARY KEY REFERENCES pages(uuid) ON DELETE CASCADE,
  journal_date TEXT NOT NULL UNIQUE
);
```

Repository row mapping joins this table into `PageKind::Journal`. Rust validates strict ISO input
and real calendar dates; SQLite text ordering gives chronological ordering. The baseline may add a
shape check, but SQLite string checks are not the semantic validator.

Journal date is immutable. Renaming a normal page does not turn it into a journal, changing a
journal title does not move it to another day, and `PageSetTitle` rejects a Journal target. Moving
content between days is an explicit block move/copy action.

The visible journal heading is localized from `JournalDate`. A stable canonical alias such as
`2026-07-17` supports links and export, but localized display text is not identity. Plain page title
normalization must not decide whether a page is a journal.

`PageKind` and journal date are included in Page creation, snapshots, archives, query DTOs, and
sync validation. An operation that reuses one page UUID with a conflicting kind/date is invalid.

## 3. Offline convergence and identity

Two disconnected replicas may both open or capture into Today before synchronizing. A local UNIQUE
constraint alone cannot make two independently generated page UUIDs converge.

Each workspace therefore receives a durable `workspace_uuid` when it is created. Bootstrap and
snapshot import preserve it, and the existing non-empty-workspace mismatch rule rejects an implicit
merge of different workspace UUIDs.

Journal page UUID is deterministically derived from `(workspace_uuid, JournalDate)` with a
namespaced UUID (UUIDv5). This is a deliberate exception to UUIDv7:

- user-created Notes, Blocks, actions, and operations continue to use UUIDv7;
- Journal has a natural identity and must be independently creatable offline;
- chronological ordering comes from `JournalDate`, not UUID ordering.

Both replicas therefore emit `PageCreate` for the same page UUID and deterministic apply remains
idempotent. `ensure_journal(date)` does not create a persisted empty block merely to satisfy the UI;
the first actual edit/capture creates ordinary UUIDv7 blocks. Concurrent captures become distinct
blocks on the same journal page instead of duplicate pages or duplicate placeholders.

An imported Logseq journal page UUID is recorded in import provenance and mapped to the notes-rs
deterministic journal UUID. Source block UUIDs may still be preserved when valid and collision-free.

## 4. Commands and queries

The application boundary is intent-oriented:

```text
ensure_journal(date) -> JournalPage
get_journal(date) -> optional JournalPage
list_journals(before_date, limit) -> JournalPage[]
append_to_journal(date, block_input) -> Block
```

`ensure_journal` is atomic and idempotent. Merely browsing an empty calendar date need not persist a
page; creation occurs on first edit/capture or an explicit create action.

`append_to_journal` is the Quick Capture boundary. It ensures the day and atomically appends one
top-level block/history action rather than making the frontend orchestrate page and block RPCs.

The normal Notes list excludes Journal pages by default. Search, links, backlinks, and explicit
all-page queries include both kinds unless their typed filter says otherwise.

## 5. Journal product surfaces

Journal has navigation surfaces, not a special content format:

- a prominent `Today` action in the navigation sidebar;
- a `Journal` action opening a calendar/timeline surface;
- previous/next day and calendar jump actions;
- Quick Capture to today or an explicitly selected date;
- a normal page pane for editing one day.

The first implementation should open one day in the ordinary Page pane. This is clear on desktop
and mobile and immediately reuses the complete editor. Visiting a missing day shows an ephemeral
empty state and creates it only when the user writes.

A later `JournalTimeline` pane may render a reverse-chronological, virtualized sequence of days.
It is a projection over Journal pages, not a durable page and not a `PageLayout`. In a timeline,
rendered days stay lightweight and only the currently edited block mounts CodeMirror. Clicking a
date heading opens that ordinary Journal page in the current or adjacent pane.

Journal naturally participates in the general workspace architecture:

```text
Today's journal | referenced project page
Journal timeline | selected day
Journal day      | AI/graph/another note
```

Normal click and `Shift+click` use the same `OpenDisposition` as every other surface. Compact mode
shows one pane at a time without destroying the other pane or draft.

Date formatting, week start, and the device's definition of Today are display/input preferences.
The server never creates journal pages on a timer. A later device-local setting may choose System or
an explicit IANA timezone/rollover hour; every command still sends an explicit `JournalDate`.

Daily templates are a later content feature applied atomically on first creation. No template is
reapplied when a page already exists.

## 6. Search, graph, and AI behavior

Journal blocks are ordinary searchable source content. Local FTS, backlinks, semantic retrieval,
reranking, and chat include them. Search APIs may add typed date ranges and `PageKind` filters.

Hundreds of daily pages can overwhelm a knowledge graph and the normal Notes sidebar. Therefore:

- Notes navigation lists `PageKind::Note` by default;
- Journal navigation owns chronological listing;
- the workspace graph hides Journal pages by default and provides an explicit include/filter toggle;
- backlinks never hide a matching Journal merely because graph visualization filters it;
- retrieval chunks carry optional journal date metadata for later temporal queries.

Extracted AI entities remain server-derived data and do not change Journal identity.

## 7. Logseq model and conversion boundary

Logseq also represents a journal as a page with a dedicated day attribute and renders journal pages
through its ordinary page component. notes-rs preserves that useful semantic distinction but not
Logseq's storage conventions.

The importer treats these as separate concerns:

```text
Logseq journals directory + filename -> JournalDate identity
Logseq structural bullets/indentation -> notes-rs block tree/order
Logseq block body                   -> block Markdown
Logseq metadata/properties          -> typed mapping or recoverable source/provenance
Logseq asset path                   -> notes-rs Attachment/blob
```

The configured journals directory plus a strictly valid journal filename is authoritative. A
date-looking file in `pages/` is not silently reclassified as Journal. Duplicate dates, invalid
filenames, and conflicting UUIDs are errors in the dry-run report before mutation.

Logseq's leading `-` is outliner structure, not necessarily semantic Markdown list style. Imported
outline items therefore default to `BlockStyle::Paragraph` on an Outline page after stripping the
structural marker. Indentation becomes `parent_uuid` and `OrderKey`. Only a semantic list inside a
block body becomes Bullet/Numbered; recognized task state may become Task when the task-state model
exists.

Continuation lines, fenced code, properties, embeds, and logbooks belong to the surrounding Logseq
block and must not be split into one notes-rs block per physical line.

## 8. Loss-aware Logseq import pipeline

Logseq import is a dedicated staged importer, never workspace archive restore:

1. Read `logseq/config.edn` and resolve configured live pages, journals, assets, filename encoding,
   date formats, and ignored paths.
2. Enumerate live `pages/`, `journals/`, `assets/`, and supported draws. Ignore recycle, backup, and
   version-history directories unless the user explicitly requests recovery import.
3. Parse every Markdown file into a source AST that preserves block boundaries, indentation,
   continuation lines, fenced regions, and raw unsupported constructs.
4. Decode normal page filenames using the configured scheme. Parse Journal identity only from the
   configured journals path and valid date filename.
5. Build a provenance map before resolving links. Preserve valid explicit block `id::` UUIDs when
   collision-free; otherwise derive stable import IDs from graph/import namespace plus source
   identity. Do not use mutable line number alone.
6. Resolve `[[page links]]`, date-title links, and `((block refs))` through the provenance map while
   preserving their raw Markdown spelling.
7. Import assets into content-addressed blob storage and rewrite references transactionally.
8. Produce a dry-run report containing page/journal/block/asset counts, duplicate dates/titles/UUIDs,
   invalid dates, broken refs, missing assets, and unsupported macros/queries/draws.
9. Commit the accepted staged model atomically through typed domain/import services.

`id::` used solely as Logseq block identity may be removed from visible Markdown after its UUID is
preserved. Other unsupported property lines remain recoverable raw source and are reported until a
typed property model can represent them; they are never silently stripped.

Rerunning an import uses its provenance manifest to keep UUID mapping stable and to report source
changes. It does not route through legacy notes-rs schema compatibility or destructively call the
owned-archive restore path.

## 9. Verified source corpus constraints

The development import target was inspected on 2026-07-17:

- 724 journal files in `/home/okhsunrog/Documents/notes/journals`;
- filenames span `2022_12_29.md` through `2026_07_17.md` and use `YYYY_MM_DD`;
- 723 files are non-empty;
- journal content contains thousands of structural bullets, nested blocks, fenced content, asset
  references, explicit `id::` UUIDs, embeds, and logbooks;
- the graph config uses the default `journals/` directory, no journal template, and
  `:file/name-format :triple-lowbar` for normal page filenames.

This is a real corpus, not a toy line-oriented import. Import tests must include representative
fixtures copied/minimized from these constructs without modifying the source graph.

## 10. Invariants and tests

1. A workspace has at most one Journal page per `JournalDate`.
2. Equal `(workspace_uuid, date)` always derives the same journal page UUID.
3. Journal date/kind is immutable and cannot conflict with a Note of the same UUID.
4. Layout changes never change Journal identity or content.
5. Browsing an empty date creates no source state; first edit/capture is atomic.
6. Two offline captures for the same date converge into one page containing both blocks.
7. Localized display/title changes never change link or journal identity.
8. Normal Notes listing excludes journals without excluding them from search/backlinks.
9. Import dry-run performs no source/database mutation.
10. Unsupported source is reported and recoverable, never silently dropped.

Tests cover invalid dates, deterministic IDs, concurrent offline creation, snapshots/archives,
operation reordering, note/journal UUID kind conflicts, calendar pagination, Quick Capture, Logseq
multiline/fence parsing, indentation, UUID collision, date links, missing assets, dry-run, atomic
commit, and stable rerun provenance.

## 11. Migration sequence

1. Add durable `workspace_uuid`, `JournalDate`, tagged `PageKind`, normalized `journal_pages`, and
   the clean-baseline/snapshot/archive/operation changes.
2. Add deterministic journal identity and concurrency tests before UI creation paths.
3. Add `ensure/get/list/append` journal services and generated typed commands/events.
4. Add Today, calendar navigation, ordinary day editing, Quick Capture, and page-kind filters.
5. Add the staged Logseq parser, provenance map, dry-run report, and atomic import.
6. Add the optional virtualized timeline, templates, temporal search filters, and graph controls.

The project is unreleased, so this updates the clean V001 baseline. No compatibility table, nullable
legacy journal column, filename-as-domain identity, or old-database conversion path is retained.
