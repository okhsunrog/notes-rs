# Outliner UI plan

> Status: shipped. This is the original design record; implementation details in the source and README now take precedence over historical future-tense notes below.

Durable plan for the per-block outliner that replaces BlockNote. Written before a context compact so the next session can pick up cold.

## Context: where we are

- **Schema is ready** (`f473d3f`). `nodes` has `parent_id INTEGER REFERENCES nodes(id) ON DELETE CASCADE` and `position REAL`. Indexes: `idx_nodes_parent (parent_id, position)`, unique `idx_page_title (kind='page', lower(title))`. `schema_version` is at 2 and actually written.
- **Rust block CRUD exists** in `src-tauri/src/db.rs`:
  - `list_block_children(parent_id) -> Vec<Node>` — ordered by `position ASC, id ASC`.
  - `create_block(parent_id, position?, content, content_json) -> Node` — auto-appends at `MAX(position)+1.0` if `position` is `None`.
  - `move_block(id, new_parent_id, new_position)`.
  - `get_or_create_page_by_title(title)` — case-insensitive, idempotent via the unique index.
- **Tauri commands + TS wrappers** for the above exist in `commands.rs` and `src/lib/api.ts` (`listBlockChildren`, `createBlock`, `moveBlock`, `getOrCreatePageByTitle`, plus `getNode`).
- **AppState race fixed** (`92db825`). Frontend gates on `isReady()` / `app:ready` event before rendering main UI; don't reintroduce assumptions otherwise.
- **EntitiesCard is event-driven** — `listen("entities:changed")`, no polling. The outliner should follow the same pattern when it needs to react to background work.
- **BlockNote is on its way out.** Current `src/components/note-editor.tsx` (BlockNote wrapper) and `src/features/pages/page-view.tsx` (single-document-per-page) are being replaced. `BlockNoteView` and `useCreateBlockNote` should not appear in new code.

## What we're building

**Model:** every visible bullet is its own `nodes` row with its own `uuid`, `parent_id`, `position`. A page is a root block (`kind='page'`, `parent_id=NULL`). The graph is _bullet-grained_: edges, backlinks, `((uuid))` refs all target individual blocks. `[[Page]]` targets a page row.

**Editor unit:** per-block `<textarea>` (not contenteditable — Tauri's three WebView backends disagree about contenteditable behavior). Two display modes per block:

- **View mode (default):** rendered HTML with styled spans for `[[wikilinks]]`, `((block-refs))`, tags. Clickable. Hoverable.
- **Edit mode (on focus):** swap the rendered span for a controlled `<textarea>` with raw markdown. On blur or 400 ms idle: save → parse refs → swap back to view.

## UX decisions (firm; don't re-derive)

### Keyboard model

| Key                        | Action                                                                                                               |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `Enter`                    | New sibling below; focus it; position = midpoint between current block and next sibling (or `position + 1.0` at end) |
| `Shift+Enter`              | Insert `\n` within current block (soft break)                                                                        |
| `Tab`                      | Indent: `move_block(id, prev_sibling.id, end-position)`                                                              |
| `Shift+Tab`                | Outdent: `move_block(id, parent.parent_id, parent.position + ε)` (place just after old parent)                       |
| `Backspace` on empty block | Delete block; focus prev visible block (DFS predecessor) at end of its content                                       |
| `Ctrl+↑` / `Ctrl+↓`        | Reorder among current parent's siblings                                                                              |
| `Ctrl+Enter`               | Toggle fold (persisted locally per block UUID)                                                                       |
| `[[`                       | Open inline page-link autocomplete (page titles via `list_pages` or new `search_pages_by_title`)                     |
| `((`                       | Open inline block-ref autocomplete (FTS over block content)                                                          |

### Paste, selection, length

- **Multi-paragraph paste:** split on blank lines into N sibling blocks in one database transaction. First paste shows a non-blocking toast; structural Undo restores the previous outline.
- **Selection:** single-block selection only in v1. Native browser select-across-divs is broken; we accept this.
- **Long-block soft nudge:** at ≥ 600 chars in a block, show an info icon next to it (not modal, not blocking). Click → "Split this block into paragraphs?" → applies the same paste-split logic. _Never_ auto-dismissed; user explicitly chooses.

### Wikilinks and block-refs

- **`[[Page Title]]`** on save:
  1. Parse refs from new content.
  2. Diff against existing refs (we'll need to track them — easiest: store a separate `block_refs(block_id, target_id, kind)` table OR just re-emit all refs each save and clean up stale ones).
  3. For each new `[[Title]]`: call `get_or_create_page_by_title(title)`. Emit edge `link_nodes(this_block_id, page_id, 'refs', 1.0)`.
  4. For each new `((uuid))`: look up block by uuid; emit edge `link_nodes(this_block_id, target_id, 'refs', 1.0)`.
- **Stub pages are eagerly materialized** — `[[Some New Concept]]` creates the page row immediately so backlinks work the moment the link is typed.
- **Hover preview only in v1**. No inline transclusion. Hover a `((uuid))` or `[[Page]]` → popover renders `get_node(target)` content with `read_subtree`-style flattening (when that command exists).

### Save semantics

- **Granularity is per-block, not per-page.** No more page-level autosave races.
- **Trigger:** on blur OR 400 ms idle since last keystroke.
- **No "Save" button** anywhere.
- **Save indicator** is per-block, subtle: a small dot on the block's bullet that pulses while dirty and fades after a successful save.

## Edge case decision tree

The cases worth pinning down explicitly so they don't re-derive later:

| Scenario                                                                                            | Decision                                                                                                                                                            |
| --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| User pastes 10 paragraphs into one block                                                            | Split on blank lines → 10 sibling blocks. Cursor lands at end of last new block. First-time toast.                                                                  |
| User types `[[Sarah]]` for a page that doesn't exist                                                | On save (blur/idle), `get_or_create_page_by_title("Sarah")` creates the stub page silently. No prompt. The page appears in the sidebar list next time it refreshes. |
| User edits a block, then clicks another block in the sidebar/page before the 400 ms idle save fires | Blur of the active block triggers immediate save (blur fires before the new block mounts/focuses). No content loss.                                                 |
| User edits a block, then switches active page entirely                                              | Same — blur on the active textarea fires; save flushes before unmount.                                                                                              |
| User indents a top-level block under no prev sibling                                                | Tab is a no-op (no valid new parent). Optionally show subtle feedback.                                                                                              |
| User Tab-indents a block with children                                                              | Children come along — they stay under their (now-indented) parent. `move_block` is a single-row update; children's `parent_id` doesn't change.                      |
| User backspaces an empty block that has children                                                    | Refuse (children would be orphaned). Or: promote children to siblings of the deleted block. v1: refuse, ignore the keystroke.                                       |
| Block content is one continuous 5000-char URL                                                       | Saved as-is. Chunker (later) will degrade via the sentence → word → grapheme ladder. UX nudge appears but is harmless.                                              |
| User pastes BlockNote-style content from clipboard with rich formatting                             | Strip to markdown. We're markdown-source-of-truth now.                                                                                                              |
| `((nonexistent-uuid))`                                                                              | Render as a struck-through chip with title "broken reference." No edge created.                                                                                     |
| `[[Title]]` typed but then deleted before save                                                      | The stub page is created on save, not on keystroke. So no-op — no orphan stubs from typos.                                                                          |

## File layout (proposed)

```
src/
  features/
    outliner/
      outliner.tsx           # top-level: takes pageId, renders the tree
      block-view.tsx         # one block in view mode (rendered markdown w/ spans)
      block-edit.tsx         # one block in edit mode (controlled textarea)
      block-node.tsx         # switches between view/edit; owns local state for one block
      block-tree.tsx         # recursive renderer; given a parent_id, fetches children, renders <BlockNode> for each
      use-block-tree.ts      # hook: load children, handle CRUD + optimistic updates
      keyboard.ts            # key handlers (Enter / Tab / etc.) — pure functions taking (block, action) -> intent
      parse-refs.ts          # extract [[…]] and ((…)) from markdown content
      render-markdown.tsx    # render markdown with styled link/ref spans (lightweight; no full md parser needed)
    pages/
      page-view.tsx          # REWRITE: hosts <Outliner pageId={page.id} /> + the page title input
      pages-list.tsx         # unchanged
src-tauri/src/
  db.rs                      # may need: search_pages_by_title, search_blocks_by_content (for autocompletes)
  commands.rs                # any new commands
```

`src/components/note-editor.tsx` (BlockNote wrapper) gets deleted at the end. Don't reference it from new code.

## Implementation order (suggested)

1. **`Outliner` skeleton.** Given `pageId`, calls `listBlockChildren(pageId)` and renders one level. Just `<div>` per block, no editing yet. Confirms the load path works end-to-end.
2. **Recursive `BlockTree` with state.** Each `BlockNode` owns its block; child trees lazy-load. Reordering = optimistic update + `moveBlock`.
3. **View ↔ edit mode swap.** Click/focus enters edit (textarea); blur saves and re-renders view. View mode is plain text initially, no parsed spans yet.
4. **Keyboard model — Enter, Backspace, Tab/Shift+Tab.** Each maps to an existing CRUD call. Position math (fractional indexing) lives in `keyboard.ts`.
5. **Markdown rendering in view mode.** Lightweight: regex-based span replacement for `[[…]]`, `((…))`, `**bold**`, `*italic*`, `code`. Don't pull in a full md parser yet.
6. **Wikilink + block-ref save-time parsing.** On every block save: parse refs, diff against last-known set, call `get_or_create_page_by_title` + `link_nodes`. Need a new Rust helper to "replace all refs for block X" (delete old, insert new) atomically, OR a per-block ref tracking column.
7. **Hover preview popover.** On hover of a link/ref span, fetch the target's first ~200 chars via `getNode(id)` and show a small popover. shadcn `HoverCard` works here.
8. **Autocomplete on `[[` and `((`.** Needs Rust helpers (or just reuse `search_fts` filtered by kind).
9. **Multi-paragraph paste split + first-paste toast.**
10. **Long-block UX nudge.**
11. **Delete `note-editor.tsx`** and update `App.tsx` to drop unused imports.

Steps 1–4 are the load-bearing core. 5–7 are the "feels right" layer. 8–10 are polish. Don't try to land it all in one commit; 3–4 commits feels right.

## Open questions to decide as we go

- **Folding state:** shipped with per-block LocalStorage persistence.
- **Per-block ref tracking:** simpler to re-emit-and-cleanup on every save (delete all edges where src=block_id AND kind='refs', then re-insert). Inefficient but correct. Switch to diff-based if hot-path-slow.
- **Optimistic UI vs round-trip:** start with round-trip for safety; layer in optimistic update for Enter / Tab specifically since those need to feel instant.
- **Block-ref autocomplete UX:** `((` shows FTS results from all blocks — but search-as-you-type might be too slow with a full vector pipeline. Use FTS-only for autocomplete, defer rerank to explicit search.
- **What to do with the existing `kind='block'` orphan rows** (from before hierarchy): leave as orphan roots in the UI. They won't show under any page. Optionally surface them in a "Loose blocks" sidebar section later.

## Things the next session should NOT need to re-derive

- That contenteditable is rejected; textarea is the answer.
- That BlockNote is being removed (not extended).
- That the data model is bullet-grained, not page-grained.
- That stub pages from `[[…]]` are created eagerly on save (not on keystroke, not on user prompt).
- That selection is single-block in v1.
- That multi-paragraph paste splits into siblings.
- That long-block enforcement is a soft nudge, not a hard limit.
- That hover previews replace transclusion in v1.

## After the outliner ships

These were deferred _behind_ the outliner. Graph expansion, agent backlink/tag/subtree tools, local BGE models, dark mode, the error boundary, and node deletion have since shipped. Remaining larger follow-ups are:

- Chunker over blocks (the agreed algorithm: block-chunk for ≥ THRESHOLD_LARGE, parent-cluster chunk for parents with small children, breadcrumb prefix, sentence→word→grapheme degradation).
- Multi-column FTS5 (`title`, `fcontent`, `body`, `tags`) with column weights at query time.
- Stable page routing/deep links, richer graph controls, and bundle splitting.

## Reference repos cloned at /tmp during planning

- `/tmp/graphiti` — Zep's PKM graph. See `graphiti_core/utils/content_chunking.py` for the should_chunk heuristic; episode/entity/community split.
- `/tmp/nano-graphrag` — ~1100 LOC GraphRAG. `_op.py` is the readable extraction reference.
- `/tmp/siyuan/kernel/sql/` — closest real-world block-outliner with SQLite. `blocks(id, parent_id, root_id, ..., length, type, ...)` + `refs` separate table + multi-column FTS5.

If those are cleaned up before the next session, recloning is cheap.
