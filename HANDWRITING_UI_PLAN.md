# Handwriting UI Integration + E-ink Display Profile Plan

Self-contained handoff plan for an executor agent. Work through the tracks **in order** (A → B → C → D); within a track, one commit per task. Every task lists steps, guardrails, and acceptance criteria. If something doesn't match this plan, or a decision arises that the plan doesn't cover: **stop and report** — do not improvise.

## Context

Tauri 2 note-taking app: Rust core (`crates/notes-core`, `notes-sync`, `ink-format`, …), Axum server (`server/`), React 19 + Tailwind 4 + Base UI frontend (`src/`), Android plugin `plugins/tauri-plugin-mobile-system` (Kotlin, ONYX Pen SDK). Generated bindings: `src/lib/bindings.ts` — **never edit by hand**; regenerate with `vp run bindings:generate` after Rust command changes.

The handwriting **backend** is finished (see `docs/planning/handwriting-backend-handoff.md` — read it fully before Track A; it is the contract). The frontend still calls removed commands, so `vp check` currently reports 6 type errors in `src/lib/api.ts` and `src/features/handwriting/handwriting-sheet.tsx`. Track A closes that gap. Tracks B–D make the app good on the user's e-ink tablet (Onyx BOOX Note Air 4C, Kaleido 3 color panel, WebView Chrome 151) without degrading other devices.

Device facts that shape the design (measured 2026-09-06, do not re-derive):

- Viewport on the tablet is 661×882 CSS px at DPR 2.8 (per-app DPI set by the user). The app therefore always runs in the **compact layout** there; e-ink work targets the compact layout first. Do not change the compact threshold and do not touch WebView text zoom / DPI.
- The WebView reports `prefers-reduced-motion: reduce`, `hover: none`, `pointer: coarse`, `any-pointer: fine` (pen). Tailwind 4 already wraps `hover:` utilities in `@media (hover: hover)`.
- `(update: slow)` is **not** reported; e-ink detection must be native.
- EinkWise (vendor per-app optimizer) is intentionally not used; the app controls the display through documented SDK calls only (`EpdController`, `UpdateMode`).

## Global rules

1. Validation gates after every task: `vp check`, `vp test`; tasks touching Rust also run `cargo fmt --all -- --check`, `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings`, `cargo test --workspace --locked --all-features`; tasks touching Kotlin also build the plugin unit tests (`./gradlew :tauri-plugin-mobile-system:testDebugUnitTest` from `src-tauri/gen/android`, or the equivalent the repo already uses — check `docs/planning/handwriting-prototype.md` for the command that was used).
2. Add JS deps with `vp add`, Rust deps with `cargo add`. Never hand-edit `package.json`/`Cargo.toml` for adds. Prefer no new deps.
3. Do not reformat or "clean up" code outside the files a task names. Preserve user-facing strings unless a task changes them deliberately.
4. Commit per task, conventional style (`feat(handwriting): …`, `fix(android): …`, `feat(appearance): …`). **Never mention any AI assistant, model, or tool in commit messages or trailers.** No `Co-Authored-By`, no session links.
5. Safe stopping points are the track boundaries. Report progress there. Track A ends with a device install that the user performs — stop before it.
6. Do not modify `src/features/handwriting/ink-canvas.tsx`, `onyx-ink.ts`, `ink-editing.ts`, `ink-region.ts`, `ink-model.ts` or the Kotlin `OnyxInk.kt` in Track A except for the prop/name changes a task explicitly lists. The frame fence, eraser gate, lasso and native reconciliation are verified on hardware and must not regress. Existing tests in `onyx-ink-session.test.tsx` and `ink-canvas.test.tsx` must keep passing (adapt their harness to new props/names only).
7. Do not add compatibility shims, `any` casts, or `@ts-expect-error` to silence the old-command errors; replace the calls.

---

## Track A — Handwritten notes as ordinary notes

Goal: a handwritten note is a `Page` with `kind.kind === "handwriting"`, listed, opened, renamed, favorited, deleted and synced like any note; the integrated editor replaces the scratch dialog; the lifecycle in the handoff's "Жизненный цикл редактора" section is implemented verbatim.

### A1. API wrappers and session model (TS)

1. In `src/lib/api.ts` remove `loadHandwritingDraft`, `saveHandwritingDraft`, `compactHandwritingDraft`. Export, through `checkedCommand`: `createHandwrittenNote`, `loadHandwritingNote`, `saveHandwritingPatch`, `handwritingHistory`, `completeHandwritingNote`, `completeAllHandwriting`, `handwritingNoteStatus`, `previewHandwritingVersion`, `resolveHandwritingConflict`. `setHandwritingBackground` is exported raw (it returns `void`, not a typed result).
2. Replace `src/features/handwriting/handwriting-session.ts` (boolean `open`) with a per-note session registry module (suggested name `handwriting-session.ts`, keep the file): it owns, per page UUID, the `DraftWriter`, the acknowledged revision, and the **pending completion promise**. Completion must be awaitable by a later mount of the same UUID and must survive unmount of the component that started it (module-level map, not React state). Expose pure functions so they are unit-testable without React: `beginSession(uuid, snapshot)`, `getWriter(uuid)`, `requestCompletion(uuid, run)` (dedups concurrent requests, records the promise, clears it on settle), `awaitCompletion(uuid)`, `endSession(uuid)`.
3. Adapt `DraftWriter` construction to the new save signature: `saveHandwritingPatch(pageUuid, patch, expectedRevision)`; `incrementalDraftSaver` gets a closure bound to the UUID (the handoff explicitly says this is enough). Delete `complete-draft.ts` and its test; completion is now the registry's job (A3).
4. Delete `HandwritingHost` from `src/main.tsx` and the file; the editor is pane-hosted (A2). `App.tsx` line ~77 disables a query while the sheet is open — replace that condition with "active pane shows a handwriting page" only if the original reason still applies (read the surrounding code; if unsure, stop and report).

**Accept:** `vp check` has zero errors after A2 (A1 + A2 may be one commit if A1 alone cannot type-check); unit tests for the registry: dedup of concurrent completion, awaiting a previous completion from a new mount, error from completion does not drop the writer's pending gestures.

### A2. Pane-hosted editor and routing by page kind (TS)

1. In `src/features/workspace/workbench.tsx` `PagePane`: when `page.kind.kind === "handwriting"` render `HandwritingNoteView` (new file `src/features/handwriting/handwriting-note-view.tsx`) instead of `PageView`. Never mount the text editor, never create a block for such pages. `key={page.uuid}` stays.
2. `HandwritingNoteView` is the current `HandwritingSheet` restructured: no `Dialog`; a compact header with **Back** (calls the workspace `closePage`/return-to-previous navigation the way the compact layout already does for text pages — find how `PageView` header does it and reuse), the editable title (reuse the title save path `renamePage` + `useTitleDraftOverlay` if that is what `PageView` uses; otherwise a minimal textarea with the same autosave debounce), favorite toggle (`usePageNavigationStore`), delete (controller `onDelete`, same confirmation as text notes); then the two toolbars; then the sheet area filling the rest. Remove the "Local draft · not yet added to your notes" description, the stroke-count/input-details/"saved on device" texts. Save errors, sheet-full and native-ink-unavailable messages stay visible and retryable exactly as today.
3. Keep `.ink-dialog` CSS working: rename the selector in `src/index.css` to a data attribute the new view sets (e.g. `[data-ink-editor]`), nothing else in that rule changes.
4. Keyboard: Ctrl/Cmd+Z / Shift+Z undo/redo stay; Back overlay registration (`registerBackOverlay`) is replaced by the normal Android Back handling of a page pane (check `src/lib/back-overlays.ts` consumers; if the pane already handles Back through compact navigation, do not double-handle).
5. `NewNoteButton` "Write by hand" → `createHandwrittenNote(null)` → `queryClient.setQueryData(queryKeys.page(uuid), page)` → `invalidate(queryKeys.pages)` → `workspace.selectPage(page)`. Add the needed callback to the workspace controller/props rather than importing the store in the button. Keep the capability gating exactly as today for **creation only**.
6. `InkCanvas` props unchanged. `mouseEnabled` and `nativeInk` wiring unchanged.

**Accept:** opening a handwriting page shows the editor in the pane (desktop and compact); text pages unchanged; "Write by hand" creates and opens a note; `vp check` and `vp test` green; `onyx-ink-session.test.tsx` adapted to render the new view (same assertions).

### A3. Editor lifecycle (TS)

Implement the handoff's numbered lifecycle. Concretely:

1. On mount: `await awaitCompletion(uuid)` (a previous session may still be completing), then `loadHandwritingNote(uuid, editing)` where `editing = canDraw` (see A4), then `beginSession`. If the page is already being edited in another pane of this app, show a read-only notice instead of a second editor (two editors of one UUID are unsupported).
2. Every completed gesture → `writer.write(next)` (unchanged behavior). Undo/Redo: `await writer.flush()` → `handwritingHistory(uuid, redo, writer.getRevision())` → `applyHistoryUpdate` → update writer/revision/canUndo/canRedo, clear selection. Errors keep unacknowledged local data; never invent a revision.
3. Back: finish the active native gesture (existing `active` gate), `await writer.flush()`; on success navigate away immediately and call `requestCompletion(uuid, () => completeHandwritingNote(uuid))` detached; on completion error show a retryable toast/notice ("Could not prepare sync" with Retry) — never treat leaving as server confirmation. On flush failure stay on the page with the existing save-error UI.
4. `visibilitychange` hidden / `pagehide`: finish gesture, `setHandwritingBackground(true)`, flush, `requestCompletion`. `pagehide` additionally calls `completeAllHandwriting()` after flushing every open session's writer.
5. `visibilitychange` visible: `setHandwritingBackground(false)`, `await awaitCompletion(uuid)`, reload with `loadHandwritingNote(uuid, true)` and re-adopt the snapshot before accepting input. Block input (canvas `disabled`) between hidden and re-adopt.
6. Unmount: `endSession` only after any in-flight flush settles; do not cancel a pending completion.
7. Domain events: on `pages_changed` containing the UUID, refresh the status query (A5) and the page query; **never** replace the active draft from an event while a session is open.

**Accept:** unit tests on the registry + a jsdom test of the view covering: hidden→visible re-adopts with a new revision; Back with a failing flush keeps the page; Back with a successful flush leaves and completion error surfaces retryably. `vp test` green.

### A4. Viewing, creation gating, lists (TS)

1. `canDraw` = handwriting availability (`useHandwritingAvailability().available`) OR `mouseEnabled`. Without it the note opens read-only: `loadHandwritingNote(uuid, false)`, canvas `disabled`, toolbars hidden, a one-line hint "Connect a pen or enable mouse drawing in Settings to edit". Opening must never depend on a detected pen.
2. Page lists (`pages-list.tsx`, `all-notes-view.tsx`, `home-view.tsx` recent lists, search results if they render page icons): pen icon (`PenLine`) for `kind.kind === "handwriting"`, `FileText` otherwise. Find the shared place if one exists (a `PageIcon` helper is acceptable, new file under `src/features/pages/`).
3. `use-notes-workspace.ts`: `recordOpenedPage` currently only for `"note"`; include `"handwriting"` so recents work. Delete confirmation text: handwriting notes use "Delete note?" like text notes.
4. Journal/document layout switching must not be offered for handwriting pages (the core rejects it anyway; do not show the control).

**Accept:** a handwriting note without a pen opens read-only; lists show the pen icon; recents include handwritten notes; tests for the icon helper and read-only gating (pure logic).

### A5. Status and conflict resolution (TS)

1. Query `handwritingNoteStatus(uuid)` under a new `queryKeys.handwritingStatus(uuid)`; invalidate on `pages_changed` (for that UUID) and `sync_status_changed` in `src/lib/query.ts`.
2. Header indicator, subtle: "Unsent changes" while `unpublishedChanges || publicationRequested`; nothing when clean. No permanent footer.
3. `heads.length > 1` → non-blocking banner "This note has N versions from other devices" with **Compare**. The conflict dialog (Base UI Dialog, `src/features/handwriting/handwriting-conflicts.tsx`): before opening, finish gesture, flush, `requestCompletion` and await it, then re-read status. Render every head as a card: `deviceName`, `modifiedAtMs` formatted as date-time, a read-only `InkCanvas` (or a static render — reuse the canvas in `disabled` mode) fed by `previewHandwritingVersion(uuid, versionUuid)`; `available === false` → placeholder "Not downloaded yet", never an empty drawing, and the card cannot be chosen. Actions: **Keep this one** per card (keep = [that uuid]); **Keep all as separate notes** (keep = all uuids, first stays in place); **Later** (closes, no resolve). Pass **all** current head UUIDs in `expectedHeads`. On a stale/conflict error: re-read status, re-render, show "Versions changed, choose again". After success: invalidate page, pages, status; reload the editor snapshot (`awaitCompletion` then `loadHandwritingNote(uuid, editing)`).
4. Dates: `modifiedAtMs` is epoch milliseconds; `Page.createdAt/updatedAt` use the existing units — do not mix.

**Accept:** pure reducer/helpers for the dialog state are unit-tested (N heads, unavailable heads unselectable, stale error path); the dialog renders in jsdom with mocked commands; `vp test` green.

### A6. Settings text and prototype leftovers (TS + docs)

1. `handwriting-preference.tsx` description: keep semantics, drop "scratch"/"local draft" wording if present anywhere in `src/`.
2. Update `docs/planning/handwriting-integration.md` "Current implementation boundary" with one paragraph: frontend integrated, what was deferred (multi-page, infinite canvas, OCR — unchanged list from the handoff).
3. Do **not** delete or migrate the old `handwriting/ink-v1.sqlite3` on the device from the app; nothing in the code should reference it anymore (grep `ink-v1`).

**Accept:** grep shows no scratch-draft wording; docs updated; gates green.

### A7. Install and hardware acceptance (user + reviewer; executor stops here)

Executor: stop and report with a summary of A1–A6 and the exact commands used. The user then: backs up the device `notes.db`, deploys the server, installs the APK, runs the handoff's manual checklist (two handwritten + one text note; write/erase; Undo/Redo after restart; Back and background; offline completion and relaunch; sync to a second device; two-device conflict keep-one/keep-both; delete with backup and restore; DPI/orientation change; no lasso jumps, lost strokes or resurrected erased ink on the physical e-ink).

---

## Track B — Standards-based fixes (device-independent)

### B1. Android manifest configuration changes

`src-tauri/gen/android/app/src/main/AndroidManifest.xml`: add `density|fontScale` to the main activity's `android:configChanges`. Reason: changing per-app DPI or font size on the tablet recreated the Activity twice, wry's re-create path produced an un-navigated WebView and a white screen. Verify `arm64` debug APK still builds (the command used before is in `docs/planning/handwriting-prototype.md`).

**Accept:** manifest diff is one attribute; build succeeds.

### B2. Reduced motion (CSS)

In `src/index.css`, one global block:

```css
@media (prefers-reduced-motion: reduce) {
  *,
  *::before,
  *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
    scroll-behavior: auto !important;
  }
}
```

Keep `.markdown-mermaid__spinner` rule (now redundant, harmless). Loading spinners (`animate-spin`) become static under reduced motion; that is acceptable here, Track C replaces them properly.

**Accept:** `vp check` green; visually verified with tauri MCP screenshot using a reduced-motion emulation if available, otherwise on device later.

### B3. Hover-revealed controls are reachable without hover (CSS + TSX)

1. Add to `src/index.css`:

```css
@media (hover: hover) {
  .reveal-on-hover {
    opacity: 0;
  }
  .group:hover .reveal-on-hover,
  .group:focus-within .reveal-on-hover,
  .reveal-on-hover:focus-visible {
    opacity: 1;
  }
}
```

2. Replace every `opacity-0 group-hover:opacity-100` (and `opacity-0 … focus:opacity-100` variants) usage — there are ~12 across `outliner/block-node.tsx`, `outliner/outliner.tsx`, `outliner/block-tree.tsx`, `pages/page-view.tsx`, `pages/pages-list.tsx`, `pages/all-notes-view.tsx`, `markdown/image-viewer.tsx`, `home/home-view.tsx` — with `reveal-on-hover`. On `hover: none` devices the controls are simply visible.

**Accept:** grep finds no `group-hover:opacity-100`; `vp check`/`vp test` green; screenshot with tauri MCP in a touch emulation or on device shows the outliner chevrons/menus visible.

### B4. Smooth scrolling respects reduced motion (TS)

Add `src/lib/motion.ts` with `scrollBehavior(): ScrollBehavior` (returns `"auto"` when `matchMedia("(prefers-reduced-motion: reduce)").matches`, else `"smooth"`), use it in `use-notes-workspace.ts` and `chat-card.tsx`.

**Accept:** unit test for the helper with a mocked `matchMedia`; gates green.

---

## Track C — Display profile and design tokens

### C1. Native display kind (Kotlin + Rust)

1. Plugin `getMobileSystemInfo`/`MobileSystemInfo`: add `displayKind: "eink" | "lcd" | "unknown"`. Android: `"eink"` when `Build.MANUFACTURER` equals `ONYX` (case-insensitive), else `"lcd"`. Desktop/other: `"unknown"`.
2. Add `colorPanel: boolean | null` — `true`/`false` only if the ONYX SDK exposes a documented query for color panels (look in the SDK's `Device`/`DeviceFeatureUtil`/`EpdController` for a color-capability method; do not use reflection into hidden APIs); otherwise `null`. If nothing documented exists, leave `null` and note it in the report.
3. Regenerate bindings.

**Accept:** Rust/TS types updated, bindings regenerated, gates green.

### C2. Display profile state and settings (TS)

1. `src/app/appearance.tsx`: add `displayProfile: "auto" | "standard" | "eink"` and `inkColor: "auto" | "color" | "mono"` persisted in `localStorage` (keys `tangleaf.display-profile`, `tangleaf.ink-color`). Resolution (pure, tested): `resolveDisplay({profile, inkColor, info}) → {display: "standard" | "eink", color: "color" | "mono"}`; auto → eink iff `info.displayKind === "eink"`; inkColor auto → mono iff `info.colorPanel === false`, else color. Set `document.documentElement.dataset.display` and `dataset.inkColor` with the resolved values.
2. Tailwind variants in `src/index.css`: `@custom-variant eink (&:is([data-display="eink"] *));` and `@custom-variant mono (&:is([data-ink-color="mono"] *));`.
3. Settings → Appearance: two selects below the palette picker: "Display" (Auto / Standard / E-ink) and "E-ink color" (Auto / Color accents / Monochrome), the second only enabled when the resolved display is eink. Include both in the configuration export/import next to `palette` (see `configuration-transfer-section.tsx`).

**Accept:** unit tests for `resolveDisplay`; settings round-trip; gates green.

### C3. Semantic tokens for non-color properties (CSS + TSX)

1. Add tokens to `:root` and expose them through `@theme inline` so utilities exist: `--shadow-panel`, `--shadow-popover`, `--shadow-floating` (map current `shadow-lg`, `shadow-xl`, `shadow-2xl` usages), `--surface-glass` (opacity used by `bg-background/55`, `bg-card/55`, `bg-background/70`, `bg-background/95`), `--motion` (multiplier for `duration-*`/`transition` durations), `--radius` (exists). Use CSS `color-mix()` with `var(--surface-glass)` for the glass surfaces via a small set of component classes (`.surface-glass`, `.surface-glass-strong`) instead of per-element alpha utilities.
2. Replace usages: all `shadow-(lg|xl|2xl)` → semantic shadow utilities; `backdrop-blur-*` → a `.surface-glass*` class that includes the blur; radial gradients (`--app-viewport-background`, `.canvas-surface`) and `.brand-button` gradient go through tokens (`--surface-gradient-strength`).
3. Under `[data-display="eink"]`: shadows `none`, glass opacity `1`, `--motion: 0`, gradient strength `0`, `--radius: 0.25rem`, `--border` and `--input` set to `oklch(0.55 0 0)`, `--muted-foreground: oklch(0.4 0 0)`, `--ring: oklch(0 0 0)`, `--background/--card/--popover/--sidebar/--canvas/--inspector` pure white, `--foreground` pure black, `--secondary/--muted/--accent` white with black foreground (states get inversion in C4). Keep `--primary` and `--destructive` from the active palette (eink stays colorful). Under `[data-ink-color="mono"]`: `--primary: black`, `--primary-foreground: white`, `--destructive: black`.
4. Focus ring under eink: `2px solid` black outline instead of the 3px translucent ring (adjust the shared `focus-visible:` classes in `components/ui/*` through a token `--focus-ring`).

**Accept:** grep finds no raw `shadow-(lg|xl|2xl)`, `backdrop-blur`, `bg-background/\d+`, `bg-card/\d+` outside `index.css`; screenshots via tauri MCP for standard and eink (`document.documentElement.dataset.display = "eink"` set via `webview_execute_js`) attached to the report; gates green.

### C4. E-ink interaction rules (CSS + TSX)

1. Pressed/selected/active states under eink use inversion: extend the existing `.ink-dialog`/`[data-ink-editor]` toolbar rule to a general `eink:` treatment for `aria-pressed="true"`, `data-selected`, `[data-highlighted]`, active sidebar item, selected search result: black background, white text.
2. Dialog overlay under eink: opaque white overlay (no `bg-black/50` dither) plus a 1px black border on the content.
3. Spinners: introduce `<BusyIndicator label>` component; under eink it renders the label text (or "Working…") statically; elsewhere the existing `Loader2 animate-spin`. Replace the `animate-spin`/`animate-pulse` usages listed by `grep -rn "animate-\(spin\|pulse\)" src`.
4. Sonner: under eink `duration` 6000 and no enter/exit motion (it already honors reduced motion; verify).
5. CodeMirror: `drawSelection({ cursorBlinkRate: 0 })` under eink in `continuous-document-editor.tsx`; textarea caret cannot be controlled — leave.
6. Typography under eink: body `font-weight: 500`, `text-[10px]` labels raised to `text-xs`.
7. Code and diagrams: Shiki `github-light-high-contrast` for eink color; for mono choose a monochrome theme from `@shikijs/themes` if one exists (check the package list) else create a minimal custom Shiki theme object (black/italic/bold only) in `syntax-highlighter.ts`; Mermaid `neutral` theme under eink. Selection through the same resolver as light/dark.

**Accept:** screenshots via tauri MCP for eink color and eink mono on the outliner, a document page, settings, a dialog, search; gates green; tests for `BusyIndicator` and theme selection.

### C5. Handwriting under mono (TS)

`ink-model.ts` stroke rendering: when `data-ink-color="mono"` (pass a `mono` flag into the canvas renderer via props, not by reading the DOM inside the renderer), map stroke colors to gray by relative luminance. Today strokes are black only, so this is a small hook; keep the data unchanged.

**Accept:** unit test for the luminance mapping; gates green.

---

## Track D — Native refresh orchestration (after C; hardware verification required)

### D1. Display mode stack (Kotlin)

New `DisplayModeStack` in the plugin owning the WebView default update mode: layers `base` (profile), `session` (ink editor), `transient` (gesture). Semantics: effective mode = topmost set layer; setting/clearing any layer re-applies the effective mode; clearing the last layer restores the raw integer captured before the first layer was set (keep OnyxInk's raw restore, it preserves the firmware's "inherit" sentinel). `OnyxInk` switches to `stack.set(SESSION, GU)` / `stack.clear(SESSION)` and its transient logic to `stack.set(TRANSIENT, ANIMATION_QUALITY)` / `clear` (keep `applyTransientUpdate`/`clearTransientUpdate` calls as they are if those are a different mechanism than the view default mode — read the code; the stack only owns the view default mode). Unit tests like `InkRefreshPolicyTest`.

### D2. Profile and refresh commands (Kotlin + Rust + TS)

1. `setDisplayProfile(eink: boolean)`: eink → `stack.set(BASE, REGAL)` if `getViewDefaultUpdateMode` reads back REGAL after setting, else `GU`; standard → `stack.clear(BASE)`. Report the effective mode in the result.
2. `requestFullRefresh()`: `EpdController.invalidate(webView, UpdateMode.GC)` (or the documented equivalent for a View). Frontend: under eink, call it debounced (300 ms) after workspace target changes and after any Base UI dialog/popover closes.
3. Frontend calls `setDisplayProfile` whenever the resolved display changes and on startup.

### D3. Hardware verification (user + reviewer)

Executor stops and reports. Verify on the tablet: REGAL acceptance, ghosting after 10 navigations with and without the GC call, ink session still GU, restore on close/background/EinkWise/rotation.

---

## Out of scope (do not do)

- Paged scrolling, infinite canvas, multi-page handwritten notes, OCR/recognition, search over handwriting.
- Auto-detecting a monochrome panel if the SDK has no documented query (leave `colorPanel: null`).
- Any EinkWise/EAC configuration writes.
- Changing the compact-layout threshold, WebView text zoom, or DPI handling.
- Dark variants of the eink profile.
- Binary/Arrow IPC for strokes.
