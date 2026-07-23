# Plan: inline image previews without scroll jank

## Context (from the 2026-07 scroll investigation)

Scroll stutter had three sources; two are fixed on `exploring-perf-issue`
(`14927b9`: no virtualization ≤400 blocks + `will-change` compositor promotion of
`[data-workspace-scroll]`; a third, floating system-wide jank, is an OS issue, out of scope).
The remaining app-level issue: **image-heavy pages (e.g. "расположение вещей") stutter more**
— inline images load/decode during scroll and can shift layout.

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
2. ✅ **Lossy previews**: added `webp` crate (native libwebp), q=80, RGB8/RGBA8 by
   alpha; `IMAGE_CACHE_FORMAT_VERSION` bumped 1→2 (invalidates DB rows + immutable URLs);
   cache dir renamed `image-cache-v1`→`image-cache-v2`, old dir deleted at startup.
3. ✅ **Warm preview cache**: `resolve_descriptors` now spawns sequential background
   generation of missing previews right after page load (`warm_preview_cache`), so the
   first scroll hits cached bytes.
4. ✅ Fullscreen viewer keeps `/original` (unchanged, verified).
5. **Tune loading during scroll** (only if still needed after 2+3): measure on
   "расположение вещей" first; `loading="lazy" decoding="async"` stays as-is otherwise.

## Validation gates

- `vp check` && `vp test`; `cargo fmt --check` + `cargo clippy` + `cargo test` in `src-tauri`.
- On-device: scroll "расположение вещей" — no layout shifts, no decode hitches mid-fling
  (feel + the CDP trace harness from the investigation: Paint/commit should stay flat).

## Guardrails

- Do not change the attachment storage format or originals; previews are a derived cache.
- Keep the CSP (`img-src ... notes-attachment: http://notes-attachment.localhost`) intact.
- Stop and report before schema/DB migrations beyond the existing image-cache table.

## Out of scope

- The floating system-wide jank (OS-level, affects Telegram too).
- Svelte migration discussion.
- Merging `testing-perf-improvements` (memo/lazy-picker fixes) — separate decision.
