# Handwriting input prototype

The first experiment is one device-local sheet opened with **New note → Write by hand**.
It tests input and visible ink latency before introducing synced handwriting content.
The existing Markdown note model and server API are unchanged.

## Availability

- Android enumerates native input devices for `SOURCE_STYLUS`, pressure, and tilt.
  Plugin events refresh the snapshot when devices change; resume and window focus also refresh it.
- Other platforms currently return `unknown`. A real browser pen event enables the entry point
  for the current process. Native desktop tablet enumeration is not implemented yet.
- **Settings → Handwriting** provides device-local automatic, always-visible, and hidden choices.
  Automatic mode hides the extra menu when no pen has been detected.
- Disconnecting a device never closes the open sheet.

## Sheet

The shared canvas accepts pen Pointer Events, including coalesced samples, pressure and tilt.
It ignores touch input to avoid palm marks. Mouse drawing can be explicitly enabled in Input details.
The eraser removes whole strokes; hardware eraser input uses the same behavior. Cancellation drops
the unfinished gesture, and undo/redo is local to the open editing session.

Completed strokes are saved after each gesture. The versioned JSON draft is kept at
`<app-data>/handwriting/draft-v1.json`, separate from notes, sync, and workspace backups.
It stores vector points rather than a screenshot. Saves are serialized, revision-checked, and
published through a synced temporary file and atomic replacement. A failed save retains the
current in-memory draft for retry and prevents Done from closing the sheet. Invalid saved data
is reported rather than overwritten. An interrupted, unfinished stroke is not guaranteed to survive.

This initial sheet is 1000 × 1400 logical units, limited to 150,000 points and 16 MiB on disk.
There is no ONYX acceleration, recognition, note insertion, or handwriting sync in this experiment.

## Device check

1. On BOOX, confirm the New note menu appears automatically and Input details reports a pen.
2. Write a few Russian lines at normal speed, then vary pressure and tilt.
3. Rest a palm on the page, lift the pen, write dots, and cross the canvas edge.
4. Erase a stroke, undo, and redo. Test the hardware eraser if the pen has one.
5. Use Done, reopen the sheet, then restart the app and check the saved handwriting.
6. Evaluate visible delay with the physical display; screenshots and synthetic pointer tests
   cannot establish pen-to-ink latency.
7. On Android without a digitizer, confirm automatic mode hides the menu. On desktop, test a
   real tablet, including pressure and reconnect behavior; mouse testing is not a substitute.

If normal canvas rendering feels too slow on BOOX, compare ONYX SDK rendering on the same sheet.
Once writing is comfortable, send a rendered image through the configured completion provider
and selected chat model. Then add durable handwriting attachments, versioned transcripts,
local FTS integration, and sync as a separate feature increment.

## Validation

Automated checks cover capability visibility, palm filtering, pointer cancellation, coordinate
mapping, pressure on pen lift, sparse-stroke erasing, serialized saves, failed-save retry,
revision conflicts, and preserving the saved file when new data is invalid.

Initial implementation validation: `vp check`, 372 frontend tests, the two Rust draft-storage
tests, and an arm64 debug APK build passed. Strict Clippy currently reports existing warnings
in `notes-blob/src/lib.rs` (`chunks_exact_to_as_chunks`) and `settings/transfer.rs`
(`collapsible_if`, `field_reassign_with_default`). The affected crates pass with those three
lints allowed; no unrelated source changes were made to silence them.
