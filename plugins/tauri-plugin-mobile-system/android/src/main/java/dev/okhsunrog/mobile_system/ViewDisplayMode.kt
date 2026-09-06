package dev.okhsunrog.mobile_system

import android.view.View
import android.webkit.WebView
import com.onyx.android.sdk.api.device.epd.EpdController
import com.onyx.android.sdk.api.device.epd.UpdateMode

/**
 * A [DisplayModeStack] bound to one WebView, plus the raw mode captured before the app took the
 * view over.
 *
 * The display profile outlives every ink session, so this is owned by the plugin and handed to
 * [OnyxInk] rather than the other way round. The capture happens on the first applied layer and is
 * dropped when the last one clears, which is exactly when [DisplayModeStack] calls back.
 */
internal class ViewDisplayMode(private val webView: WebView) {
    /** The enum-visible mode before the first layer was set. Diagnostics only. */
    var previousMode: UpdateMode? = null
        private set

    /**
     * The raw firmware value before the first layer was set. Unlike [previousMode] this keeps the
     * "no view override" sentinel, which the SDK enum conversion loses.
     */
    var previousRaw: Int? = null
        private set

    private var captured = false

    private val stack = DisplayModeStack(
        apply = { mode ->
            if (!captured) {
                captured = true
                previousMode = EpdController.getViewDefaultUpdateMode(webView)
                previousRaw = readRaw()
            }
            EpdController.setViewDefaultUpdateMode(webView, mode)
        },
        restoreRaw = {
            val restored = previousRaw?.let { raw ->
                runCatching {
                    View::class.java.getMethod("setDefaultUpdateMode", Int::class.javaPrimitiveType)
                        .invoke(webView, raw)
                }.isSuccess
            } ?: false
            if (!restored) EpdController.resetViewUpdateMode(webView)
            captured = false
            previousMode = null
            previousRaw = null
        },
    )

    val effective: UpdateMode?
        get() = stack.effective

    fun isSet(layer: DisplayModeStack.Layer): Boolean = stack.isSet(layer)

    fun set(layer: DisplayModeStack.Layer, mode: UpdateMode) = stack.set(layer, mode)

    fun clear(layer: DisplayModeStack.Layer) = stack.clear(layer)

    /** The mode the firmware actually reports for the view, which need not be what we asked for. */
    fun readMode(): UpdateMode? = EpdController.getViewDefaultUpdateMode(webView)

    fun readRaw(): Int? = runCatching {
        View::class.java.getMethod("getDefaultUpdateMode").invoke(webView) as? Int
    }.getOrNull()
}
