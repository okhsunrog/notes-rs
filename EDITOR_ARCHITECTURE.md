# Editor and Page Presentation Architecture

Status: accepted on 2026-07-17 and partially implemented. The durable layout/presentation boundary,
shared semantic renderer, pane workspace, marker-free `DocumentCodec`, revision-guarded atomic
replace, continuous CodeMirror Source authoring, and linked Reading projection are implemented.
Document Live Preview decorations and the remaining large-document/mobile gates are still pending.

This decision defines how notes-rs presents and edits the same typed page/block model as an
outliner and as a continuous Markdown document. It deliberately separates durable content from
pane-local presentation and from server-side retrieval chunks.

General side-by-side composition, navigation, linked preview panes, and the Assistant dock are
defined in [`WORKSPACE_ARCHITECTURE.md`](WORKSPACE_ARCHITECTURE.md). Split is a workspace operation,
not an editor mode.

Journal is an orthogonal typed page identity using the same editor and is defined in
[`JOURNAL_ARCHITECTURE.md`](JOURNAL_ARCHITECTURE.md).

## 1. Goals

The editor must support both short block-first notes and long articles or project documentation
without introducing two incompatible content models. It must also preserve:

- the typed page/block tree as durable source state and Markdown as its portable textual body;
- stable UUID-addressed blocks for references, sync, history, and navigation;
- exact local-first operation semantics on desktop and Android;
- practical normalized Markdown import/export, including large pasted or imported documents;
- a focused Outline workflow and an Obsidian-like Document workflow;
- one extensible editor engine rather than separate rich-text implementations.

Storage blocks, visible editing regions, and semantic retrieval chunks are intentionally distinct:

```text
Page
  -> durable Block tree       UUID-addressed, synced source state
  -> editor presentation     Outline block or continuous Document buffer
  -> retrieval chunks        disposable, server-only derived AI data
```

A storage block is the smallest addressable editing/sync unit. It is not required to be one visual
bullet, one CodeMirror syntax node, or one embedding document.

## 2. State ownership

The former persisted `PageView = Outline | Document | Reading` has been replaced by the following
state ownership boundary. Reading no longer crosses the Rust/RPC/sync contract.

| State                    | Values                          | Owner and lifetime                        |
| ------------------------ | ------------------------------- | ----------------------------------------- |
| `PageLayout`             | `Outline`, `Document`           | Rust/SQLite; synced with the page         |
| `PagePresentation`       | `Editing`, `Reading`            | React pane/session; never synced          |
| `EditingMode`            | `LivePreview`, `Source`         | Device-local application preference       |
| active block/selection   | UUID, ranges, caret             | Editor component/session                  |
| drafts/composition state | text, dirty state, IME session  | Editor component/session                  |
| folding                  | collapsed block UUIDs           | Device-local UI state                     |
| source content           | pages, blocks, styles, ordering | Rust/SQLite through typed commands/events |

`PagePresentation` may be remembered locally per window or page for convenience, but it must never be
part of an operation, archive, snapshot, or sync payload. A remote device must not change what the
current pane is showing.

`EditingMode` is a device preference because keyboard, screen size, and authoring preferences vary
between desktop and Android. Changing it never mutates page content.

## 3. Product behavior

### Outline layout

Outline has one primary workflow: Editing with Live Preview.

- Inactive blocks are rendered Markdown.
- The focused block becomes editable in place.
- Markdown markers are revealed only where the caret/selection needs them.
- There is no persistent view-mode switcher in the page header.
- Reading is not an Outline mode; side-by-side panes belong to the workspace.
- Raw Source for the active block may remain available as an advanced command, but is not a normal
  visible mode.

Only the active block mounts an editor instance. Inactive blocks remain lightweight semantic React
output; notes-rs must not mount one editor per visible block.

### Document layout

Document presents the page as one continuous authoring surface even though storage remains a tree
of UUID blocks.

- `Editing` shows a continuous Markdown editor, using Live Preview by default.
- `Reading` shows fully rendered Markdown without an editable surface.
- `Source` uses the same text and editor state as Live Preview with preview decorations disabled.

Document presentation commands are always reachable from the note menu, command palette, and
shortcuts. A device-local preference may pin a compact `Write / Read` control in the pane chrome;
it is hidden by default so it consumes no permanent space.

Reading uses the same semantic Markdown renderer as a linked preview pane. An editor-plus-preview
layout opens a second Reading pane bound to the first pane's in-memory `PageSession`; it is not a
third Document mode. A read-only editor DOM is not the reading surface.

## 4. Editor engine

notes-rs will standardize on CodeMirror 6 as the programmable Markdown editor engine.

It is used in two configurations:

```text
Outline:  one EditorView for the active storage block
Document: one EditorView for a virtual continuous page buffer
```

Live Preview and Source are configurations of the same `EditorState`; switching modes must not copy
text into another editor or discard selection/history. CodeMirror compartments, state fields,
effects, transactions, syntax trees, and decorations are implementation tools inside the editor
adapter, not domain types.

Use the official CodeMirror packages directly behind a small React lifecycle component. Avoid a
third-party React wrapper unless it demonstrates a concrete need that the adapter cannot cover.

CodeMirror is an engine, not a ready-made Obsidian clone. notes-rs owns:

- Live Preview decoration policy;
- `[[wikilink]]` and `((block-reference))` syntax and autocomplete;
- task, property, attachment, and future extension widgets;
- mapping between Document source ranges and block UUIDs;
- translation of editor transactions into typed domain intentions;
- remote-update and dirty-draft behavior;
- mobile composition safeguards.

No CodeMirror type may cross the generated RPC boundary or become part of `notes-core`.

## 5. Markdown dialect and rendering

The durable source is the typed page/block tree: `BlockStyle` holds block-level semantics and each
block's `markdown` field holds its textual body. Markdown import/export is a normalized portable
representation of that model, not a byte-for-byte archive of UUIDs and arbitrary tree metadata.

All editing and rendering surfaces must share one documented dialect:

- CommonMark plus the supported GitHub Flavored Markdown extensions;
- notes-rs wikilinks and UUID block references;
- typed `BlockStyle` semantics;
- explicitly registered future extensions such as properties or transclusion.

`BlockStyle` is a closed tagged value. Its `Task { state: TaskState }` variant carries workflow
state atomically, so neither storage nor RPC can express a task without state or attach state to a
non-task block. Explicit state selection supports Todo, Doing, Now, Later, Done, Waiting, and
Cancelled. The checkbox action completes any open state as Done and reopens a terminal state as
Todo; Reading presentation renders status without mutation controls. Style and task-state updates
share the same `BlockSetStyle` operation and LWW clock.

The CodeMirror/Lezer grammar and the semantic renderer must have parity tests over the supported
dialect. The authored-note and AI surfaces now share one AST-based renderer; Document Reading,
linked preview panes, search excerpts, and future extensions must reuse that boundary instead of
introducing another Markdown implementation.

Unsupported or incomplete inline syntax must remain recoverable in a block's source text. The
visual editor must not silently drop unknown Markdown merely because it does not decorate it.
Canonical export may normalize equivalent whitespace, marker choice, and other formatting, but
must preserve supported semantics. Workspace archives and sync snapshots, not plain Markdown, are
the lossless representation of UUIDs, parentage, styles, and operation state.

## 6. Continuous Document adapter

The Document editor works over a virtual Markdown buffer assembled from the page's ordered block
tree. One versioned `DocumentCodec` owns the normalized serialization, parsing/reconciliation
policy, and a source map conceptually equivalent to:

```text
BlockSpan {
  block_uuid,
  block_style,
  source_range,
}
```

This is an editor projection, not persisted content or a second source of truth. Existing ranges
move through ordinary text transactions; structural edits explicitly create, split, merge, delete,
or reorder blocks through typed domain commands.

The user must not maintain hidden storage boundaries manually:

- normal Document typing behaves like continuous Markdown authoring;
- pasted or imported Markdown is segmented automatically;
- Markdown structural boundaries and an explicit segmentation policy produce storage blocks;
- long source regions may become several blocks, and short adjacent regions may remain distinct
  when identity or references require it;
- a block referenced by UUID cannot silently lose its identity during harmless formatting edits.

The exact segmentation and identity-matching algorithm is replaceable policy behind the document
adapter. It preserves UUIDs for unchanged blocks, creates UUIDv7 values only for new blocks, and has
explicit deterministic rules for split, merge, delete, reorder, and style changes. It must be
specified with normalized round-trip and identity fixtures before the full Document editor is
shipped. UUIDs are never embedded as visible Markdown markers merely to support the editor.

Generic Markdown import creates new UUIDs. A provenance-aware Logseq/Obsidian importer may recover
source UUIDs and metadata when the source format provides them. Plain Markdown export is normalized
and is not expected to reproduce hidden UUID or parentage metadata byte for byte.

Document editing does not flatten the durable outline permanently. The adapter must preserve typed
styles, parentage, and order or reject an edit whose meaning is ambiguous; it must never guess in a
way that silently loses structure.

## 7. Transactions, history, and synchronization

CodeMirror transactions own immediate editor behavior, selection, composition, and unsaved local
changes. Rust operations remain the durable mutation and synchronization boundary.

- Local editor changes are sent as a page-level transactional reconciliation intention. Rust
  atomically emits the required granular page/block operations and one coherent history action;
  the UI does not orchestrate a sequence of independently failing RPCs.
- Remote operations enter an open editor as explicitly tagged remote transactions, not by replacing
  the entire editor state.
- Remote changes must not enter local text undo history as if the user typed them.
- A dirty local draft is never overwritten silently; conflict handling compares the corresponding
  persisted revision/HLC and presents an explicit resolution path.
- Rust action undo/redo remains authoritative for committed structural/domain actions.
- Editor-local undo/redo may cover uncommitted text transactions, but the ownership boundary must be
  tested so one shortcut cannot replay both layers.

## 8. Android and performance constraints

The editor must treat IME composition as a protected transaction interval. It must not normalize,
split blocks, remount the editor, or reconfigure syntax under an active composition. Real-device
validation includes Gboard Cyrillic input, autocorrect, long-press selection, paste, boundary
Backspace/Enter, and hardware-keyboard shortcuts.

Live Preview work is limited to changed or visible ranges where possible. The implementation must
not rescan or decorate an entire large document on every keystroke. Page assembly, block diffs, and
backend queries remain independently measurable; viewport rendering does not excuse an O(n) RPC
loop per block.

## 9. Rejected directions

- **Keep the textarea and extend it:** rejected because notes-rs would have to rebuild selection,
  IME, history, syntax trees, decorations, accessibility, and large-document behavior.
- **Use one rich-text/JSON editor as the canonical format:** rejected because Markdown portability,
  source-text recoverability, Logseq/Obsidian import, and custom syntax are product requirements.
- **Use different editor engines for Outline and Document:** rejected because dialect behavior,
  shortcuts, mobile bugs, extensions, and undo would diverge.
- **Persist Reading as a page view:** rejected because it is pane-local presentation, not content.
- **Make storage blocks equal retrieval chunks:** rejected because addressable sync granularity and
  semantic retrieval granularity have different requirements.

ProseMirror-, Lexical-, and JSON-block-based editors may be reevaluated if Markdown stops being the
canonical format. They are not the current default.

## 10. Implementation gates

Before replacing the current editor everywhere, a focused CodeMirror spike must prove:

1. one active Outline block with GFM, wikilinks, block references, autocomplete, autosave, and
   remote-draft handling;
2. one continuous Document containing 200-500 UUID blocks with split, merge, paste, selection, and
   remote updates;
3. canonical normalized import/edit/export round trips, supported semantics, and recovery of
   untouched unknown inline source;
4. responsive editing and preview for approximately 1-2 MB of Markdown;
5. correct desktop undo ownership and real Android Gboard/IME behavior;
6. matching semantic output between Live Preview and the shared Reading renderer.

If a gate fails, the adapter or segmentation policy is revised before broad migration. The durable
page/block model and RPC boundary do not depend on a particular decoration implementation, so these
changes remain localized.

## 11. Migration sequence

1. **Completed:** replace persisted `PageView` with `PageLayout = Outline | Document`; remove
   Reading from schema, operations, snapshots, archives, RPC, and sync.
2. **Completed:** introduce the shared Markdown dialect and AST renderer and remove the old regex
   renderer.
3. Complete the CodeMirror spike and define the editor and versioned `DocumentCodec` contracts.
4. Replace the active Outline textarea while preserving current block operations and conflict UI.
5. Implement the continuous Document session and automatic segmentation fixtures.
6. Add pane-local Write/Read and device-local editor preferences; compose side-by-side preview
   through the general workspace pane architecture.
7. Delete the old regex renderer, textarea editor, and any provisional view compatibility code.

Because the project is unreleased, this migration updates the clean V001 baseline and requires a
fresh development database. No legacy `PageView::Reading` compatibility path will be retained.
