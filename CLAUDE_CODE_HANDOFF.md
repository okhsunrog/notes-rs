# Claude Code handoff: notes-rs after G5b

Prepared at the deliberate stopping point after the Document Live Preview foundation. This file is
an execution handoff, not a replacement for the architecture records.

## Repository state

- Repository: `/home/okhsunrog/code/rust/notes-rs`
- Branch: `main`
- Last implementation commits:
  - `c39659b feat(markdown): unify bounded notes link parsing`
  - `df4090b feat(document): add live preview authoring modes`
- The user-owned untracked `.codex/agents/` directory must not be edited, deleted, staged, or
  committed.
- No push, deployment, production database mutation, or real Logseq source mutation is implied.
- The project is unreleased and has no users, so correct breaking changes are preferred over legacy
  compatibility paths.

Read these records before changing architecture:

1. `ROADMAP.md`
2. `EDITOR_ARCHITECTURE.md`
3. `WORKSPACE_ARCHITECTURE.md`
4. `JOURNAL_ARCHITECTURE.md`
5. `SYNC_ARCHITECTURE_PLAN.md`

## Product and architecture already implemented

- Rust/SQLite is the durable source of truth; operations use UUID/HLC/LWW sync semantics.
- `PageLayout = Outline | Document` is durable. Reading is pane-local presentation, never synced.
- TanStack Query owns backend snapshots; `PageSession` owns live drafts and enforces one writer per
  page/window.
- Outline is block-first. Document projects the block tree through `DocumentCodec` into one
  continuous Markdown buffer and saves through one revision-guarded atomic replace command.
- Workspace panes and the collapsible Assistant are separate from editor modes.
- Logseq import, Journal identity/UI, attachments, shared semantic Reading renderer, KaTeX, Shiki,
  safe Mermaid, and server-owned AI already exist.
- Automatic paid embeddings/entity extraction default to disabled.
- Desktop and Android share the local notes/sync model; AI and vector indexing are server-owned.

## G5a/G5b completed

### Shared notes-link dialect

Relevant files:

- `src/features/markdown/notes-link-scanner.ts`
- `src/features/markdown/remark-notes-links.ts`
- `src/features/markdown/notes-link-parity.test.tsx`
- `src/features/document/notes-link-markdown-extension.ts`

Behavior:

- one typed linear scanner drives Reading and Lezer recognition of `[[Page]]` and canonical UUID
  block references;
- odd escapes, nested/malformed recovery, Unicode trimming, opacity inside ordinary Markdown
  links/images/reference labels, and renderer/Lezer parity have explicit fixtures;
- production scanning does not allocate diagnostics;
- diagnostics are capped and report truncation;
- open candidates per malformed line are capped at 1,024, fail closed until newline, and then
  recover normally;
- a 1.2 MB adversarial same-line fixture is bounded;
- page-link content that another renderer plugin would split (emphasis/code/math/raw HTML,
  autolinks, video macros) is explicitly inert for now. Do not silently relax this without parity
  tests for both Reading and Lezer.

### Document Live Preview foundation

Relevant files:

- `src/features/document/continuous-document-editor.tsx`
- `src/features/document/document-live-preview.ts`
- `src/features/document/document-authoring-preference.ts`
- `src/features/document/document-authoring-controls.tsx`
- `src/features/document/document-page.tsx`
- `src/features/pages/page-session.tsx`
- `src/features/pages/page-view.tsx`

Behavior:

- Write (`live_preview`) and Source are CodeMirror compartments in the same mounted `EditorView`;
- mode changes preserve the document, selection, and local undo history;
- known Markdown punctuation is visually hidden only outside the active source range;
- a blurred editor renders all visible lines; focus/caret reveals the current source construct;
- decorations cover buffered visible ranges rather than rescanning the whole document per edit;
- background Lezer tree completion triggers decoration refresh;
- mode reconfiguration never runs during IME composition and uses bounded retry plus event/update
  fallbacks;
- preference is versioned device-local state, defaulting to Live Preview;
- Read is semantic React output, not a read-only CodeMirror surface;
- Write/Source are disabled with an accessible explanation if the same page has a writer in the
  adjacent pane, so a rejected transition cannot mutate the global preference;
- compact WebKitGTK title input metrics were adjusted to avoid clipping large title glyphs.

## Verified gate

At the stopping point:

```text
vp check                         pass
vp test                          267 tests pass
vp build                         pass (Vite 8.1.3)
main eager JS                    about 2.493 MB, accepted project policy
git diff --check                 pass
native Wayland WebView           Write/Source/Read and wide/compact inspected
```

The app was run Wayland-native for diagnostics with:

```sh
WEBKIT_DMABUF_RENDERER_DISABLE_GBM=1 \
  vp exec tauri dev --config src-tauri/tauri.dev.conf.json --no-watch
```

Do not turn that environment variable into a permanent application workaround without a separate
decision. A prior WebKitWebProcess crash pointed into `dri_gbm.so/libgbm`; no new coredump appeared
during this G5 session.

## Next logical boundary: G5c

Do not restart the editor architecture. Extend the existing allowlisted Live Preview adapter in
small green commits. Recommended order:

1. Define a typed editor-decoration/widget policy and parity fixture table for each supported
   construct. Keep unsupported syntax visible and recoverable.
2. Add safe task/list interactions and typed internal-link navigation. Modifier-click should use
   the existing workspace `OpenDisposition` path rather than direct WebView navigation.
3. Add fenced-code presentation/copy behavior without runtime grammar downloads.
4. Add table and attachment-image widgets using the existing backend-authorized image resolver;
   never expose `file://` or arbitrary paths.
5. Add math and Mermaid widgets by reusing the existing bounded/sanitized Reading implementations;
   do not create a second sanitizer or allow remote resources.
6. Add reference-link behavior only after Reading/Lezer parity and ordinary-link opacity tests.
7. Improve accessibility: Live Preview is visual CSS today; Reading remains the semantic fallback.
   Widgets need keyboard operation, names, focus behavior, and source reveal.
8. Add a realistic 1-2 MB Document editor benchmark. Avoid pathological full-Lezer benchmarks made
   only of delimiters; custom scanners still require adversarial bounded tests.
9. Validate desktop IME/selection/undo, then perform the named manual Android gate on a real device:
   Gboard Cyrillic, autocorrect, composition, long-press selection, paste, Backspace/Enter, safe
   areas, light/dark theme, and hardware shortcuts.

Keep G5c separate from later retrieval/server work. Do not enable automatic vectorization or entity
extraction during renderer/editor tests.

## Engineering constraints

- Preserve unrelated changes and inspect `git status` before every edit/commit.
- Use logical commits and update the timeline in `ROADMAP.md` after each complete feature boundary.
- Run `vp check`, focused tests, full `vp test`, and `vp build` for frontend stages.
- Inspect visible UI in the real Tauri WebView after UI changes; check wide and compact projections.
- Do not add a second global state cache. Persisted Rust state stays in TanStack Query; editor drafts,
  caret, composition, and dirty state stay local/PageSession-owned.
- Do not mount one CodeMirror instance for every Outline block.
- Do not persist Reading or editor mode through Rust/sync.
- Do not add environment-variable configuration paths; application settings are authoritative.
- Do not add containers for deployment; this project uses native/musl-style deployment conventions.
- Never modify `/home/okhsunrog/Documents/notes`; it is read-only Logseq source data.

## Ready-to-paste prompt for Claude Code

```text
Work autonomously in /home/okhsunrog/code/rust/notes-rs on current main.

First read CLAUDE_CODE_HANDOFF.md completely, then ROADMAP.md, EDITOR_ARCHITECTURE.md,
WORKSPACE_ARCHITECTURE.md, JOURNAL_ARCHITECTURE.md, and the relevant current implementation/tests.
Inspect git status before editing. The untracked .codex/agents/ directory is user-owned: never edit,
delete, stage, or commit it.

Continue from the completed G5a/G5b boundary; do not redesign or replace the existing CodeMirror,
DocumentCodec, PageSession, workspace, Reading renderer, or notes-link scanner architecture. Implement
G5c incrementally: safe semantic Live Preview widgets and navigation, renderer/Lezer parity,
accessibility, realistic large-document performance, and preparation for real Android IME testing.

Start with the smallest high-value vertical slice: typed task/list interactions plus internal page
and block link navigation through the existing OpenDisposition/workspace path. Source must reveal
under caret/focus; unsupported or malformed Markdown must remain visible and recoverable. Reuse the
existing semantic renderer, URL/image policies, sanitizers, and typed APIs. Do not create direct
WebView navigation, file:// access, remote-resource loading, a second Markdown sanitizer, or a new
global backend-state store.

After each logical slice: format, run vp check and focused tests, then full vp test and vp build;
inspect the actual Tauri WebView at wide and compact sizes; independently review the diff for IME,
selection/history, security, accessibility, and O(n) behavior; commit only a green coherent boundary;
and update ROADMAP.md with exact start/end/duration and remaining scope.

Do not enable or call paid embeddings/entity extraction, do not mutate the real Logseq graph, do not
deploy/push/reset databases, and do not add environment-variable configuration or legacy paths.
Stop at the last clean green commit if a gate cannot be satisfied, and report exact commits, tests,
live UI evidence, remaining manual Android checks, and the next starting point.
```
