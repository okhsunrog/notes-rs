# Workspace Panes and Companion Architecture

Status: accepted on 2026-07-17 and implemented. The Zustand pane tree, adjacent navigation,
responsive compact projection, linked Reading sessions, and collapsible Assistant dock are shipped;
native multi-window hosting and promoting the Assistant into a normal pane remain reserved.

This decision defines how one native notes-rs window composes navigation, one or more independent
content surfaces, and the AI companion. It deliberately treats side-by-side layout as a general
workspace capability rather than a special Document editor mode.

## 1. Goals

The workspace must support these scenarios through one model:

```text
Page A editor | Page B editor
Page A editor | linked rendered preview of Page A's draft
Page          | graph focused around that page
Journal       | referenced page
Page          | Assistant, if promoted into a pane later
```

Adding a second surface must not duplicate page data, create a second durable content source, or
introduce feature-specific split implementations. Responsive layout must not destroy pane or editor
sessions merely because the window becomes narrow.

## 2. Workspace composition

One native window has three independent regions:

```text
WorkspaceWindow
├── NavigationSidebar
├── Workbench
│   └── PaneTree
│       ├── Pane
│       └── Pane
└── AssistantDock
```

The navigation sidebar and Assistant dock are window chrome. They are not leaves in the primary
pane tree and collapsing either one does not rewrite the workbench layout.

The workbench is a binary layout tree conceptually equivalent to:

```text
WorkspaceNode =
  Pane(PaneId)
  | Split {
      axis: Horizontal | Vertical,
      ratio,
      first: WorkspaceNode,
      second: WorkspaceNode,
    }
```

The first implementation may expose at most two horizontal panes. The recursive model is retained
so a later UI can allow more panes or vertical splits without replacing navigation and session
ownership.

## 3. Pane state and content

A pane is UI identity and navigation state, not a domain object. `PaneId`, split IDs, sizes, focus,
and navigation history never enter Rust operations, archives, snapshots, RPC, or sync.

Pane content is a serializable discriminated union rather than a stored `ReactNode`:

```text
PaneContent =
  Home
  | Page { page_uuid, optional_block_uuid, presentation, session_binding }
  | Graph { optional_focus }
  | JournalTimeline { anchor_date }
  | Assistant { session_id }       // reserved/promotable
```

Each kind is rendered by an exhaustive registry/switch. New surfaces can therefore participate in
the same split, focus, close, navigation, and responsive behavior without changing the tree model.

`JournalTimeline` is defined in [`journals.md`](journals.md). Individual
journal days resolve to ordinary Page content and need no special editor pane.

Each pane owns:

- its current `PaneContent`;
- independent back/forward history;
- scroll and local selection/focus restoration;
- pane chrome actions such as close, split, move, and open beside.

The window owns the tree, pane records, active pane ID, split ratios, and compact-mode visible pane
ID. Workspace state stores content UUIDs, never cached `Page` or `Block` snapshots; TanStack Query
continues to own shared backend cache.

## 4. Page and editor sessions

`PageLayout = Outline | Document` remains durable, synced page state. A pane displaying a Document
chooses a local `PagePresentation = Editing | Reading`. `EditingMode = LivePreview | Source` remains
a device preference with an optional pane override.

Split is not a value of `PagePresentation`.

One native window has at most one writable `PageSession` for a page UUID. A session owns the current
CodeMirror state, draft revision, dirty/composition state, selection, and draft subscribers. Several
panes may project the same session, but the application must not silently create two divergent
writable drafts of one page in one window.

An editor-plus-preview command is a composite workspace action:

1. split the active pane or reuse an adjacent pane according to UI policy;
2. open the same page there with `Reading` presentation;
3. bind that pane to the source `PageSession` draft.

The rendered pane therefore sees unsaved text in real time. It does not refetch the last SQLite
snapshot and does not create another editor state. If its source editor closes, the linked pane is
closed or deterministically becomes an unlinked rendering of persisted content after dirty-state
resolution.

## 5. Navigation intents

Every navigation source uses one typed intent:

```text
open_target(target, disposition)

OpenDisposition =
  Current
  | Adjacent { optional_axis }
  | NewNativeWindow          // reserved, not required initially
```

- A normal click navigates the active pane.
- `Shift+click` opens a link, page, block, search result, graph item, or journal day in an adjacent
  pane.
- Touch receives the same capability through a context menu or long press.
- A block target carries both containing page UUID and block UUID so focus is reproducible.
- Feature components do not pass raw DOM mouse events through the navigation boundary.

Whether repeated `Adjacent` requests create another split, reuse a neighbor, or obey a two-pane UI
limit is replaceable policy. It does not change the intent or pane tree.

Global editor commands act on the active pane/session. Pane chrome owns close and navigation;
individual page components do not mutate global workspace selection directly.

## 6. Responsive projection

Responsive behavior changes only the projection of the workspace tree:

- desktop and landscape tablet render supported splits with resizable separators;
- compact/mobile renders one leaf at a time with a pane switcher;
- hidden compact leaves remain open and keep their navigation/session state;
- Editor/Preview becomes a quick pane transition on a phone rather than two unusably narrow halves;
- returning to a wide viewport restores the same side-by-side tree.

Resizing the window must not close panes, flush a draft solely because of a breakpoint, remount an
active editor during IME composition, or rewrite workspace history.

## 7. Assistant dock

The default AI experience is one collapsible auxiliary dock per native window, outside the primary
pane tree:

```text
AssistantDockState {
  visibility: Hidden | Rail | Open,
  width,
  context_binding,
}
```

- `Open` is resizable, approximately 320-480 px on desktop.
- `Rail` is approximately 44-48 px and exposes expand, streaming, unread, and error state.
- `Hidden` removes the dock from layout and tab order.
- Below a useful width the dock becomes an overlay/drawer instead of crushing workbench panes.
- Android uses a full-surface/bottom-navigation presentation, not a narrow rail.
- Visibility and width are device/window-local and never synced.

The `AssistantSession` is independent from its host component. Collapsing, hiding, moving between
compact surfaces, or later promoting the Assistant into a normal pane does not lose its input,
thread, scroll, streaming request, or pending tool state.

AI context is explicit:

```text
AssistantContext =
  FollowActivePane
  | PinnedPage(page_uuid)
  | PinnedBlock(block_uuid)
  | PinnedSelection(pane_id, draft_revision, range)
  | Workspace
```

`FollowActivePane` is the default. Every submitted turn captures an immutable context snapshot so a
later focus change cannot alter an in-flight request. The dock shows its current context and allows
pin/unpin. One Assistant session may later be rendered as `PaneContent::Assistant`; that is an
additional host for the same controller, not a separate chat implementation.

Dock accessibility requires an `aria-expanded`/`aria-controls` toggle, focus restoration on close,
keyboard-resizable separators, an overlay focus trap only when used as a modal drawer, and coarse
`aria-live` streaming state rather than announcing every token.

## 8. State ownership summary

| State                                            | Owner                                     |
| ------------------------------------------------ | ----------------------------------------- |
| pages, blocks, `PageLayout`, journal identity    | Rust/SQLite and sync                      |
| backend snapshots                                | TanStack Query                            |
| pane tree, ratios, focus, navigation stacks      | device/window-local workspace store       |
| draft, caret, selection, composition, CM history | page/editor session                       |
| Assistant thread/input/stream/context            | window-local Assistant session controller |
| Assistant visibility and width                   | device/window-local dock state            |

Workspace state may be restored after restart on the same device, but it is never content sync.
Restoration validates that referenced pages still exist and falls back to Home for stale targets.

## 9. Invariants and validation

1. The pane tree is acyclic; pane IDs are unique; every leaf references an existing pane.
2. The active pane always references a current leaf.
3. Workspace state contains IDs and serializable UI state, not React components, CodeMirror objects,
   or RPC records.
4. All navigation sources use `open_target` and an `OpenDisposition`.
5. Responsive projection never mutates the pane tree.
6. A linked Reading pane consumes the exact in-memory revision of its source session.
7. One writable page session exists per page UUID per native window.
8. Closing a dirty editor resolves/flushes the draft before dependent panes are detached.
9. Assistant collapse/unmount never cancels a stream or loses input.
10. Pane layout, Reading presentation, navigation stacks, and dock state are never synced.

Tests must cover tree normalization, focus after close/split, Shift+click from every navigation
surface, linked preview revisions, dirty close, compact/wide transitions, Assistant collapse during
streaming, and keyboard/focus behavior.

## 10. Migration sequence

1. Introduce a window-local workspace store and typed `PaneContent`/`OpenDisposition` contracts.
2. Replace the single `activePageUuid` controller with pane-aware navigation and shared page
   sessions.
3. Split the current layout into `AppShell`, `Workbench`, and `AssistantDock`.
4. Move Graph from its full-workspace special case into a pane surface.
5. Implement adjacent page opening and linked Document preview using the same pane mechanism.
6. Move Assistant state out of `ChatCard`; add controlled Open/Rail/Hidden behavior.
7. Add compact pane switching without destroying the desktop pane tree.

No compatibility layer for the provisional single-center workspace or Document `Split` mode is
retained before release.
