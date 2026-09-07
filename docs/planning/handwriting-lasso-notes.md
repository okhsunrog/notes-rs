# Lasso selection: what the stock BOOX Notes app does

Research notes taken before reworking our lasso, so the behaviour we copy is traceable.

Sources:

- stock app: `/home/okhsunrog/tmp_zfs/boox-notes-inspect/decoded/sources/com/onyx/android/` (jadx,
  largely unobfuscated; the `sdk/notecore` package is the Onyx note SDK bundled into the APK)
- Onyx SDK AARs: `/home/okhsunrog/tmp_zfs/onyx_sdk_decompiled/` (see its `REPORT.md`)

## a) How the lasso trace is drawn

**While the pen moves: firmware, `STROKE_STYLE_DASH`, dash parameter `5.0`.**

Entering selection mode activates `Provider.SCRIBBLE_SELECTION_PROVIDER`
(`note/action/selection/StartSelectionAction.java:55`). Every pen resume then runs
`ResumeRawDrawingRequest`, which for that provider configures the firmware renderer:

- `note/request/pen/ResumeRawDrawingRequest.java:91-93` → `j()` → `:57-59`
  `noteManager.setStrokeStyle(5)` (`TouchHelper.STROKE_STYLE_DASH`)
- `:102` → `i()` → `:52-55`
  `Device.currentDevice().setStrokeParameters(5, new float[]{5.0f})`
- `:103` → `k()` → `:61-65` also turns the eraser channel into the same dashed renderer while the
  selection provider is active (`setErasePathDrawing(true, 5)` →
  `sdk/notecore/editor/NoteManager.java:831-838` → `touchHelper.setEraserRawDrawingEnabled(true, 5)`),
  so the trace looks identical when drawn with the pen's eraser end.

No `setStrokeWidth` is issued for the trace — it inherits the active pen width. There is a
`SelectionTrackRenderer` in the SDK (`sdk/notecore/editor/render/SelectionTrackRenderer.java:37-42`,
dash `10/10`, stroke width `4`), but it is only registered for `InteractiveMode.SCRIBBLE_SELECTION_TRACK`
(`sdk/notecore/editor/render/RendererHelper.java:68`) and nothing in the app ever activates that
mode, so on this build the live trace is **firmware only**.

Our `OnyxInk.kt` already uses the same recipe (`setStrokeParameters(STROKE_STYLE_DASH, [5f])` plus
`setStrokeStyle(STROKE_STYLE_DASH)`), so the dash parameters were never the cause of a broken trace.

**After pen-up: an app-drawn dashed bounding box, no firmware involvement.**

`sdk/notecore/editor/render/SelectionRenderer.java`:

- `:223-232` `j()` strokes the selection rectangle path with `getDrawSelectionRectPaint`
- `:484-489` `getDashedTrackPaint`: `DashPathEffect({10, 10}, 0)`, `strokeWidth = 3.0`
- `:513-533` base paint: `Style.STROKE`, `strokeWidth = 2.0`, colour `renderContext.getLinePaintColor()`
  = `0xFF000000` (`sdk/scribble/shape/RenderContext.java:76`)
- the rectangle is the union of the selected shapes expanded by
  `SelectionRect.SELECTION_RECT_PADDING = 12.0` (`sdk/scribble/data/SelectionRect.java:44`, applied in
  `expandSelectionRect()` `:328-334`)
- handles are drawn on top: white-filled, black-stroked corner squares (±10 px, `:394-435`), edge
  circles (r = 10, `:459-481`) and a rotation bitmap (`:379-392`)

Those numbers are in device pixels of a ~1404 px wide page. Our sheet is 1000 units over the same
page, so `× 1000/1404 ≈ 0.71`: padding ≈ 8.5, stroke ≈ 2.1, dashes ≈ 7/7.

**Selected strokes are not highlighted.** No colour or width change anywhere in `SelectionRenderer`
— the dashed box (plus handles) is the entire affordance.

## b) The floating menu during a drag

There are two different "float menus" in the stock app and only one of them is ours:

|     | what it is                                           | class                                                             |
| --- | ---------------------------------------------------- | ----------------------------------------------------------------- |
| A   | the user's draggable **pen toolbar**                 | `note/menu/view/handler/FloatToolMenuHandler.java`                |
| B   | the **selection action menu** over a lasso selection | `note/menu/popup/SelectionPopupMenu.java` (a plain `PopupWindow`) |

`RefreshFloatToolMenuPositionEvent` and `CollisionAvoidanceUtil` belong to A only.

**B is repositioned on every (throttled) move, never hidden.**

1. `note/handler/scribble/SelectionHandler.java:2703-2718` pushes each move point into an
   `ObservableHolder`
2. `:1352-1362` buffers it by `selectionTransformTimeMs`, default **10 ms**
   (`sdk/note/ui/config/DeviceConfig.java:76`)
3. `:1386-1390` → `note/manager/SelectionManager.java:217-242`: after each `TranslateAction` it posts
   `UpdateSelectionPopupEvent`
4. `note/ui/ScribbleFragment.java:2004-2011` calls `SelectionPopupMenu.show(...)` →
   `note/utils/SelectionPopupMenusKt.java:55-60` `PopupWindow.update(x, y, w, -1)`

Nothing in the drag path hides it; pen-up only re-runs the same update
(`SelectionHandler.java:2741` → `:1185-1187` → `:593-606`).

**Geometry** (`note/utils/SelectionPopupMenusKt.java`):

- `:33-38` x: horizontally centred on the selection rect, clamped only on the left
- `:40-45` y: `margin` **above** the rect, flipping **below** when `rect.top < popupHeight + margin`
- `margin` = `note_popup_selection_menu_margin` = **8 dp** (`res/values/dimens.xml:2966`)

This is what `selectionMenuPosition` in `ink-editing.ts` already implements.

**B registers no firmware exclude rect.** The exclude-rect slots
(`sdk/notecore/editor/data/NoteDocViewInfo.addExcludeRect(type, rect)`) are: 1 float audio,
2 zoom preview, 3 float tool menu (A) `FloatToolMenuHandler.java:431`, 4 link-note jump,
5/6/8 canvas seekbar, 7 page navigator, 9 clipboard bar, 10 float compact icon. There is **no entry
for `SelectionPopupMenu`**, and it even overrides `windowChangeEvent` to a no-op
(`SelectionPopupMenu.java:187-189`) so it does not pause the pen the way other popups do
(`note/eventhandler/PenEventHandler.java:106,272` gate raw drawing on `isPopupWindowShowing()`).

Consequence: on stock the firmware handwriting region is never punched through by the selection
menu, so a lasso trace crossing the menu is drawn whole; the popup is a separate window and takes
the tap by itself.

While a selection with shapes exists, stock additionally hides toolbar A and drops _its_ exclude
rect (`FloatToolMenuHandler.java:800-808`).

## c) How the drag is rendered

App redraw per step, under a firmware fast mode — not a firmware transient bitmap:

- `note/action/selection/TranslateAction.java:67-92` re-runs the translate request and installs an
  overlay renderer (`RenderVarietyShapesAction.java:74-96` →
  `sdk/notecore/editor/display/RenderManager.java:543-547`), which calls `invalidate()` — a full
  `View` redraw of the note view per drag step.
- `StartTransformAction.java:227` posts `ApplyFastModeEvent(true)` and
  `QuitTransformAction.java:545` posts `ApplyFastModeEvent(false)`;
  `note/eventhandler/EpdEventHandler.java:109-116,176-179` turns that into
  `EpdController.applyTransientUpdate(ANIMATION_QUALITY)` for the whole app surface, with a 5 s
  auto-exit (`:124`).

Our pipeline is the same shape: JS redraws the moving ink into the canvas per preview frame while
`InkRefreshPolicy` holds `ANIMATION_QUALITY`.

## Diagnosis of the two reported defects

1. **Dashed trace breaks, top part missing.** `ink-canvas.tsx` published the floating selection
   menu's box as a firmware _exclude rect_ (`excludeRects: menuAt ? …`), which
   `OnyxInk.configure()` pushed into `TouchHelper.setLimitRect(limit, excludes)`. The menu sits just
   **above** the selection, so a new lasso drawn near an existing selection lost exactly the part of
   its trace that crossed that band. Stock registers no such rect (see (b)).
   Secondary, unrelated clipping to keep in mind: `limit.top` is also raised to `clipTop` (the
   viewport edge) in `OnyxInk.kt`, so a sheet scrolled partly out of view legitimately clips the
   trace at the viewport boundary.

2. **Menu teleports after the drag.** `menuAt` was derived from `draft.strokes`, which only changes
   on commit, so the menu could not follow the preview. Stock repositions its popup every ~10 ms
   during the drag.

## What we changed

- The selection menu no longer produces a firmware exclude rect. Its box is still handed to the
  plugin, but only as a _page overlay_: `OnyxInk.begin()` suppresses ink and swallows the gesture
  when the pen lands on it, so a tap still reaches the DOM button while the handwriting region — and
  therefore the trace — stays whole.
- The menu follows the live preview bounds, throttled to one animation frame, and hides while a new
  lasso is being drawn.
- `useOnyxInk` defers a reconfigure while a gesture is open: `OnyxInk.configure()` pauses raw
  drawing, which would cancel the stroke in progress.
- The persistent outline uses the stock proportions converted to sheet units (`SELECTION_*` in
  `ink-editing.ts`): pure black, padding `9`, frame `2`, trace `3`, dashes `7/7`. Selected strokes
  are still drawn unchanged, matching stock — the frame is the whole affordance.
- The floating menu's box is now part of the sheet's repaint damage, so a menu that moved with the
  selection does not leave a ghost at its old place.
