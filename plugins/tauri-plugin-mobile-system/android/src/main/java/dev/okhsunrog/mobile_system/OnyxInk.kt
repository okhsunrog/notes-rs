package dev.okhsunrog.mobile_system

import android.app.Activity
import android.graphics.Color
import android.graphics.Rect
import android.graphics.RectF
import android.os.Build
import android.view.ViewTreeObserver
import android.webkit.WebView
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.JSObject
import com.onyx.android.sdk.api.device.epd.EpdController
import com.onyx.android.sdk.data.note.TouchPoint
import com.onyx.android.sdk.pen.RawInputCallback
import com.onyx.android.sdk.pen.TouchHelper
import com.onyx.android.sdk.pen.data.TouchPointList
import com.onyx.android.sdk.utils.ResManager
import org.json.JSONArray
import org.json.JSONObject
import org.lsposed.hiddenapibypass.HiddenApiBypass
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
    var session: String = ""
    var sequence: Long = 0
}

/** Vendor fast ink is transient. Completed points go to the ordinary, durable web canvas. */
class OnyxInk(
    private val activity: Activity,
    private val webView: WebView,
    private val emit: (JSObject) -> Unit,
) : ViewTreeObserver.OnWindowFocusChangeListener {
    private var helper: TouchHelper? = null
    private var config: OnyxInkArgs? = null
    private var sheet = RectF()
    private var limit = Rect()
    private var resumed = true
    private var drawing = false
    private var sequence = 0L
    private var generation = 0L
    private val frames = InkFrameFence()
    private var repaintCount = 0L
    private var pendingFrame: Runnable? = null
    private var pendingObserver: ViewTreeObserver? = null
    private var fastPreview = false
    private var previewAt = 0L
    private var maxPressure = 4095f
    private var failed: String? = null
    private val refresh = Runnable { refreshFrame() }

    companion object {
        fun supported(): Boolean = Build.MANUFACTURER.equals("ONYX", true)
    }

    init {
        // Vendor firmware APIs are hidden from apps targeting recent Android versions.
        // Initialize before any EpdController/Device static lookup, including cleanup calls.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            check(HiddenApiBypass.addHiddenApiExemptions("Landroid/onyx/", "Landroid/view/View;")) {
                "BOOX firmware drawing APIs are unavailable"
            }
        }
        webView.viewTreeObserver.addOnWindowFocusChangeListener(this)
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
        check(failed == null) { failed ?: "Pen SDK unavailable" }
        check(Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && webView.isHardwareAccelerated) {
            "BOOX ink requires Android 10 or later with hardware rendering"
        }
        if (config?.session != args.session) {
            close()
        }
        pause()
        config = args
        // CSS pixels may differ from Android density because BOOX has per-app DPI settings.
        val scale = webView.width / args.viewportWidth
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
    }

    fun commit(args: OnyxFrameArgs) {
        if (config?.session != args.session) return
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
                if (!canPresent() || !frames.present(submission, sequence, drawing)) return@post
                // Keep the raw pen layer alive. The firmware reconciles ink from the new buffer.
                EpdController.handwritingRepaint(webView, limit)
                repaintCount++
            }
        }
        pendingFrame = submitted
        pendingObserver = observer
        observer.registerFrameCommitCallback(submitted)
        webView.invalidate()
    }

    private fun canPresent(): Boolean =
        helper != null && frames.canSubmit(sequence, drawing) &&
            resumed && webView.hasWindowFocus() && !limit.isEmpty

    private fun cancelFrameSubmission() {
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
        if (!resumed || !webView.hasWindowFocus() || config == null || limit.isEmpty) return
        helper?.setRawDrawingEnabled(true)
        helper?.setRawDrawingRenderEnabled(config?.eraser == false &&
            (config?.interaction == false || (config?.fastLasso == true && config?.hasSelection == false)))
    }

    private fun pause() {
        webView.removeCallbacks(refresh)
        frames.request() // Invalidate visual/frame callbacks from the old geometry or lifecycle.
        cancelFrameSubmission()
        helper?.setRawDrawingEnabled(false)
        resetPalm()
        if (drawing) {
            drawing = false
            send("cancel")
        }
    }

    private fun resetPalm() {
        EpdController.appResetCTPDisableRegion(activity)
    }

    fun close() {
        pause()
        generation++
        helper?.closeRawDrawing()
        helper = null
        config = null
    }

    fun destroy() {
        close()
        webView.viewTreeObserver.removeOnWindowFocusChangeListener(this)
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
        val args = config ?: return
        val x = (point.x - sheet.left) / sheet.width() * 1000
        val y = (point.y - sheet.top) / sheet.height() * 1400
        val movingSelection = args.hasSelection && x >= args.selectionLeft - 12 && x <= args.selectionRight + 12 &&
            y >= args.selectionTop - 12 && y <= args.selectionBottom + 12
        fastPreview = args.fastLasso && !erasing && !args.eraser && !movingSelection
        if (args.interaction) helper?.setRawDrawingRenderEnabled(fastPreview)
        drawing = true
        webView.removeCallbacks(refresh)
        cancelFrameSubmission()
        val position = IntArray(2)
        webView.getLocationOnScreen(position)
        val palm = Rect(limit).apply { offset(position[0], position[1]) }
        EpdController.setAppCTPDisableRegion(activity, arrayOf(palm))
        previewAt = 0L
        send("begin", JSONArray().put(normalize(point)), erasing)
    }

    private fun end() {
        if (!drawing) return
        drawing = false
        resetPalm()
        send("end")
        webView.postDelayed(refresh, 120)
    }

    private fun stroke(list: TouchPointList, erasing: Boolean) {
        if (!drawing || config == null || list.isEmpty) return
        val points = JSONArray()
        var pressure = 0.5
        // Same point budget as the portable draft. Never silently retain an unbounded native list.
        for (point in list.points.take(150_000)) {
            if (!point.x.isFinite() || !point.y.isFinite() || !point.pressure.isFinite()) continue
            if (point.pressure > 0) pressure = (point.pressure / maxPressure).toDouble().coerceIn(0.0, 1.0)
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
        if (!drawing || fastPreview || (config?.interaction != true && config?.eraser != true && !erasing)) return
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
