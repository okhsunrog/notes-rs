# Handwriting integration boundaries

The accepted workflow distinguishes durable local gesture writes from versions
queued for synchronization. New geometry stays PCO-8 and completed gestures are
saved locally with their history. Leaving the note or backgrounding requests
publication and synchronization after pending local writes finish. There is no
additional five-second autosave/publication timer and no Save button. Persist
unsent changes for offline retry and recovery after an interrupted process;
recovery must not depend on receiving a shutdown callback.

Accepted compaction policy (2026-09-06): one bounded job every 60 seconds while
there is new geometry or unfinished packing; continued writing does not reset the
deadline. Leaving the note or backgrounding drains all currently useful jobs.
Use at most 512 fresh blocks, 8 MiB encoded input and 16 MiB decoded column values
per job, keeping the existing whole-segment/250,000-point output target. All
triggers share one compaction lane. Idle documents do not need recurring polling.
The minute interval does not trigger synchronization.

Before synchronizing an ink version, the required order is: finish its local write
queue, complete useful compaction, atomically pin the resulting document root and
enqueue that immutable version, then transfer missing records/chunks. This applies
to exit, background publication, manual sync and retry after restart. Compaction
failure postpones that version's publication; retain durable local writes and
pending publication intent for retry. Do not block unrelated text-note sync.
Already prepared versions can be retried without recompressing their sealed blocks.
Singleton or non-shrinking input is a successful no-op, not an infinite sync barrier.

Concurrent edits must not move the version being sent: validate the chosen logical
revision when atomically pinning its compacted root/outbox entry. Later edits form
the next unpublished version. Never upload mutable SQLite state by reading a live
head repeatedly. Back can leave after durable writes; compaction/network completion
runs in the background. Android may stop a background process, so the eventual
outbox/recovery protocol cannot rely on finishing an onPause callback.

## Storage and portable export decision (2026-09-06)

SQLite remains the application storage backend. Integrate handwriting into the
common notes database using separate immutable CBOR metadata records and INKCHNK
geometry BLOBs, connected by document roots. One note is a graph of records and
chunks, not one monolithic SQLite BLOB. Physical blocks may contain multiple
logical strokes without reducing Undo granularity. Sync should transfer missing
blocks in batches rather than equating each database row with a network request.

A future self-contained single-note import/export container will package the
root, required metadata, geometry and resources. Its name, extension and container
layout are deliberately undecided; INKDOC was only a discussion placeholder.
INKCHNK continues to identify an internal point-data chunk, not a complete note.
The container belongs with the independent format library when that work is
prioritized. It is not a prerequisite for common-database integration or sync.

There is no planned move from SQLite to individual note files. Keep the format
library independent of persistence so an alternative adapter remains possible,
without implementing a second working store or custom file journal now.

## Current implementation boundary (2026-09-06)

The backend now uses note-scoped records, history and publication state in the
common notes.db. The standalone scratch adapter and commands have been removed.
Causal InkPublish operations, missing-blob HTTP batches, server validation and
manual conflict-resolution APIs are implemented. Completion durably requests
publication, finishes packing, and atomically pins a version in the sync outbox.
Open editor bases are protected from replacement by incoming remote versions.
Workspace archives include binary ink, unpublished working copies and conflict
variants; restore publishes compacted versions transactionally.

The frontend is integrated. Panes route by `page.kind.kind`, so a handwritten note
is listed, opened, renamed, favorited, deleted and synced like any other note, and
the text editor is never mounted for it. One module-level session per note UUID owns
the writer and the pending completion, so leaving, backgrounding or unmounting the
view cannot strand unpublished work; reads wait for the completion the previous
session owes. Capabilities gate creating and drawing only: without a pen or the
mouse preference a note still opens read-only. Publication state is shown as unsent
changes, and concurrent published branches are compared and resolved manually.
Deferred, unchanged from the backend handoff: multi-page notes, infinite canvas and
OCR/recognition, including search over handwriting.

Pen, touch and the keyboard are separated by a pause registry rather than by
window focus. The firmware paints the pen into a screen region above every
window and the IME never takes our window focus, so nothing native can see it:
the plugin keeps a set of named reasons — the keyboard (read from the WebView's
window insets), a focused DOM field, an open dialog, popover, select or menu —
and re-arms the pen only once the set is empty, re-pushing the limit rect and
enabling input before render after 150 ms. `EpdController.setAppCTPDisableRegion`
is gone from the gesture path: it killed every touch in that band, which is what
made the soft keyboard unusable over a sheet, and palm rejection comes from the
limit rect instead. Nothing editable lives on the drawing screen any more: a new
note is named `Handwriting <yyyy-MM-dd HH:mm>` at creation and renaming happens
in a dialog, which pauses the pen while it is open. The editor chrome is one
toolbar row; per-tool options are in a popover anchored to the active tool.
Evidence for the input model: `/home/okhsunrog/tmp_zfs/reversed_onyx_notes_app/REPORT.md`
(the stock BOOX Notes app, decompiled) and
`/home/okhsunrog/tmp_zfs/reference_notes_apps/REPORT.md` (Notate, Notable,
PngNote, Saber, Mokke).

The installed BOOX build is still the preceding scratch prototype; its file has not
been migrated or deleted, and nothing in the application refers to it any more. See
[the backend handoff](handwriting-backend-handoff.md) for exact commands, lifecycle
requirements, verified boundaries and remaining limitations. In particular,
full snapshots currently fetch historical published graphs as well as current heads,
and historical published-body retention has no pruning policy yet.

The handoff's remaining sync limitation no longer applies: a batch the server
refuses outright no longer stalls the outbox behind it. The client resends the
batch one operation at a time to find the offender, quarantines that operation
with the server's own reason — it stays stored and is never deleted — and lets
every later change through. Sync status reports how many changes could not be
sent and offers to send them again, which releases the quarantine.

## Validation required before installation of integrated storage

Migrate a copy of the current device DB first; retain the original until verified.
Check independent documents, deletion/GC with retained histories, save/outbox
atomicity, process interruption, offline queue recovery, and two-client sync.
Initial full snapshot loading remains supported; history navigation uses deltas.
The prototype's format crate remains independent of application database/sync code.

## Proposed user experience

Handwritten notes should live alongside text notes in All notes and share titles,
favorites, navigation, deletion, and synchronization. A pen icon and a small ink
preview distinguish their contents. Recognition will later provide searchable
text for the whole sheet or note without replacing the original handwriting.

Keep the existing capability-aware Write by hand creation option. Opening and
viewing an existing handwritten note must work without a detected pen. Device
preferences may explicitly enable creation and mouse drawing.

The integrated editor should use the common note identity/navigation with a
compact Back/title header, drawing tools, contextual tool options, and a large
sheet. Back flushes pending local writes before leaving. Normal saving and input
diagnostics do not need a permanent footer; save failures remain visible and
retryable. Mouse drawing belongs in device settings. Until integration is real,
the integrated editor replaced it and its local-draft description.

For the first integrated version, prefer explicit sheets with vertical navigation
and an Add page action at the end. This preserves predictable page boundaries for
recognition, previews and export. Infinite paper and mixed text/ink content remain
separate design decisions; the UI cleanup does not settle their storage model.
