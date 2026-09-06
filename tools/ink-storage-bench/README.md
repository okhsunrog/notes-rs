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
The host runner accepts `run.mjs remote-directory output.jsonl [0|1] [forward|reverse]`.
Use a new remote directory for each suite. Reverse the order for a second paced
pair to help distinguish policy effects from execution order and device state.
Use a dedicated directory under `/data/local/tmp` on Android. Never pass notes.db
or the application's handwriting database. No networking is performed.

The fixture is append-only. Deletion/move/Undo/Redo interactions with the production
adapter and handwritten visual acceptance remain separate integration tests.

## Production-adapter compaction scheduling

`compaction page.json NEW.db none|exit|SECONDS [speed:0=unpaced,1=realtime] [max_chunks=16] [decoded_MiB=128]`

This second executable directly compiles the application's model/validation,
SQLite adapter, history and compaction modules. It does not copy their algorithms.
A small error shim replaces the Tauri command boundary. Geometry is always PCO-8.
Policies vary when the same production compactor is invoked. Calls default to
16 fresh blocks, 8 MiB of compressed input and 128 MiB of decoded column values;
`exit` does not imply combining a whole page into a single chunk. Exit drains
bounded jobs until no more packing succeeds. `none` is the no-packing baseline.
Timed policies arm after a completed stroke and continue while a pack succeeds;
new strokes rearm an idle worker. Recording timestamps control this schedule even
in speed=0 runs. Unlike the app, this driver serializes saves and compaction, so
paced event lag measures this harness, not WebView/pen latency or lock contention.

Counters cover production patch calls, compaction preparation/encoding/publication,
SQLite IO and history maintenance. Setup, input parsing, final DB inspection and
bitwise verification of every retained Undo state are excluded. Closed connections
follow the production adapter's lifetime, including SQLite last-close checkpoints.
`head_geometry_bytes` and `head_metadata_bytes` count only the current document;
`retained` also includes history. Neither is a measured network transfer size.
CPU time and kernel write counters are not energy or physical flash measurements.

Each run requires a fresh database. Final history verification navigates backward
through up to 51 states; these are disposable databases, not export artifacts.
Never run this program against app data. The checked-in production compaction tests
are also compiled by `cargo test -p ink-storage-bench --bin compaction`.

`node tools/ink-storage-bench/run-compaction.mjs /data/local/tmp/ink-compaction-UNIQUE NEW.jsonl`
executes none/5s/30s/60s/exit policies twice in opposite orders. Stage the release
Android binary as `bench` and the private fixture as `page.json` first. It writes
progress and appends each verified run immediately. The runner performs no app
installation or data mutations outside its dedicated test directory.

The runner also accepts optional `maxChunks decodedMiB` arguments (defaults
`16 128`). Use `512 16` for the larger-batch experiment. These are explicit
benchmark arguments, not environment settings honored by the installed app.
The adapter now inspects decoded value sizes before selecting input blocks;
highly compressible histories exceeding the budget are processed in partial jobs.
This is a decoded-data/work budget, not an RSS ceiling: encoded buffers, metadata,
output builders and codec scratch space need additional memory. Output chunks
still target 250,000 points, keeping whole segments; a larger input batch can
publish several output chunks in one transaction. Already sealed input is skipped.
`peak_rss_before_exit_kib` and `peak_rss_kib` are process high-water marks before
and after exit work; neither isolates allocation by the compactor.

The ignored release test `large_compressible_history_respects_decoded_budget`
requires `INK_STRESS_DEST` to be a new DB under an `ink-compaction-*` directory.
It generates 40 versions of a 75,000-point stroke (147 MB decoded across history),
packs them in 16 MiB jobs, checks continued progress, then verifies all history
states bit-for-bit and SQLite integrity. `INK_STRESS` reports compaction time,
maximum selected decoded bytes and peak RSS before verification. Generation and
verification are excluded from the timer; generation is included in peak RSS.
The input is artificial low-entropy data for budget testing, not a compression
ratio prediction for natural handwriting. Never point this test at application data.

### Exit-phase profiling

Build the same binary's test harness in release mode for Android:
`cargo test -p ink-storage-bench --bin compaction --release --target aarch64-linux-android --no-run`
(using the same NDK compiler/linker environment as the normal executable).
Run only the ignored `profile_recorded_exit` test with `--ignored --nocapture`.
`INK_PROFILE_SOURCE` must name a closed disposable benchmark DB without WAL/SHM;
`INK_PROFILE_DEST` must be a new path. Both paths must be absolute and their parent
basename must start with `ink-compaction-`. The test copies the source, restores
its latest retained history state, then times `prepare`, `repack`, and `publish`
separately. Setup/redo and final bitwise/revision checks are excluded. Output has
an `INK_PROFILE` JSON line. Repack includes decode, merge and PCO encode; publish
includes history remapping, persistence, GC, commit and connection teardown.
This profiling code exists only under cfg(test), not in the installed app.

The detailed profile also reports nested metadata/GC counters. These overlap with
outer prepare/publish times and must not be summed together. `INK_PROFILE_CHUNKS`
may be set to 2..512 for a phase-only batch-size experiment; the application and
normal scheduling benchmark still use 16. Encoded/decoded byte budgets and output
chunk limits remain in force. This knob does not select a new shipping policy.

`run-compaction-crashes.mjs REMOTE NEW.jsonl` uses a test harness staged as
`profile-crash` and the closed `run-0.db` baseline fixture. It copies that fixture
and sends SIGKILL to the isolated test process immediately before or after the
publication commit (using the experimental 512-block limit). A fresh process
recovers the WAL, checks SQLite integrity/foreign keys, compares every retained
history state bit-for-bit to the read-only source, and verifies the head/cursor
and unchanged user revision. No signal is sent to the installed application.
This checks process termination, not physical power failure. All fault injection
is cfg(test)-only and requires an explicitly enabled profiling session.
