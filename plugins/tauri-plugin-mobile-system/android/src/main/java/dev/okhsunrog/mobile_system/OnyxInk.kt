package dev.okhsunrog.mobile_system

import android.app.Activity
import android.graphics.Color
import android.graphics.Rect
import android.graphics.RectF
import android.os.Build
import android.os.SystemClock
import android.util.Log
import android.view.ViewTreeObserver
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.JSObject
import com.onyx.android.sdk.api.device.epd.EpdController
import com.onyx.android.sdk.api.device.epd.UpdateMode
import com.onyx.android.sdk.device.Device
import com.onyx.android.sdk.data.note.TouchPoint
import com.onyx.android.sdk.pen.RawInputCallback
import com.onyx.android.sdk.pen.TouchHelper
import com.onyx.android.sdk.pen.data.TouchPointList
import com.onyx.android.sdk.utils.ResManager
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.ceil
import kotlin.math.floor

@InvokeArg
class OnyxInkArgs {
    var session: String = ""
    var enabled: Boolean = false
    var left: Double = 0.0
    var top: Double = 0.0
    var width: Double = 0.0
    var height: Double = 0.0
    var clipTop: Double = 0.0
    var clipBottom: Double = 0.0
    var viewportWidth: Double = 0.0
    var strokeWidth: Double = 3.0
    var eraser: Boolean = false
    var interaction: Boolean = false
    var fastLasso: Boolean = false
    var hasSelection: Boolean = false
    var selectionLeft: Double = 0.0
    var selectionTop: Double = 0.0
    var selectionRight: Double = 0.0
    var selectionBottom: Double = 0.0
}

@InvokeArg
class OnyxFrameArgs {
    var partial: Boolean = false
    var left: Double = 0.0
    var top: Double = 0.0
    var right: Double = 0.0
    var bottom: Double = 0.0
    var session: String = ""
    var sequence: Long = 0
}

/** Vendor fast ink is transient. Completed points go to the ordinary, durable web canvas. */
internal class OnyxInk(
    private val activity: Activity,
    private val webView: WebView,
    private val displayMode: ViewDisplayMode,
    private val pauses: InkPauseRegistry,
    private val emit: (JSObject) -> Unit,
) : ViewTreeObserver.OnWindowFocusChangeListener {
    private var helper: TouchHelper? = null
    private var config: OnyxInkArgs? = null
    private var sheet = RectF()
    private var limit = Rect()
    private var resumed = true
    private val gesture = InkGesturePairing()
    private var sequence = 0L
    private var generation = 0L
    private val frames = InkFrameFence()
    private var repaintCount = 0L
    private val damage = InkDamage()
    private val qualityDamage = InkDamage()
    private var lastRepaint = Rect()
    private var repaintedPixels = 0L
    private var pendingFrame: Runnable? = null
    private var pendingObserver: ViewTreeObserver? = null
    private var fastPreview = false
    private var previewAt = 0L
    private var maxPressure = 4095f
    private var failed: String? = null
    private var imeVisible = false
    private var imeSource: String? = null
    private val refresh = Runnable { refreshFrame() }
    private val gate = RawDrawingGate(object : RawDrawingSwitches {
        override fun render(enabled: Boolean) { helper?.setRawDrawingRenderEnabled(enabled) }
        override fun input(enabled: Boolean) { helper?.setRawInputReaderEnable(enabled) }
        override fun pushRects() { helper?.setLimitRect(limit, emptyList()) }
        override fun resetDefaults() { helper?.resetPenDefaultRawDrawing() }
    })
    // Resuming into the tail of an IME teardown leaves ghost ink; stock Notes waits too.
    private val resumeGate = Runnable { resume() }
    private val eraserRenderGate = InkEraserRenderGate(
        pause = { helper?.setRawDrawingRenderEnabled(false) },
        resume = { restoreToolRendering() },
    )

    private var lastRepaintModeRaw: Int? = null
    private val repaintMode = InkRepaintMode(
        enter = {
            displayMode.set(DisplayModeStack.Layer.TRANSIENT, UpdateMode.HAND_WRITING_REPAINT_MODE)
            lastRepaintModeRaw = displayMode.readRaw()
        },
        // Back to whatever still owns the view: the editor's fast mode, or the profile under it.
        leave = { displayMode.clear(DisplayModeStack.Layer.TRANSIENT) },
    )
    private var fastModeAccepted: Boolean? = null
    private var fastModeRequests = 0L
    private var qualityRestores = 0L
    private val displayPolicy = InkRefreshPolicy(
        enter = {
            fastModeAccepted = EpdController.applyTransientUpdate(UpdateMode.ANIMATION_QUALITY)
            fastModeRequests++
            Log.d("OnyxInk", "transient animation quality requested: $fastModeAccepted")
        },
        leave = {
            // The EpdController wrapper discards this return value; retain it for diagnosis.
            val accepted = Device.currentDevice().clearTransientUpdate(false)
            qualityRestores++
            Log.d("OnyxInk", "transient mode cleared: $accepted")
        },
    )
    private val settleDisplay = Runnable {
        if (!gesture.drawing && displayPolicy.settle(SystemClock.uptimeMillis())) {
            val dirty = qualityDamage.take()
            // Repaint even if the last content revision was already presented in fast mode.
            config?.let { args -> commit(OnyxFrameArgs().apply {
                session = args.session
                sequence = this@OnyxInk.sequence
                partial = true
                if (dirty != null) {
                    left = dirty.left; top = dirty.top; right = dirty.right; bottom = dirty.bottom
                }
            }) }
        }
    }

    companion object {
        fun supported(): Boolean = Build.MANUFACTURER.equals("ONYX", true)

        /** Stock Notes' `DELAY_ENABLE_RAW_DRAWING_MILLS` for a monochrome panel. */
        const val RESUME_DELAY_MS = 150L
        const val IME_REASON = "ime"
    }

    init {
        // Vendor firmware APIs are hidden from apps targeting recent Android versions.
        // Initialize before any EpdController/Device static lookup, including cleanup calls.
        check(VendorAccess.ensure()) { "BOOX firmware drawing APIs are unavailable" }
        // A session killed mid-gesture leaves the panel in transient mode, and
        // nothing else ever clears it. Start from a known state.
        runCatching { Device.currentDevice().clearTransientUpdate(false) }
            .onFailure { Log.d("OnyxInk", "no transient mode to clear: ${it.message}") }
        // Nothing arms the capacitive-panel cutout any more (see resetPalm), but a build that
        // still did may have died holding one. This is the only place it is touched.
        runCatching { resetPalm() }
            .onFailure { Log.d("OnyxInk", "no palm region to clear: ${it.message}") }
        webView.viewTreeObserver.addOnWindowFocusChangeListener(this)
        watchIme()
    }

    /**
     * The soft keyboard never takes our window focus — it is `FLAG_NOT_FOCUSABLE` — so
     * `hasWindowFocus()` cannot see it, while the firmware happily paints ink over it: the limit
     * rect is a screen region with no notion of window z-order. Insets are the one signal that
     * arrives, and this window is edge-to-edge, so they do.
     */
    private fun watchIme() {
        ViewCompat.setOnApplyWindowInsetsListener(webView) { _, insets ->
            imeChanged(insets.getInsets(WindowInsetsCompat.Type.ime()).bottom > 0)
            insets
        }
        ViewCompat.getRootWindowInsets(webView)?.let {
            imeChanged(it.getInsets(WindowInsetsCompat.Type.ime()).bottom > 0)
        }
        ViewCompat.requestApplyInsets(webView)
    }

    private fun imeChanged(visible: Boolean) {
        if (visible == imeVisible) return
        imeVisible = visible
        if (visible) imeSource = "insets"
        if (visible) pauses.pause(IME_REASON) else pauses.resume(IME_REASON)
        pauseStateChanged()
    }

    /**
     * A reason was added or dropped. Pausing is immediate — the keyboard is already coming up —
     * and resuming waits for the panel to settle, cancelled if something pauses again meanwhile.
     */
    fun pauseStateChanged() {
        webView.removeCallbacks(resumeGate)
        // Keep the editor's display mode: the sheet is still on screen, only the pen is down.
        if (pauses.isPaused) pause(releaseDisplay = false)
        else webView.postDelayed(resumeGate, RESUME_DELAY_MS)
    }

    fun configure(args: OnyxInkArgs): JSObject {
        if (!args.enabled) {
            // Ignore cleanup from a sheet which has already been replaced.
            if (config?.session == args.session) close()
            return status()
        }
        require(args.session.length in 1..128)
        require(listOf(args.left, args.top, args.width, args.height, args.clipTop,
            args.clipBottom, args.viewportWidth, args.strokeWidth, args.selectionLeft, args.selectionTop,
            args.selectionRight, args.selectionBottom).all { it.isFinite() })
        require(args.width > 0 && args.height > 0 && args.viewportWidth > 0)
        require(args.strokeWidth in 0.1..20.0)
        // A transient SDK failure — a hidden-API hiccup, a digitizer that was
        // not ready yet — used to disable fast ink for the rest of the process.
        // The message survives for status(); the gate does not.
        failed = null
        check(Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && webView.isHardwareAccelerated) {
            "BOOX ink requires Android 10 or later with hardware rendering"
        }
        if (config?.session != args.session) {
            close()
        }
        pause(releaseDisplay = false)
        config = args
        // CSS pixels may differ from Android density because BOOX has per-app DPI settings.
        val scale = webView.width / args.viewportWidth
        val previousSheet = RectF(sheet)
        val previousLimit = Rect(limit)
        sheet = RectF((args.left * scale).toFloat(), (args.top * scale).toFloat(),
            ((args.left + args.width) * scale).toFloat(), ((args.top + args.height) * scale).toFloat())
        limit = Rect(floor(sheet.left).toInt(), ceil(maxOf(sheet.top.toDouble(), args.clipTop * scale)).toInt(),
            ceil(sheet.right).toInt(), floor(minOf(sheet.bottom.toDouble(), args.clipBottom * scale)).toInt())
        if (!limit.intersect(0, 0, webView.width, webView.height)) limit.setEmpty()
        try {
            if (helper == null) {
                ResManager.init(activity.applicationContext)
                check(EpdController.getTouchWidth() > 0 && EpdController.getTouchHeight() > 0) {
                    "BOOX firmware did not expose the digitizer coordinate range"
                }
                maxPressure = EpdController.getMaxTouchPressure().takeIf { it > 0 } ?: 4095f
                helper = TouchHelper.create(webView, TouchHelper.FEATURE_SF_TOUCH_RENDER, callback(generation), false)
                helper!!.setPenUpRefreshEnabled(false) // Refresh only after the web canvas acknowledges its frame.
                helper!!.setPostInputEvent(false)
                helper!!.setHostViewScrollListenerEnabled(false)
                helper!!.setLimitRect(limit, emptyList()).openRawDrawing()
                helper!!.setEraserRawDrawingEnabled(false, 0)
                helper!!.enableSideBtnErase(true)
            }
            helper!!.setLimitRect(limit, emptyList())
                .setStrokeWidth(((if (args.fastLasso) 1.5 else args.strokeWidth) * sheet.width() / 1000).toFloat())
                .setStrokeColor(Color.BLACK)
                .setStrokeStyle(if (args.fastLasso) TouchHelper.STROKE_STYLE_PENCIL else TouchHelper.STROKE_STYLE_FOUNTAIN)
            resume()
            if (sheet != previousSheet || limit != previousLimit) commit(OnyxFrameArgs().apply {
                session = args.session
                sequence = this@OnyxInk.sequence
            })
            check(helper!!.isRawDrawingCreated) { "Pen SDK did not create a drawing session" }
            return status()
        } catch (error: Throwable) {
            failed = error.message ?: error.javaClass.simpleName
            close()
            throw error
        }
    }

    fun status(): JSObject = JSObject().apply {
        put("available", failed == null)
        put("active", helper?.isRawDrawingInputEnabled == true)
        put("error", failed)
        put("repaintCount", repaintCount)
        put("lastRepaint", lastRepaint.toShortString())
        put("repaintModeActive", repaintMode.active)
        put("eraserRenderPaused", eraserRenderGate.active)
        put("lastRepaintModeRaw", lastRepaintModeRaw)
        put("repaintedPixels", repaintedPixels)
        put("visibleCanvasPixels", limit.width().toLong() * limit.height())
        put("qualityModeOwned", displayMode.isSet(DisplayModeStack.Layer.SESSION))
        put("viewUpdateMode", displayMode.readMode()?.name)
        put("viewUpdateModeRaw", displayMode.readRaw())
        put("requestedViewUpdateMode", displayMode.effective?.name)
        put("previousViewMode", displayMode.previousMode?.name)
        put("previousViewModeRaw", displayMode.previousRaw)
        put("fastModeRequested", displayPolicy.fastRequested)
        put("fastModeAccepted", fastModeAccepted)
        put("fastModeRequests", fastModeRequests)
        put("qualityRestores", qualityRestores)
        put("paused", JSONArray(pauses.reasons()))
        put("imeVisible", imeVisible)
        put("imeSource", imeSource)
    }

    fun commit(args: OnyxFrameArgs) {
        if (config?.session != args.session) return
        if (args.partial) addDamage(args.left, args.top, args.right, args.bottom)
        else addDamage(0.0, 0.0, 1000.0, 1400.0)
        cancelFrameSubmission()
        val revision = frames.request()
        // This only guarantees readiness for the next WebView draw, not a submitted frame.
        webView.postVisualStateCallback(revision, object : WebView.VisualStateCallback() {
            override fun onComplete(requestId: Long) {
                if (config?.session != args.session || !frames.ready(requestId, args.sequence)) return
                webView.removeCallbacks(refresh)
                webView.postDelayed(refresh, 120)
            }
        })
    }

    private fun refreshFrame() {
        if (!canPresent() || pendingFrame != null) return
        val observer = webView.viewTreeObserver
        if (!observer.isAlive) return
        val submission = frames.submission()
        val submitted = Runnable {
            // Frame callbacks may run off the UI thread. Recheck pen and lifecycle state there.
            webView.post {
                if (!frames.isCurrent(submission)) return@post
                pendingFrame = null
                pendingObserver = null
                try {
                    if (!canPresent() || !frames.present(submission, sequence, gesture.drawing)) return@post
                    // Submit the canonical buffer before releasing the eraser's pen-render pause.
                    val dirty = damage.take() ?: return@post
                    val region = Rect(
                        floor(sheet.left + dirty.left * sheet.width() / 1000).toInt() - 2,
                        floor(sheet.top + dirty.top * sheet.height() / 1400).toInt() - 2,
                        ceil(sheet.left + dirty.right * sheet.width() / 1000).toInt() + 2,
                        ceil(sheet.top + dirty.bottom * sheet.height() / 1400).toInt() + 2)
                    if (!region.intersect(limit)) return@post
                    EpdController.handwritingRepaint(webView, region)
                    lastRepaint = Rect(region)
                    repaintedPixels += region.width().toLong() * region.height()
                    repaintCount++
                    eraserRenderGate.framePresented()
                } finally {
                    repaintMode.release()
                }
            }
        }
        pendingFrame = submitted
        pendingObserver = observer
        // Stock Notes marks the actual submitted buffer as a handwriting repaint.
        // A later handwritingRepaint call alone does not remove fast ink on this firmware.
        repaintMode.acquire()
        observer.registerFrameCommitCallback(submitted)
        webView.invalidate()
    }

    private fun canPresent(): Boolean =
        helper != null && frames.canSubmit(sequence, gesture.drawing) &&
            resumed && webView.hasWindowFocus() && !pauses.isPaused && !limit.isEmpty

    private fun cancelFrameSubmission() {
        repaintMode.release()
        frames.cancelSubmission()
        val callback = pendingFrame
        val observer = pendingObserver
        if (callback != null && observer?.isAlive == true) observer.unregisterFrameCommitCallback(callback)
        pendingFrame = null
        pendingObserver = null
    }

    fun onPause() { resumed = false; pause() }
    fun onResume() { resumed = true; resume() }
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        if (hasFocus) resume() else pause()
    }

    private fun resume() {
        if (!resumed || !webView.hasWindowFocus() || pauses.isPaused || config == null || limit.isEmpty) return
        webView.removeCallbacks(resumeGate)
        val acquiredQuality = !displayMode.isSet(DisplayModeStack.Layer.SESSION)
        if (acquiredQuality) {
            displayMode.set(DisplayModeStack.Layer.SESSION, UpdateMode.GU)
            Log.d("OnyxInk", "view quality mode: ${displayMode.readMode()}, previous: ${displayMode.previousMode}")
        }
        gate.resume(toolRendering())
        if (acquiredQuality) config?.let { args -> commit(OnyxFrameArgs().apply {
            session = args.session
            sequence = this@OnyxInk.sequence
        }) }
    }

    private fun pause(releaseDisplay: Boolean = true) {
        if (releaseDisplay || gesture.drawing) releaseDisplayMode()
        webView.removeCallbacks(refresh)
        webView.removeCallbacks(resumeGate)
        frames.request() // Invalidate visual/frame callbacks from the old geometry or lifecycle.
        cancelFrameSubmission()
        gate.pause()
        eraserRenderGate.reset()
        if (gesture.ended()) send("cancel")
    }

    /** Whether the active tool wants firmware ink rather than only its raw points. */
    private fun toolRendering(): Boolean = config?.eraser == false &&
        (config?.interaction == false || (config?.fastLasso == true && config?.hasSelection == false))

    private fun restoreToolRendering() {
        helper?.setRawDrawingRenderEnabled(!pauses.isPaused && toolRendering())
    }

    private fun releaseDisplayMode() {
        repaintMode.release()
        webView.removeCallbacks(settleDisplay)
        displayPolicy.reset()
        qualityDamage.take()
        // The display profile may still hold the layer below; the stack restores the raw mode
        // only once nothing is left above it.
        displayMode.clear(DisplayModeStack.Layer.SESSION)
    }

    private fun addDamage(left: Double, top: Double, right: Double, bottom: Double) {
        damage.add(left, top, right, bottom)
        if (displayPolicy.fastRequested) qualityDamage.add(left, top, right, bottom)
    }

    private fun markNativePoint(point: TouchPoint) {
        if (config == null || sheet.isEmpty || !point.x.isFinite() || !point.y.isFinite()) return
        val x = (point.x - sheet.left).toDouble() / sheet.width() * 1000
        val y = (point.y - sheet.top).toDouble() / sheet.height() * 1400
        val margin = maxOf(3.0, (config?.strokeWidth ?: 3.0) * 2)
        addDamage(x - margin, y - margin, x + margin, y + margin)
    }

    /**
     * Clears a capacitive-panel cutout left behind by an older build. The app never arms one: it
     * kills every touch in that screen band, which is what made the soft keyboard unusable over
     * the sheet, and no reference implementation needs it — palm rejection comes from the limit
     * rect. See `/home/okhsunrog/tmp_zfs/reversed_onyx_notes_app/REPORT.md` (the stock app ships
     * `setAppCTPDisableRegion` with no callers) and
     * `/home/okhsunrog/tmp_zfs/reference_notes_apps/REPORT.md` (none of the five apps uses it).
     */
    private fun resetPalm() {
        EpdController.appResetCTPDisableRegion(activity)
    }

    fun close() {
        pause()
        generation++
        helper?.closeRawDrawing()
        helper = null
        config = null
        damage.take()
    }

    fun destroy() {
        close()
        webView.removeCallbacks(resumeGate)
        webView.viewTreeObserver.removeOnWindowFocusChangeListener(this)
        ViewCompat.setOnApplyWindowInsetsListener(webView, null)
        // The registry outlives this session; a keyboard held down here must not pause the next.
        pauses.resume(IME_REASON)
    }

    private fun send(kind: String, points: JSONArray? = null, erasing: Boolean = false) {
        val args = config ?: return
        emit(JSObject().apply {
            put("session", args.session)
            put("kind", kind)
            put("sequence", sequence)
            put("width", args.strokeWidth)
            put("erasing", erasing || args.eraser)
            put("fastPreview", fastPreview)
            if (points != null) put("points", points)
        })
    }

    private fun begin(point: TouchPoint, erasing: Boolean) {
        if (config == null || helper?.isRawDrawingInputEnabled != true) return
        if (!point.x.isFinite() || !point.y.isFinite() || !point.pressure.isFinite()) return
        // The firmware can skip an end callback; without this the palm region
        // and the transient display mode stay claimed until the next paired end.
        if (gesture.beginNeedsEnd()) end()
        val args = config ?: return
        val x = (point.x - sheet.left) / sheet.width() * 1000
        val y = (point.y - sheet.top) / sheet.height() * 1400
        val movingSelection = args.hasSelection && x >= args.selectionLeft - 12 && x <= args.selectionRight + 12 &&
            y >= args.selectionTop - 12 && y <= args.selectionBottom + 12
        fastPreview = args.fastLasso && !erasing && !args.eraser && !movingSelection
        // Hardware erasing must pause the firmware pen layer even while Pen is selected.
        // Keep raw input enabled so the software eraser continues receiving points.
        eraserRenderGate.begin(erasing || args.eraser)
        if (args.interaction) helper?.setRawDrawingRenderEnabled(fastPreview)
        gesture.begun()
        webView.removeCallbacks(settleDisplay)
        displayPolicy.begin((args.interaction || args.eraser || erasing) && !fastPreview)
        markNativePoint(point)
        webView.removeCallbacks(refresh)
        cancelFrameSubmission()
        previewAt = 0L
        send("begin", JSONArray().put(normalize(point)), erasing)
    }

    private fun end() {
        if (!gesture.ended()) return
        displayPolicy.end(SystemClock.uptimeMillis())
        if (displayPolicy.fastRequested) {
            webView.removeCallbacks(settleDisplay)
            webView.postDelayed(settleDisplay, InkRefreshPolicy.QUIET_MS)
        }
        send("end")
        // The submission in flight belongs to the frame this gesture superseded,
        // exactly as at pen-down.
        cancelFrameSubmission()
        webView.postDelayed(refresh, 120)
    }

    private fun stroke(list: TouchPointList, erasing: Boolean) {
        if (!gesture.drawing || config == null || list.isEmpty) return
        val points = JSONArray()
        var pressure = 0.5
        // Same point budget as the portable draft. Never silently retain an unbounded native list.
        for (point in list.points.take(150_000)) {
            if (!point.x.isFinite() || !point.y.isFinite() || !point.pressure.isFinite()) continue
            if (point.pressure > 0) pressure = (point.pressure / maxPressure).toDouble().coerceIn(0.0, 1.0)
            markNativePoint(point)
            points.put(normalize(point, pressure))
        }
        if (points.length() > 0) {
            sequence++
            send("stroke", points, erasing)
        }
    }

    private fun normalize(point: TouchPoint, pressure: Double = (point.pressure / maxPressure).toDouble().coerceIn(0.0, 1.0)): JSONObject = JSONObject().apply {
        put("x", ((point.x - sheet.left) / sheet.width() * 1000).toDouble().coerceIn(0.0, 1000.0))
        put("y", ((point.y - sheet.top) / sheet.height() * 1400).toDouble().coerceIn(0.0, 1400.0))
        put("pressure", pressure)
        put("tiltX", point.tiltX.coerceIn(-90, 90))
        put("tiltY", point.tiltY.coerceIn(-90, 90))
        put("time", point.timestamp.coerceAtLeast(0))
    }

    private fun preview(point: TouchPoint, erasing: Boolean) {
        if (gesture.drawing) markNativePoint(point)
        if (!gesture.drawing || fastPreview || (config?.interaction != true && config?.eraser != true && !erasing)) return
        val now = android.os.SystemClock.uptimeMillis()
        if (now - previewAt < 32) return
        if (!point.x.isFinite() || !point.y.isFinite() || !point.pressure.isFinite()) return
        previewAt = now
        send("preview", JSONArray().put(normalize(point)), erasing)
    }

    private fun callback(epoch: Long) = object : RawInputCallback() {
        override fun onBeginRawDrawing(shortcut: Boolean, point: TouchPoint) { if (epoch == generation) begin(point, false) }
        override fun onEndRawDrawing(outside: Boolean, point: TouchPoint) { if (epoch == generation) end() }
        override fun onRawDrawingTouchPointMoveReceived(point: TouchPoint) { if (epoch == generation) preview(point, false) }
        override fun onRawDrawingTouchPointListReceived(points: TouchPointList) { if (epoch == generation) stroke(points, false) }
        override fun onBeginRawErasing(shortcut: Boolean, point: TouchPoint) { if (epoch == generation) begin(point, true) }
        override fun onEndRawErasing(outside: Boolean, point: TouchPoint) { if (epoch == generation) end() }
        override fun onRawErasingTouchPointMoveReceived(point: TouchPoint) { if (epoch == generation) preview(point, true) }
        override fun onRawErasingTouchPointListReceived(points: TouchPointList) { if (epoch == generation) stroke(points, true) }
    }
}
