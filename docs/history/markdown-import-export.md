# Markdown Import/Export + Block Statistics Plan

> Historical development record. Task ordering and completion claims reflect the
> original investigation, not a current execution plan. Verify against current code
> before using outstanding items. Source paths are relative to the repository root.

Self-contained handoff plan. Run AFTER the current `search-and-fixes.md` execution finishes (A6 and M5 both touch Settings; A5 touches search UI). One commit per task, in order. If something doesn't match this plan: stop and report.

## Architecture decision (fixed — do not revisit in this plan)

**All plain-Markdown import and export flows through the frontend `DocumentCodec`** (`src/features/document/document-codec.ts`). Rationale: the codec is the single grammar defining how markdown becomes blocks; routing import through it makes `import(file) ≡ paste(file)` true by construction and avoids a second parser that would drift. The Rust Logseq importer (`crates/notes-import`) is NOT affected — Logseq's outline format is a different grammar and stays server/CLI-side. Server-side plain-MD parsing is out of scope permanently unless cross-language conformance fixtures are introduced first.

Consequence: codec normalizations are import semantics. M1 fixes the one that loses data (code language) BEFORE import ships.

## Global rules

1. Gates per task: `vp check`, `vp test`; Rust-touching tasks also `cargo fmt --all -- --check`, `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings`, `cargo test --workspace --locked --all-features`.
2. `src/lib/bindings.ts` is generated — regenerate with `vp run bindings:generate` after Rust command changes, never hand-edit.
3. JS deps via `vp add`, Rust deps via `cargo add`. Tauri plugins need both sides plus capability entries — follow the pattern of existing plugins in `src-tauri/`.
4. Do not modify `workspace-model.ts`, the sync engine, or `crates/notes-import`.
5. Commit style: `feat(document): …`, `feat(settings): …`.

---

## M1. Code blocks keep their language

`BlockStyle` is `{ kind: "code" }` with no language; the codec's parser drops the fence info string and `projectCode` emits a bare fence — pasted/imported ` ```rust ` loses its language on round-trip, so Reading-mode highlighting dies.

1. Extend the style to `{ kind: "code", language: string | null }` end-to-end: Rust `BlockStyle` (find its definition in `crates/notes-core`; check how styles are persisted — likely JSON — and whether a migration is needed for existing rows; absent field must deserialize as `null`), regenerate bindings.
2. Codec: `parseBlockNode` captures the info string's first word (trimmed, lowercase) from `FencedCode`; `projectCode` re-emits it after the opening fence. `semanticSignature` must include the language (a language change is a real edit).
3. Renderer: pass `style.language` through to the existing shiki highlighter path for code-style blocks (verify how `rendered-block`/`markdown-components` currently pick a language for fenced content _inside_ block markdown — do not regress that path).
4. Outliner style menu: unchanged UI; language is set implicitly by typing/pasting fences, not by the menu. Do not build a language picker.
5. Tests: codec round-trip preserves ` ```rust `; fence-within-code still escapes via longer fences; old persisted `{kind:"code"}` loads as `language: null`.

## M2 (decision task — small). Headings 4–6

The codec clamps `####`–`######` to `heading_3`. Two options: (a) extend `BlockStyle` with `heading_4..6` end-to-end (schema, projections, style menu icons), or (b) keep the clamp but document it as import semantics. Implement **(b)** now — one doc paragraph in ../architecture/editor.md and a codec test pinning the clamp — and report (a) as a follow-up candidate with an effort estimate. Do not implement (a) without a go-ahead.

## M3. Export to Markdown files

1. Add `@tauri-apps/plugin-dialog` (+ matching Rust plugin + capabilities) for save/open dialogs; file writing may use the dialog plugin's save path plus a small Rust command if no fs plugin is present — prefer a typed Rust command (`export_page_markdown(uuid) -> String` already effectively exists as codec input; writing the file can be a command taking path + content) over adding a broad fs capability.
2. Per-page action (page menu): "Export as Markdown".
   - Document-layout pages: `DocumentCodec.encode(blocks).markdown` — the canonical projection, verbatim.
   - Outline-layout pages: project as nested bullets — `- ` with two-space indent per depth (match `listContinuationIndent`), task states as `- [ ]`/`- [x]`, one blank line between top-level blocks only if a block is multi-line. Define this projection as a pure tested function next to the codec.
   - Filename: sanitized page title (or journal date), `.md`.
3. Bulk export: Settings action "Export all pages" → directory picker → one file per page, journals under `journals/`, report count + failures. Reuse the per-page functions; no zip.
4. Tests: pure projection functions (document and outline) golden-tested; no dialog mocking — separate the pure part from the shell.

## M4. Import Markdown files

1. Entry point: Settings section "Import" (next to the existing Logseq import) → "Import Markdown files…" → multi-select `.md` open dialog → read contents (dialog plugin / small Rust command).
2. Per file, in the frontend:
   - **Frontmatter**: detect a leading YAML (`---`)/TOML (`+++`) block; extract `title` (string) if present; drop the rest of the frontmatter from the imported body. Log dropped keys into the import report — do not invent a properties model.
   - Title: frontmatter `title` → else filename stem.
   - Body → the codec's parse path (`parseMarkdown` → units, i.e. the reconcile path with an empty previous map — reuse, don't fork, the codec internals; export a thin `parseDocument(markdown)` helper from the codec module if needed).
   - Create a document-layout page, then persist via the existing `replacePageDocument` flow.
3. Report UI (reuse Logseq-import report patterns): per-file ok/failed, dropped frontmatter keys, and a notice that relative links/images are not resolved (media import is out of scope).
4. Duplicate titles: titles are unique by design (they are `[[link]]` identities — the DB enforces a unique normalized index). On collision with an existing page or another imported file, suffix the title (`"Title (2)"`, `"Title (3)"`, …) and record the renaming in the import report. Never steal or blank an existing page's title.
5. Tests: frontmatter extraction (YAML, TOML, absent, malformed → treated as body); title fallback; a fixture file round-trips import → export with only the documented canonical normalizations (M2 clamp etc.).

## M5. Settings: block statistics page

Diagnostic page for embedding/chunking behavior. Client-side proxy metric: **block markdown length** (the true embedding input adds a server-side composition header; the exact view stays with the server inspector in EMBEDDINGS_PLAN E6 — link that in the page's caption).

1. New Rust command `block_statistics() -> BlockStatistics`: total blocks, empty count, and a fixed-bucket histogram of `length(markdown)` in chars (buckets: 0, 1–25, 26–100, 101–400, 401–1000, 1001–2000, 2001–4000, 4001–8000, >8000), plus top-20 longest blocks as `{blockUuid, pageUuid, pageTitle, chars}`. One SQL pass; no new state.
2. Settings → "Diagnostics" section → "Block statistics" page: render the histogram as simple CSS bars (no chart library), with visual threshold markers at 2,000 ("splits into sub-chunks") and 8,000 ("hard cap") and one-line explanations of what each threshold means. Top-20 list links each row to the page (open via the existing workspace controller).
3. Refresh button; computed on demand, not live.
4. Tests: Rust command bucket edges (0, exactly 25, exactly 2000, >8000); TS presentation logic as pure functions.

## M6 (ordering exception: implement BEFORE M4). Render HTML blocks visibly

The codec files HTML blocks as `paragraph` units, and the Reading-mode renderer then hides their content entirely (`skipHtml` + sanitize) — the text survives in the DB but the user sees nothing. Imported MD files with HTML fragments will hit this immediately.

1. In the shared markdown renderer, render raw-HTML nodes as **escaped visible text** styled as code (inline `<code>` for inline HTML, a code-styled block for HTML blocks) instead of dropping them. Never render actual HTML — the sanitization posture (`skipHtml`, `rehype-sanitize`, url/image policies) must not weaken; this is a presentation change only.
2. Tests: an HTML block shows its escaped source in Reading mode; script/iframe content stays inert (rendered as text, present in DOM only as text nodes); existing sanitizer tests unaffected.

---

## Out of scope

- Media/attachment resolution on import; link rewriting; Obsidian vault semantics (folders → namespaces).
- Properties/frontmatter model beyond `title`.
- Any server-side markdown parsing.
- Heading 4–6 model extension (M2 option (a)) without explicit approval.
- Charting libraries for M5.

## Final verification

Full `vp` + cargo gates green; bindings drift check clean; manual smoke: export a document page → import the file → visually identical page (modulo documented normalizations); report per-task commits and deviations.
