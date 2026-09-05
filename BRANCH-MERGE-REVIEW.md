# Performance branch merge review — 2026-09-05

Target: direct merge of `exploring-perf-issue` into `main`, without a PR or history rewrite.
`origin/main` was fetched and matches local `main` at review time.

## Commit decisions

| Commit                                                                      | Decision                                                                                                           |
| --------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `5c68935` synthetic scroll harness                                          | Removed by `bf5d966`, including the test rows and hash route. Retain history only.                                 |
| `14927b9` full rendering up to 400 blocks, compositor promotion             | Keep. Large pages still virtualize; fixed overlays must remain portaled outside the transformed workspace content. |
| `83d8963` Android SDK 37                                                    | Keep, as explicitly requested. Final release build is a gate.                                                      |
| `d19cb7f` lossy WebP previews and cache warming                             | Keep with the subsequent scheduling fixes. Originals remain unchanged.                                             |
| `a85b2cf` SSE UTF-8 handling                                                | Keep; split-multibyte network chunks have regression coverage.                                                     |
| `819321f` preview scheduling, touch and image viewer fixes                  | Keep. Includes cache recovery, bounded generation, fullscreen sizing and zoom reset.                               |
| `acdf2f1` Tauri/tao dev, AGP 9, window chrome, heartbeat and UI refinements | Keep as requested. Lockfile pins upstream revisions; no need to split into PRs.                                    |
| `9924151` diagnostic log cleanup                                            | Keep. Image timings use debug rather than info.                                                                    |

Do not merge `bb80b29` from `testing-perf-improvements` as-is. Its custom memo
comparators omit callback props (`onTaskStateChange`, picker callbacks), and
hover/edit-only picker mounting changes keyboard accessibility and the lifetime
of portaled menus. Reconsider separately if large-page profiling warrants it.

## Review follow-ups

- Stabilize the Markdown decoration test by completing CodeMirror's bounded
  background parse before asserting the entire fixture. Do not change production
  parsing or weaken the assertions.
- Fix UTF-8-unsafe slicing in attachment UUID extraction. Invalid references with
  Cyrillic or emoji must not panic and must not hide later valid references.
- Preserve native Android edge-to-edge and safe-area behavior. The earlier
  targetSdk 34 and bar-opacity experiments are not part of the final diff.
- Keep debug-only MCP access for development; do not enable the bridge in release.
- Preserve the pre-existing generated AndroidManifest comment changes separately;
  they are not a functional fix and should not be accidentally bundled into a commit.

## Validation and remaining merge gates

- Frontend: `vp check` and all 350 tests passed after harness removal; all 350
  passed again after making the Markdown test wait for parsing.
- Rust: 34 application tests passed, one real-corpus test intentionally ignored;
  all eight notes-sync tests passed before the UTF-8 extraction follow-up.
- Attachment regression tests: all seven passed after `287c9cc`, including
  malformed Unicode references. Clippy passed with the narrow allowance below.
- Desktop release executable and arm64 Android release APK builds completed
  successfully. Desktop used `--no-bundle`; AppImage/deb packaging is not tested.
- The release APK was installed with data preserved on Pixel 8 Pro; the user
  confirmed that testing was successful.
- Desktop release reached `notes-rs ready` with a fresh isolated profile under
  Xvfb/X11 and ran until the 12-second test timeout. This is startup validation,
  not visual Wayland or accelerated-rendering validation. Existing user data was
  not used. Prior KDE visual checks were on the development build.
- Clippy currently uses a narrow allowance for the pre-existing Rust 1.98
  `chunks_exact_to_as_chunks` lint in notes-blob; no other warnings are allowed.
- The scoped build and smoke-test gates are complete with the limitations above.
  The branch is ready for a local direct merge; publishing remains a separate action.
