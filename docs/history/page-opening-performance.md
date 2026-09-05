# Page-opening responsiveness — 2026-09-05

## Scope and method

Pixel 8 Pro, note “расположение вещей”: 385 visible outline rows and ten images.
Measurements used CDP against an APK with production frontend and optimized Rust:
`CARGO_PROFILE_DEV_OPT_LEVEL=3 CARGO_PROFILE_DEV_DEBUG=1 vp run tauri android build --debug --apk --target aarch64 --split-per-abi`.
Debug assertions and WebView inspection distinguish this from release.

The same visible Dashboard note button was clicked programmatically. A temporary
MutationObserver counted mounted rows and title availability; requestAnimationFrame
tracked main-thread frame opportunities, and PerformanceObserver recorded tasks
longer than 50 ms and resource timings. Hooks were disconnected after each run.
No note contents, settings, or image caches were reset. First-open results mean
first opening in that app process, with the existing disk image cache, not a cold
import. These are individual runs, not statistically established percentiles.

The first rAF callback and DOM changes are not proof of physical display
presentation. Human touch dispatch and compositor frame pacing are not measured
by this harness. A CPU-profile run is explicitly identified below.

## Baseline

Full mounting below the 400-row virtualization threshold made this note expensive
to open, even when all data was in memory:

| Scenario                 | First rAF | Rows present        | Longest task |
| ------------------------ | --------- | ------------------- | ------------ |
| First opening            | 73.5 ms   | All 385 at 897.6 ms | 719 ms       |
| Reopen                   | 729.2 ms  | All 385 at 728.9 ms | 730 ms       |
| Reopen with CPU sampling | 674.0 ms  | All 385 at 673.7 ms | 674 ms       |

On the first opening the title and loading indicator entered the DOM at 73 ms.
The ten cached image requests started around 999 ms and took 15–19 ms each.
Reopens made no new resource requests. CPU samples included Markdown/GFM setup,
Base UI Select, panel-size reads, DOM work and garbage collection; this does not
establish exact inclusive cost percentages for those components.

## Changes and repeated checks

`34191d8` yields an initial paint with an outline skeleton, then mounts batches
of 16 rows. A shallow-memoized static row wrapper prevents unchanged rows from
re-rendering with each batch; callbacks are not omitted from custom comparators.
Rows remain mounted after loading, preserving the earlier scroll strategy.
Pages above 400 rows still use the existing virtualizer. Small pages do not
need progressive loading. Navigation to an editing target keeps it available.

| Scenario                     | First rAF | First 16 rows | All rows  | Longest task    |
| ---------------------------- | --------- | ------------- | --------- | --------------- |
| First opening after batching | 65.6 ms   | 256.5 ms      | 1147.4 ms | 70 ms           |
| Reopen after batching        | 24.4 ms   | 51.2 ms       | 834.9 ms  | None over 50 ms |

`49db140` pauses pending batches on scroll/pointer activity, resuming after
180 ms of quiet. This trades full-document completion time for responsiveness
while the user interacts with the already visible part.

| Scenario                         | First rAF | First 16 rows | All rows  | Longest task |
| -------------------------------- | --------- | ------------- | --------- | ------------ |
| First opening with gesture pause | 72.3 ms   | 234.1 ms      | 1127.1 ms | 77 ms        |
| Reopen with gesture pause        | 50.4 ms   | 79.9 ms       | 913.8 ms  | 51 ms        |

During an early programmatic smooth scroll to 1200 CSS px, 48 rows were initially
mounted, 64 were present 150 ms later, and all 385 were present after completion
and resumption. No tasks over 50 ms were observed in that scenario. One batch can
finish before the first scroll event; the feature does not preempt executing JS.
Returning to Dashboard during loading left zero outline rows and no loading
indicator after 1.2 seconds. The loading animation respects reduced motion.
After full loading, a smooth scroll to 3500 CSS px and back retained all 385 rows
and produced no tasks over 50 ms. This is not a compositor or human-fling test.

`0e8c329` prevents a small page growing past 16 rows from hiding its existing
content and restarting progressive loading; it has a focused regression test.
The final frontend suite passed all 358 tests; formatting, lint and type checks
also passed. Production frontend builds and both intermediate optimized-debug
Android builds succeeded.

## Interpretation and boundaries

This change improves time to feedback and first content, not total work: the last
rows can finish later than before. It avoids the large synchronous initial render
without reinstating per-scroll row unmounting. It does not make every mount batch
fit the 8.3 ms budget of a 120 Hz display, nor optimize Markdown parsing itself.
Further parser/menu optimization needs a separate measured change; image-protocol
or cache changes were not warranted by these warm-cache measurements.

The final arm64 release build also succeeded and was installed with application
data preserved. Native screenshots confirmed the page header with the skeleton,
followed by the note's text and images. The process remained alive during this
check. Numeric measurements above are from the optimized diagnostic builds,
not from release screenshot timing.

The first launch immediately after APK replacement sometimes exited; repeat
launch worked. This behavior was also present in the baseline. Native
`FORTIFY: pthread_mutex_lock called on a destroyed mutex` messages were observed
with diagnostic builds, but the final release's exit history reported
`EXIT_SELF`, status 0. These observations do not establish a common cause.
The startup issue has not been diagnosed or classified as fixed by this work.
