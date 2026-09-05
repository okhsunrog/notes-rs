# Plan: inline image previews without scroll jank

## Context (from the 2026-07 scroll investigation)

Scroll experiments on `exploring-perf-issue` introduced nonvirtual rendering for
≤400 blocks and compositor promotion (`14927b9`). Residual intermittent jank was
also reported in other apps; its cause has not been established. Image-heavy pages
(e.g. "расположение вещей") need separate cold-load and warm-scroll measurements.

Key facts already verified:

- Attachments are served by `src-tauri/src/attachment_protocol.rs` (782 lines) via the
  `notes-attachment:` scheme → `http://notes-attachment.localhost`.
- A preview pipeline PARTIALLY EXISTS there: `preview_width`, `AttachmentImageCache` (DB,
  `IMAGE_CACHE_FORMAT_VERSION`), dimension probing, and the `image` crate (=0.25.10) with
  `webp` feature is already a dependency. Blob cache dir: `image-cache-v1` (`lib.rs:149`).
- Frontend: `src/features/markdown/use-attachment-images.ts` resolves uuids → URLs;
  `src/features/markdown/image-viewer.tsx` renders inline `<img loading="lazy" decoding="async">`
  and a fullscreen viewer.

## Goal

Inline (in-text) images are cheap thumbnails: small resolution, fast-decoding format,
**known dimensions up front** (zero layout shift on scroll). Full-quality image only in the
fullscreen viewer.

## Tasks

1. ✅ **Audit findings (2026-07-23)**: pipeline was mostly built already —
   inline `<img>` gets `/preview` (≤1024px) with `width`/`height` attributes set
   (aspect-ratio derived pre-load, CSS `height:auto` keeps it → no layout shift);
   fullscreen loads `/original`. Two REAL gaps found:
   (a) the `image` crate encodes WebP **lossless only** → photo "previews" were
   hundreds of KB–1.5MB with slow decode;
   (b) previews were generated lazily on first protocol request → decode+resize+encode
   of a 12MP JPEG (~200-500ms) landed mid-fling on first scroll.
2. ✅ **Lossy previews**: added `zenwebp` (pure Rust), q=80, RGB8/RGBA8 by
   alpha; `IMAGE_CACHE_FORMAT_VERSION` bumped 1→2 (invalidates DB rows + immutable URLs);
   cache dir renamed `image-cache-v1`→`image-cache-v2`, old dir deleted at startup.
3. ✅ **Warm preview cache**: `resolve_descriptors` now spawns sequential background
   generation of missing previews right after page load (`warm_preview_cache`). This
   does not guarantee that generation finishes before the first scroll.
4. ✅ Fullscreen viewer keeps `/original` (unchanged, verified).
5. **Tune loading during scroll** (only if still needed after 2+3): measure on
   "расположение вещей" first; `loading="lazy" decoding="async"` stays as-is otherwise.

## Validation gates

### 2026-09-05 follow-up

- Current unoptimized Android build: Resource Timing for 10 previews ranged from
  1.216 to 44.236 seconds. Repeat image load plus `decode()` took 8–29 ms; the
  first three originals (1280×964) took 229–275 ms. These are end-to-end timings,
  not isolated Rust encoding costs; browser-cached and cold results differ.
- Fix: workspace-scoped per-source locks, fresh DB cache lookup after locking,
  at most two generators and at most one background job. Detached request tasks
  keep locks through cache publication even when a caller cancels. HEAD reads
  now verify cached bytes too, allowing stale warm-ups to repair missing files.
- Timers: request queue, blocking-pool queue, read, decode, resize, encode,
  blob install, DB publication and total time. Encoder remains q80/method4.
- Regression: concurrent foreground/background requests with stale descriptors
  generate only once, and corrupted cache regenerates. Registry and slot limits
  also have a focused test.
- Device validation completed with optimized Rust on Pixel 8 Pro:
  temporarily renamed only `image-cache-v2` to
  `image-cache-v2-before-perf-20260905` (retained backup; originals and DB untouched).
  This tests missing-file recovery with existing DB metadata, not a fresh-import
  background warm-up. Ten distinct sources each generated once, all foreground.
  Rust totals: 189–276 ms/image; read 5–21 ms, decode 13–32 ms, resize 41–58 ms,
  encode 108–133 ms, blob installation 10–35 ms. DB publication 0.46–3.22 ms.
  The ten browser requests completed 1.192–2.375 s after their start; the last
  completed 3.275 s after clicking the note. This is not an isolated A/B of the
  scheduler: Rust optimization and cache conditions differ from the old run.
- Warm reopen: all ten loaded; resource durations 27–39 ms, about 895 ms from
  navigation click to the polling check observing all loaded (50 ms resolution).
  Fullscreen ultimately loaded `/original` at 1280×964. Four programmatic smooth
  scrolls (1500/3500/6500/0 CSS px) produced no new image requests and no long
  tasks. This does not establish compositor frame pacing or human-fling quality.
- Validation: `vp check`, 349 frontend tests, 30 Rust tests (one ignored), fmt.
  Strict Clippy is blocked by an existing `chunks_exact_to_as_chunks` warning in
  `crates/notes-blob/src/lib.rs:93` under Rust 1.98. With only that lint allowed,
  `cargo clippy -p notes-rs --lib --tests --locked -- -D warnings -A clippy::chunks_exact_to_as_chunks`
  passes. Unrelated generated manifest whitespace was left untouched.
- Optional follow-up: compare method0/1/4 on the same sources. No encoder setting
  change was made without measurement, and no cache version bump is needed here.
- Profiling build (production frontend, optimized Rust, debug WebView):
  `CARGO_PROFILE_DEV_OPT_LEVEL=3 CARGO_PROFILE_DEV_DEBUG=1 vp run tauri android build --debug --apk --target aarch64 --split-per-abi`.
  This is not identical to release: debug assertions and instrumentation remain.

- `vp check` && `vp test`; `cargo fmt --check` + `cargo clippy` + `cargo test` in `src-tauri`.
- On-device: scroll "расположение вещей" — no layout shifts, no decode hitches mid-fling
  (feel + the CDP trace harness from the investigation: Paint/commit should stay flat).

## Guardrails

- Do not change the attachment storage format or originals; previews are a derived cache.
- Keep the CSP (`img-src ... notes-attachment: http://notes-attachment.localhost`) intact.
- Stop and report before schema/DB migrations beyond the existing image-cache table.

## Out of scope

- Attribution of intermittent jank reported in other apps.
- Svelte migration discussion.
- Merging `testing-perf-improvements` (memo/lazy-picker fixes) — separate decision.
