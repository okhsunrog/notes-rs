# Handwriting integration boundaries

The accepted workflow distinguishes durable local gesture writes from published
note versions. New geometry stays PCO-8. Every five seconds with changes, on leaving
the note, and on backgrounding, publish a saved note version and enqueue sync.
There is no Save button. Recovery must not depend on receiving a shutdown callback.

## Current implementation boundary

The scratch sheet still has its own database and is not a synced note. Fresh-only
packing and the five-second maintenance worker do not implement note autosave.
Do not describe a maintenance completion as a synchronized note save.

## Integration constraints found in the current repository

- `notes-core` owns notes.db migrations and an asynchronous SQLite worker. The
  scratch adapter must not set its own application_id or user_version on that DB.
- The main connection uses synchronous=NORMAL; the scratch sheet uses FULL. A
  migration must explicitly preserve the chosen durability semantics, rather than
  silently inheriting a weaker setting for ink writes.
- Scratch `ink_head`, `ink_cursor`, and `ink_history` are singleton tables. They
  must become document-scoped before supporting more than one handwritten note.
  Record/chunk GC must pin roots across all documents and retained local histories.
- Current synchronization operations cover pages, text blocks and attachments.
  A new ink version needs an explicit domain representation; do not hide a binary
  root ID in markdown or send every stroke as a text operation.
- Existing blob upload streams files. SQLite-resident ink blobs need an adapter
  for that transport (or a bounded export cache), not a second authoritative copy.
- Atomically publish a saved root and its sync outbox entry. Gesture-local drafts
  must remain recoverable without being advertised as the last published version.
- Physical compaction is not an edit: it must not change the user-visible revision
  or force re-upload of previously acknowledged unchanged geometry.
- Handle simultaneous remote/local edits explicitly; never overwrite a dirty local
  draft merely because a newer remote root was received.

## Validation required before installation of integrated storage

Migrate a copy of the current device DB first; retain the original until verified.
Check independent documents, deletion/GC with retained histories, save/outbox
atomicity, process interruption, offline queue recovery, and two-client sync.
Initial full snapshot loading remains supported; history navigation uses deltas.
The prototype's format crate remains independent of application database/sync code.
