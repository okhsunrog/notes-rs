package dev.okhsunrog.mobile_system

import com.onyx.android.sdk.api.device.epd.UpdateMode

/**
 * The WebView's default update mode, owned by independent layers.
 *
 * Three parts of the app want to decide how the panel redraws and none of them knows about the
 * others: the display profile picks a base mode for the whole app, the ink editor forces a fast
 * mode while it is open, and a single gesture briefly asks for the handwriting repaint mode. Left
 * to themselves they overwrite each other, and whichever finishes last decides what the panel is
 * left with — which is how a closed editor used to leave the app in its writing mode.
 *
 * The topmost set layer wins. Setting or clearing any layer re-applies the effective mode, so a
 * layer may come and go under a stronger one without ever being visible. Clearing the last set
 * layer restores what the view had before the first one was set, through [restoreRaw] rather than
 * an [UpdateMode]: the firmware's "inherit the system default" state has no enum value, so writing
 * back a read-back enum would pin the view to a concrete mode forever.
 *
 * Pure by construction: the two firmware calls are injected, so the ordering rules are unit tested
 * without a device.
 */
internal class DisplayModeStack(
    private val apply: (UpdateMode) -> Unit,
    private val restoreRaw: () -> Unit,
) {
    /** Ordered weakest to strongest. */
    enum class Layer { BASE, SESSION, TRANSIENT }

    private val modes = arrayOfNulls<UpdateMode>(Layer.values().size)

    /** True while at least one layer owns the view's default update mode. */
    val owned: Boolean
        get() = modes.any { it != null }

    /** The mode the view is set to, or null while no layer owns it. */
    val effective: UpdateMode?
        get() {
            for (index in modes.indices.reversed()) modes[index]?.let { return it }
            return null
        }

    fun isSet(layer: Layer): Boolean = modes[layer.ordinal] != null

    fun set(layer: Layer, mode: UpdateMode) {
        if (modes[layer.ordinal] == mode) return
        modes[layer.ordinal] = mode
        reapply()
    }

    fun clear(layer: Layer) {
        if (modes[layer.ordinal] == null) return
        modes[layer.ordinal] = null
        reapply()
    }

    private fun reapply() {
        val mode = effective
        if (mode == null) restoreRaw() else apply(mode)
    }
}
