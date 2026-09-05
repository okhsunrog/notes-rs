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

JSON is an interim input-test format. The agreed production direction is columnar stroke data,
with point offsets per stroke and separate coordinate, pressure, tilt, and time columns. Codec
and quantization choices are being benchmarked separately. Preserve native source precision and
store coordinate transforms/pressure ranges as metadata when comparing compression variants;
the prototype's normalized f64 values must not silently become the production encoding contract.

This initial sheet is 1000 × 1400 logical units, limited to 150,000 points and 16 MiB on disk.
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
shared canvas draws completed strokes and the existing draft writer saves them. A Chromium
visual-state callback acknowledges the canvas frame before the native adapter releases the
transient drawing mode to refresh it. A 120 ms quiet period avoids doing that during a stroke;
a sequence check prevents refreshing a frame which is missing a newer completed stroke.

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
