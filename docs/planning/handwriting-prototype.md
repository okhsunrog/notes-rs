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

Completed strokes are saved after each gesture in `<app-data>/handwriting/ink-v1.sqlite3`,
separate from notes, sync, and workspace backups. Canonical CBOR metadata and lossless
INKCHNK/PCO-8 columns are SQLite BLOBs. A serialized blocking worker checks the expected
root revision, reuses unchanged chunks, and atomically publishes the next root using WAL/FULL.
Failed transactions preserve the saved page; failed saves retain the in-memory draft for retry.
Unfinished gestures are not guaranteed to survive. There is no JSON storage/import/fallback;
the original test corpus remains exported separately for compression benchmarks.

The independent `ink-format` crate owns codecs, core typed bodies and graph validation.
The app adapter currently retains normalized f64 input without quantization and stores one
chunk per changed stroke. Larger-chunk compaction and durable history are subsequent work;
current Undo/Redo lasts for the open editing session. Data placement and chunk-size policy
remain outside the crate. See [ink-format-v1.md](ink-format-v1.md) for the full design.

This initial sheet is 1000 × 1400 logical units, limited to 150,000 points and a 64 MiB
encoded snapshot budget (not a hard total SQLite/WAL disk quota).
Recognition, note insertion, and handwriting sync are not included. BOOX acceleration is an
optional native rendering path described below; the portable canvas remains the fallback.

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

Compare the portable canvas and ONYX SDK rendering on the same BOOX device.
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

## BOOX fast-ink experiment

The WebView-only trial on Note Air 4C accepted pressure and handwriting, but the user found
normal-speed writing uncomfortably delayed. The next experiment uses ONYX Pen SDK 1.5.4.3
(the publisher's Maven metadata was checked on 2026-09-05), through the existing Android plugin.

A separate SurfaceView is not required. `TouchHelper` binds directly to the existing WebView
with `FEATURE_SF_TOUCH_RENDER`. This selects the vendor system rendering path and native raw
input reader. The DOM sends the canvas rectangle and its visible clipping rectangle; the
native adapter converts CSS coordinates using actual WebView width, rather than assuming
Android density matches the BOOX per-app display settings.

While the pen is down, the vendor draws transient ink on the physical display. Completed
native point lists are normalized into the same portable draft coordinates, pressure, tilt,
and timestamps as Pointer Events. JavaScript does not also record those pen gestures. The
shared canvas draws completed strokes and the existing draft writer saves them. It renders full
replacements offscreen and publishes them with one canvas copy, without resetting the visible
backing store for each stroke. A Chromium visual-state callback establishes readiness for the
next draw; an Android frame-commit callback then confirms that a frame has been rendered and
submitted. Only then does the adapter call `EpdController.handwritingRepaint` for the sheet's
visible region, keeping raw drawing enabled. Submission is not a physical display-present fence.
A 120 ms quiet period groups updates between gestures. Stroke sequences, frame revisions, and
submission generations reject stale callbacks, including undo with an unchanged stroke sequence.
This handoff requires Android 10+ and hardware rendering; otherwise native setup falls back.

This is a two-stage rendering experiment, not a second note format. The SDK fountain brush and
our portable pressure brush have different shaping algorithms, so a small change in stroke
appearance on reconciliation is possible. Toolbar erasing and hardware erasing use the existing
whole-stroke eraser after a native point list arrives; fast continuous eraser previews are not
implemented. Native input batches are bounded by the same draft budget.

The adapter pauses on Activity pause or window focus loss, resets the app's temporary palm
rejection region, and closes the SDK on sheet exit. Session identities and callback generations
reject events from closed sheets. Non-BOOX devices and SDK startup failures retain Pointer Events;
Input details reports the chosen renderer and startup errors. AndroidX Jetifier is needed for
legacy transitive SDK artifacts. The native C++ runtime is packaged once, following the vendor demo.

Before declaring this path usable, test on the physical display:

- Verify Input details says BOOX Pen SDK, and compare writing at normal speed.
- Check pen position near every edge, pressure, dots, and fast consecutive letters.
- Wait for reconciliation, then undo/redo, scroll, close/reopen, and restart to check persistence.
- Rest a palm, test the hardware eraser, rotate the screen, open the notification shade, and return
  from another app. Confirm the normal interface remains responsive after leaving the sheet.
- Screenshots and automated events cannot establish physical pen-to-ink latency.

Sources inspected locally under `~/tmp_zfs/OnyxAndroidDemo` (commit `689ff7f`) and
`~/tmp_zfs/onyx-sdk-inspect` (published AARs and class inspection):

- https://github.com/onyx-intl/OnyxAndroidDemo/blob/master/doc/Onyx-Pen-SDK.md
- https://github.com/onyx-intl/OnyxAndroidDemo/blob/master/app/OnyxPenDemo/src/main/java/com/onyx/android/eink/pen/demo/scribble/ui/ScribbleWebViewDemoActivity.java
- https://github.com/onyx-intl/OnyxAndroidDemo/blob/master/app/OnyxPenDemo/src/main/java/com/onyx/android/eink/pen/demo/scribble/ui/ScribbleTouchHelperDemoActivity.java
- https://repo.boox.com/repository/maven-public/com/onyx/android/sdk/onyxsdk-pen/maven-metadata.xml

The Git repository contains examples and documentation; the Pen SDK implementation is distributed
as compiled Maven artifacts. The README and older documentation list older dependency versions;
the inspected published API additionally exposes explicit renderer selection and refresh controls.

Runtime integration found two additional requirements: Tauri permissions must explicitly allow
native `register_listener`/`remove_listener`, and the BOOX firmware APIs must be accessible before
the SDK's static Device initialization. Like the vendor demo, the adapter uses HiddenApiBypass
(6.1), restricted to `android.onyx` and `android.view.View` APIs. Without it, Android 13 blocks
firmware calls and the SDK can report a created helper despite an empty coordinate mapping.
The adapter also checks the exposed digitizer coordinate range before enabling native ink.

### Verified on Note Air 4C (2026-09-06)

The arm64 debug APK was installed with package data retained. Native status reported available
and active, the SDK reader found `onyx_emp_Wacom I2C Digitizer`, and real pen input reached the
portable draft through SDK callbacks. The user confirmed that normal-speed writing became
comfortable. This confirms the physical latency improvement qualitatively; no latency in
milliseconds was measured. Completed real SDK strokes were checked in the on-device draft file.

One measured fragment contained 11 strokes / 3442 points; within-stroke intervals were most often
2 ms. SDK timestamps in this run were Unix epoch milliseconds. Pressure values were consistent
with discrete 0..4095 levels before normalization. A lossless columnar fixture of the normalized
snapshot was exported for the separate codec benchmark; original native coordinates and size
were not captured separately.

`vp check` and all 377 frontend tests (71 files) passed. The debug Android APK build passed.
Release shrinking, other BOOX firmware versions, and full rotation/notification-shade/eraser
acceptance remain outside this first successful pen-latency check.

### Intermittent word disappearance follow-up

The user subsequently reported words briefly disappearing and returning after pen-up. The first
implementation paused raw rendering, invalidated the entire WebView in DU mode, and resumed
immediately. It also incorrectly treated the visual-state callback as a submitted frame and
reset canvas dimensions on every draft update. The revised handoff above removes these sources
of a blank transition. Tests cover uninterrupted canvas publication and stale frame/pen/lifecycle
callbacks: 378 frontend tests and four native frame-fence tests pass. The updated arm64 debug
APK was installed with existing strokes retained. During the physical-display retest, the user
reported no longer noticing the disappearance. Native status remained active and recorded
16 repaint calls, confirming the revised handoff executed during the trial. This is a successful
qualitative check on this device, not a guarantee across firmware versions or all interactions.

API references:

- https://developer.android.com/reference/android/webkit/WebView.VisualStateCallback
- https://developer.android.com/reference/android/view/ViewTreeObserver#registerFrameCommitCallback(java.lang.Runnable)

### Selection, erasing, and paper

The sheet now supports freehand/rectangular lasso selection, dragging the selection, copying,
scaling, and deletion. Pixel erasing splits vector strokes at intersections with a swept round
brush; stroke erasing deletes intersected strokes, and lasso erasing deletes strokes intersecting
or inside the enclosed region. Clear sheet is undoable. With the pen tool selected, the hardware
eraser uses the most recently selected eraser mode and size. There are no separate layer erasers
because the prototype has a single handwriting layer.

Plain/grid paper is stored separately from strokes in the draft. Existing drafts default to
plain paper. Grid spacing is 25 logical units, aligned to the sheet rather than the viewport;
erasing never removes the grid. The field is backward compatible when loading old drafts, but
older application builds do not accept this new field.

Portable and BOOX input share editing geometry. BOOX writing and freehand lasso use fast native
ink; the latter is transient and never becomes a stored stroke. Dragging an existing selection,
rectangle selection, and eraser previews use throttled native move events (at most once per 32 ms),
while completed operations use the full SDK point list. Static sheet bitmaps are cached across
contour updates; contours and ink are still published together. A cancelled gesture discards its preview; a completed edit creates
one undo entry. Partial erasing interpolates pressure, tilt, and time at cut endpoints. Point
budget checks reject an oversized edit without discarding the original handwriting.

The new native preview/selection path still needs physical-display acceptance on the device;
automated tests cover both Pointer Events and native event routing, including hardware erasing.

On-device follow-up: the user confirmed that selection, movement, erasing, and live contours
work. They then reported selection borders moving out of step with handwriting, and inadequate
contrast for active tools. Selection/gesture overlays now render into the same staging bitmap
as the ink, so one canvas publication contains both. Drag completion retains the same anchor
as its live preview. Active tools, modes, paper and size choices use black/white contrast;
transitions are disabled in the handwriting toolbars. After installing the refinement with the
saved draft retained byte-for-byte, the user confirmed on the physical display that the frame
and handwriting move together and the active tool is clearly visible.

### Freehand lasso latency and EinkWise follow-up

The earlier freehand lasso rendered through the throttled WebView preview path and redrew every
stroke on each update. It now uses the SDK pencil trace with no intermediate JavaScript preview;
the complete native point list selects strokes at pen-up. Selection bounds are sent to Android
so dragging within the selection keeps its existing bitmap preview instead of drawing a trace.
Regression tests cover transient native selection, propagation of selection bounds, and sheet
bitmap reuse/invalidation. All 388 frontend tests and four native frame-fence tests pass; the
arm64 debug APK builds. Physical acceptance of the new fast lasso remains pending.

The grid was present in canvas and screencap but effectively invisible on the physical panel.
ADB inspection of EinkWise on firmware 4.2-rel (2026-04-28) showed Customize / Speed, Vivid,
Original layout, High Contrast OFF, Anti-flicker 10 for Tangleaf. Switching only the profile from
Speed to Regal made the old grid visible, confirmed by the user before updating the APK.
Grid lines are also darkened from #c4c4c4 / 0.65 to #777777 / 1 logical unit for better contrast;
the resulting weight on Regal needs physical acceptance.

EinkWise opens for the foreground app via action.open.eink.center.request, verified from ADB.
The official SDK exposes getAppScopeRefreshMode/setAppScopeRefreshMode, distinct from persistent
EinkWise color, contrast, layout, and refresh profiles. Scoped view/gesture mode switching is now implemented experimentally as described below. SDK reflective setters can fail silently; verify actual behavior on firmware.
The device is left in Regal for this trial.

### Scoped display modes inspired by stock Notes

While the sheet owns the focused WebView, its previous view update mode is saved and GU
(16-level grayscale partial refresh) is requested. This is a view waveform request, not a
persistent HD/Regal EinkWise profile change. Closing the sheet, losing focus, or pausing the
activity restores the previous view mode; resuming requests a fresh, fenced repaint.

Software gestures (selection dragging, rectangular lasso, eraser previews) request transient
ANIMATION_QUALITY. Pen writing and the SDK freehand lasso do not start this request. Like the
stock Notes quiet-period policy, it is cleared five seconds after pen-up; a new pen-down
cancels the pending cleanup. Consecutive gestures share one request. Clearing it requests a
fresh WebView frame acknowledgement and handwritingRepaint even if the last content revision
was already presented. Configuration changes after a completed selection retain the pending
cleanup; geometry changes during a gesture cancel it and release the display modes.

EinkWise profiles, contrast, dithering thresholds and turbo settings are not changed. The
implementation uses documented SDK entry points for transient/view modes, rather than the
stock app's private EAC configuration pathway. Native status reports the queried view mode,
request counts, and the SDK return value for entering transient mode. These report firmware
calls, not measured physical display latency. Three new unit tests cover timing/lifecycle
ownership alongside the four frame-fence tests; all 388 frontend tests pass.

Physical acceptance of the transient mode and grayscale restoration on Note Air 4C is pending.

Installed on Note Air 4C with the saved draft retained byte-for-byte. Readback on the actual
WebView changed from GC (raw 5) to GU (raw 2) while the sheet was open, then returned to GC
(raw 5) after Done. The EAC profile dump remained byte-identical across installation and the
open/close check. Raw integer restoration preserves inherited/unknown firmware values which
SDK enum conversion can lose. Seven Android unit tests pass. Gesture acceleration and the
five-second quality restoration still await physical-display feedback; readback of a view
mode alone does not establish the effective waveform during a gesture.

### Drag continues after pen-up: measured preview backlog

The user reported continued movement after lifting the pen. A read-only WebView listener on
Note Air 4C recorded 41 rectangle-selection previews with median/max event delivery lag of
8/20 ms, followed by 92 drag previews with median/max lag of 1475/3696 ms. The JavaScript long
task observer also recorded tasks of 50–77 ms and a final 350 ms task. This identifies an actual
preview processing backlog; the five-second display-mode quiet period does not move vectors.

Native previews now retain only the latest event for the next animation frame. Begin, final
stroke, end, cancellation, and unmount discard any queued preview; final geometry always uses
the complete SDK point list. During dragging, the paper/unselected ink and selected ink are
rasterized once, then composited with the translated selection frame. Full vector rasterization
happens again at commit. This avoids repainting every stroke at the native event rate. Tests
cover a burst of 100 previews followed by final input, cached drag rendering, clamping, and a
single undoable final coordinate update.

The user confirmed on the physical display that movement now stops immediately after pen-up
and that dragging is substantially smoother and faster. The follow-up recording contained
eight gestures: median preview delivery lag per gesture was 13–28 ms, versus 1475 ms for the
previous measured drag. Per-gesture maxima were 30–395 ms. Final stroke delivery measured from
the last SDK point was 66–474 ms, and some final rasterization tasks still took 301–365 ms;
these are event-processing measurements, not pen-to-display latency. The multi-second preview
backlog no longer appeared in this trial. Timing listeners were unregistered after capture.

### Regional rendering and panel repaint

The canonical scene cache now redraws only changed ink bounds and overlapping strokes in
page order. Starting a drag removes selected ink from that cache; finishing rasterizes the
selected ink at its final vector coordinates, preserving the separate translated preview
bitmap during the gesture. Background/size changes still rebuild the scene. The staging
canvas is reused for region rendering, then an opaque, pixel-aligned patch is copied back;
clipping vector paths directly changed Skia antialiasing near clip edges in a device probe.
A 35-stroke / 19,400-point offscreen WebView comparison tested seven repeated moves and a
deletion at 0.5/1/1.37 scales: maximum channel difference versus full redraw was 3/255,
with the small mismatch count stable after the second operation, and exact pixels at 1.37.
This is a renderer probe, not a physical latency measurement or proof of bitwise identity.

The frontend accumulates damage from ink changes and old/new decorations until a frame
acknowledgement is requested. Native code unions those bounds with SDK trace points and
retains them across superseded acknowledgements. Only a successfully submitted, current
frame consumes that damage. Handwriting repaint is limited to its visible intersection,
with a safety margin; no-op frames do not request a panel repaint. The five-second cleanup
uses the union of areas affected during the transient interval. Focus/size/background
transitions retain full repaint fallback. Native status includes lastRepaint, repaintedPixels
and visibleCanvasPixels for diagnosing the actual requested area.

The arm64 debug APK was installed on Note Air 4C and the saved JSON was byte-identical across
installation. Notification shade, EinkWise, Home/return, landscape/portrait, and Done/reopen
were exercised with ADB on the installed build. Lost focus/closed sheet reported inactive,
quality ownership false and the previous raw view mode 5; return reported active with GU/raw 2. Rotation retained the dialog and updated canvas bounds. The saved EAC profile dump was
byte-identical, rotation settings were restored, and USB stay-awake remains enabled. The
SDK logs some unavailable optional reflective APIs on initialization; no application crash
occurred during these checks. Physical acceptance of the new regional gesture cleanup and
its performance remains pending. Validation: 392 frontend tests and nine Android unit tests.

### Eraser profiling and incremental saves

The user confirmed the regional lasso cleanup left no old ink or selection frames. A second
trial still showed 250–331 ms final long tasks. Those should not be attributed solely to
rasterization: a read-only Android bridge probe with the same 2.3 MB snapshot measured
346–383 ms synchronous invocation overhead, while plain JSON.stringify took 8–10 ms.

A CPU profile of hardware erasing identified cutIntervals/eraseGesture and allocation/GC as
major gesture costs. Stroke and swept-capsule bounding checks now skip provably unrelated
geometry; whole-stroke mode no longer builds unused pixel fragments. No additional gesture
simplification is introduced. In 450 randomized comparisons against the previous algorithm,
the resulting geometry matched (new fragment UUIDs normalized for comparison). A read-only
WebView benchmark of ten swept paths over the 35-stroke corpus measured 60–125 ms before and
0.2–12.1 ms after for geometry alone, not end-to-end panel latency.

Native begin/preview/cancel handlers also now receive the hook's latest acknowledged draft,
including stroke batches which arrived before React rendered. A regression test writes a
new point and immediately starts hardware erasing elsewhere in the same batch: unrelated
new ink must survive; a subsequent hit must erase it. The user reported a newly written word
not disappearing under the hardware eraser; whether this race explains that observation
still needs the updated physical retest.

Saving sends an incremental IPC patch with page order, changed strokes and background.
Unchanged stroke points stay in Rust. Whole-stroke deletion sends no point arrays, reducing
a representative bridge payload from 2301833 to 1370 bytes; the read-only bridge probe then
measured 1.2–5.3 ms synchronous work. Disk storage remains the original complete JSON draft.
The backend checks the acknowledged revision, reconstructs and validates the full document,
and uses the same atomic write. The frontend advances its comparison snapshot only after
success, preserving retries, queued edits and Undo. New Rust tests cover unchanged points,
order/deletion/restoration, stale writes, duplicate/missing IDs and invalid geometry.

Validation before hardware retest: 396 frontend tests, five handwriting storage Rust tests;
native renderer unchanged since the nine passing Android tests. The proposed CBOR/column
storage draft was reviewed separately; no storage migration was performed.

### Fresh words remained visible after erasing: display reconciliation

The hardware retest recorded two new pen strokes and six hardware eraser strokes. Both new
words were removed from the canonical canvas and persisted JSON; the resulting file was
byte-identical to the pre-test draft. Neither the canvas export nor the Android screenshot
contained the words, while the user still saw them on the physical panel. A fenced repaint
of the full visible canvas through handwritingRepaint alone did not remove them. This
isolates a fast-ink/display reconciliation failure, not an eraser geometry or save failure.
The same recording contained no JavaScript long task over 50 ms after the preceding optimizations.

Stock Notes GrayscaleRefreshAction and the SDK partial-refresh example mark the submitted
buffer with HAND_WRITING_REPAINT_MODE. Tangleaf now holds this mode from invalidation through
frame submission, then restores the sheet's GU mode. Cancellation, pen-down and display-mode
release also unwind the temporary mode; no early raw-layer clear is introduced. A new unit
test covers idempotent acquisition/release. Ten Android tests pass and the arm64 APK built
and installed with byte-identical draft data. Device readback at submission was raw 524290,
then GU/raw 2 afterward. Physical acceptance of fresh-word erasing remains pending.

### Density and font-scale changes without Activity recreation

The Android manifest now declares density and fontScale in configChanges. This keeps the
existing Activity/WebView through these changes; Tauri already forwards configuration
updates, while the handwriting ResizeObserver and window resize handlers update canvas
resolution and native pen geometry. The reported white-screen incident involved repeated
Activity recreation; the exact earlier renderer failure has not been reproduced under a
debugger. Tauri issue 15671 concerns a different task-removal/foreground-service trigger,
so it is not treated as proof of the same root cause or a reason to recreate windows blindly.

On the Note Air 4C, the installed arm64 build retained the same Activity, CDP page and a
JavaScript sentinel through font scale 0.85 -> 1.0 -> 0.9 -> 0.85, EinkWise app density
450 -> default 300, system densities 300 -> 360 -> 400 -> 320 -> 300 (400 -> 320 separated
by approximately 150 ms), and restoring EinkWise to 450. JavaScript and plugin IPC stayed
responsive; the open handwriting dialog survived, canvas dimensions adapted and Pen SDK
reported active. System-density testing with EinkWise pinned to 450 was separately checked
and did not count as a change to the application's density. The complete EinkWise profile
and persisted draft were byte-identical before and after testing; system density and font
scale were restored. Validation: vp check, 396 frontend tests and the arm64 debug APK build.
This does not prevent an external force-stop or cover every other Activity recreation path.

### Hardware eraser must pause firmware pen rendering

The handwriting-repaint buffer flag alone did not fix the physical retest. Two new strokes
and twelve hardware eraser strokes were recorded; the persisted draft again matched the
pre-test draft byte for byte. Switching the toolbar from Pen to Eraser, without modifying
the document, made the already-erased words disappear on the physical display (user confirmed).

Stock Notes EraseFinishAction retains raw input, then ScribbleHandler routes EraseFinishEvent
through InvalidateViewWithPenControlAction: pause raw rendering, invalidate, resume. The
pause calls TouchHelper.setRawDrawingRenderEnabled(false), which leaves scribble mode and
sets firmware pen state PEN_PAUSE. Tangleaf already did this for the toolbar eraser, but
hardware erasing while Pen remained selected omitted the transition.

An eraser render gate now pauses only firmware rendering at erase-down, retaining raw input
for software geometry. It stays paused until an accepted canvas frame is submitted, or a
new pen-down supersedes the pending erase frame. Tool settings determine the restored render
mode; lifecycle shutdown clears the gate without enabling rendering. Normal pen strokes do
not acquire this pause. Tests cover repeated erasing, frame completion, an immediate return
to writing and lifecycle cancellation. Validation: twelve Android unit tests, vp check and
the arm64 debug APK build pass.

The updated APK passed the physical retest: the user confirmed that fresh words now erase
quickly and correctly with the back of the stylus while Pen remains selected. The recording
captured nine writing strokes and eight hardware eraser strokes; afterward Pen SDK remained
active and the eraser render gate was released. Diagnostic listeners were stopped after
the retest. This supersedes the failed fresh-word erasing retests above.

## SQLite columnar storage validation — 2026-09-06

The independent `ink-format` crate now supplies typed core bodies and full snapshot
validation. `handwriting/storage.rs` stores canonical CBOR and INKCHNK/PCO-8 BLOBs
in one SQLite transaction. The JSON reader/writer and fallback are removed.

Validation at `3c2b0e1`:

- Crate: 19 integration tests and one doctest, including an independent copy outside
  the workspace with fresh registry resolution; package creation succeeded.
- App: 45 Rust unit tests passed (one unrelated corpus import test remains ignored),
  including seven handwriting tests covering float bits, CAS, removal/restoration,
  immutable chunk reuse and rollback when a trigger aborts the head update.
- `vp check` and all 396 frontend tests passed. Crate Clippy is clean. App-only Clippy
  passes with the existing unrelated `collapsible_if` warning excluded; a fully
  strict workspace check also hits an existing `notes-blob` `chunks_exact` lint.
- Arm64 debug APK built and installed on BOOX a72aa394. A two-point test stroke was
  saved through the real WebView IPC, loaded exactly, and survived force-stop/relaunch
  with identical root revision, coordinates, pressure/tilt/time and grid background.
  SQLite integrity check returned `ok`; the DB contained six CBOR records and one
  787-byte INKCHNK BLOB. This small save took 33.4 ms including IPC; it is not a full-page
  performance benchmark or a power-loss test.
- The temporary stroke was removed, and the sheet reopened with zero strokes, Grid
  selected and Saved status. The old device JSON draft was removed after taking an
  additional local copy; the original exported benchmark corpus is retained.

Physical pen/eraser/lasso acceptance on this storage build is pending. Rendering and
BOOX refresh code were not changed in this step. Durable Undo/Redo, compaction and
integration into regular notes remain subsequent work.

## Persistent handwriting history

SQLite schema 2 adds `ink_history` and `ink_cursor`; an existing SQLite page becomes
its initial history state, without importing any JSON. Each acknowledged gesture
creates one state. Fifty transitions are retained, including background changes.
Undo/Redo moves the persistent cursor, and a new edit after Undo discards the redo
branch. GC pins all retained roots. The revision is now a unique transition token,
not a physical root hash; returning to an earlier page cannot revive a stale CAS.
Record IDs are verified against their content-derived identity during reads.

The frontend queues every completed gesture, including during a slow/failed save.
It flushes that queue before navigating history and briefly blocks input during the
navigation. History navigation does not serialize old strokes back through IPC.
Atomic snapshot publication, fifty-action retention, SQLite schema upgrade and
ABA/conflict behavior are covered by storage tests. Compaction must preserve these
roots and the current revision token.
