# Ink storage policy benchmark

Private Android/Linux storage-only experiment. Uses the production `ink-format`
codec and SQLite WAL + synchronous=FULL. It does **not** call the application
storage adapter, WebView, Pen SDK, sync worker or rendering code. SQLite schema is
experimental: immutable geometry versions, CBOR history states, and an atomic
checkpoint/remap. Results cannot establish application latency or battery savings.

`ink-storage-bench page.json new.db raw|pco [speed] [crash-phase]`

- `speed=0`: no sleeping, but original sample timestamps still determine 5-second
  autosave batches. This is a throughput stress test, not real-time input.
- `speed=1`: original timing; long writes can delay later events. Reports maximum
  event lateness. This serial model does not simulate the application's worker queue.
- Both modes record completed strokes durably, retain 51 history states, and pack
  only fresh blocks at each autosave. Already sealed blocks are never repacked.
- Both currently repack even a single pending block, deliberately holding batching
  behavior constant. This is not an optimized special case for immediate PCO.
- Final save flushes the remaining batch, representing leaving the page.
- Initial JSON parsing and database setup, final validation and explicit final WAL
  checkpoint are excluded from timing/IO counters. Normal SQLite auto-checkpoints
  during the run are included. VmHWM includes initial JSON loading; it is process
  peak RSS, not incremental memory attributable to the codec. Each run should be a
  fresh process/database. `write_bytes` is kernel-accounted process storage IO,
  not physical flash writes/wear; `wchar` is logical bytes submitted to write calls.
- Every persisted point is checked bit-for-bit (including timestamps and negative
  zero), along with widths, segment identity, SQLite integrity and history references.
- `verify` instead of a mode checks a database without writing application data.
- Crash phases `before_pack`, `after_encode`, `before_commit`, `after_commit` kill
  this process at the first autosave; reopen with `verify`. This exercises SQLite
  process-crash recovery, not sudden physical power loss.

Input corpus is private and must not be committed. Existing databases are rejected.
Use a dedicated directory under `/data/local/tmp` on Android. Never pass notes.db
or the application's handwriting database. No networking is performed.

The fixture is append-only. Deletion/move/Undo/Redo interactions with the production
adapter and handwritten visual acceptance remain separate integration tests.
