# Android APK replacement and activity lifecycle

Investigation on 2026-09-05, Pixel 8 Pro / Android 17, package
`dev.okhsunrog.notes_rs`. Application data was preserved throughout.

## Relationship to Tauri PR 15949

[PR 15949](https://github.com/tauri-apps/tauri/pull/15949) fixes illegal activity
result launcher registration from `PluginManager.onDestroy`. The pinned Tauri
commit `270c63f117eb1f4ff0a653ca63b2ca61e9175663` includes its merge commit
`cb2ecac53a505b974ef24f0d4f66ac04fc298eff` (12 commits ahead). The checked-out
`PluginManager.kt` registers launchers per activity in `onCreate`; teardown only
removes the entry and selects a surviving activity.

## Initial reproduction

Installing the release APK over the foreground app and immediately issuing
`adb shell am start -W -n dev.okhsunrog.notes_rs/.MainActivity` reproduced the
exit. The new process was PID 5882:

- 20:28:49.781: shell requested the activity launch.
- 20:28:51.366: SystemUI requested another launch with flags `0x8000`, following
  its `PackageUpdateActivity`.
- 20:28:51.833: the first activity's window was removed.
- 20:28:52.344: Zygote reported that process 5882 exited cleanly, status 0.

There was no `IllegalStateException` from the PR's old code path in this
reproduction. This is a different observed failure signature in a related
multi-activity lifecycle scenario, not evidence that the PR caused a regression.

The runtime has a relevant path: Tao translates non-configuration activity
destruction to `WindowEvent::Destroyed`; Tauri removes that Rust window and
requests exit when its window map becomes empty. Temporary application lifecycle
logging is used to distinguish this path from a startup error.

## Instrumented reproduction

Temporary `MainActivity` instance/id logs and Tauri `RunEvent` logs confirmed
the path in process 8171:

- 20:37:30.979: activity A created, id 124390773.
- 20:37:32.691: activity B created, id 14138105, in the same task/process.
- 20:37:32.935: A destroyed with `finishing=true`, `changing=false`;
  its `super.onDestroy()` returned successfully.
- 20:37:32.936: Tauri emitted `WindowEvent { label: "main", event: Destroyed }`,
  then `ExitRequested { code: None }`, then `Exit`.

Thus Java launcher teardown is no longer throwing. The runtime still associates
the sole Tauri window with A; B's existence does not preserve it when A is
destroyed. This confirms the exit mechanism, not a safe general-purpose upstream
fix. Merely preventing `ExitRequested` would leave the Rust window removed and
is not an adequate repair.

## Installation workflow control

Installing the same diagnostic APK over the foreground app without immediately
issuing `am start` let SystemUI restore it normally. PID 9116 had one activity,
remained running, and displayed the dashboard and subsequently the target note.
A second install-only trial also restored one activity and remained running
(PID 9776), without a Tauri exit event.
This establishes an installation-workflow workaround for the observed race,
not a repair of the upstream lifecycle problem. After installing over a running
app, allow system restoration to complete before deciding whether a manual
launch is needed.

## Image viewer Back handling

The workspace's Android Back listener previously navigated without checking the
image dialog. An overlay dismissal stack now consumes Back before workspace or
settings navigation. Closing uses the viewer's normal reset path; registrations
are removed on close/unmount. Tests cover viewer close/reopen, nested overlay
priority and cleanup. The frontend suite passed 360 tests.

On the phone, opening an image followed by `KEYCODE_BACK` closed the fullscreen
viewer and retained the note and its scroll position, verified by screenshots.
A second Back press then returned to Dashboard. Temporary lifecycle logging was
removed from the source after diagnosis.

The final clean release APK was built and installed without a competing manual
launch. SystemUI restored the application successfully again (PID 10332).
