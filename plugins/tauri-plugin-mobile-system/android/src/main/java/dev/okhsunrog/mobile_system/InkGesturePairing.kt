package dev.okhsunrog.mobile_system

/**
 * begin and end must alternate.
 *
 * The firmware can start a gesture without ever reporting its end — a cancelled
 * stroke, a lifecycle pause between the two callbacks. A begin arriving while
 * one is still open would then leave the palm-rejection region and the transient
 * display mode claimed until the next paired end, which may never come.
 */
internal class InkGesturePairing {
    var drawing = false
        private set

    /** True when the caller must run its end path before starting a new gesture. */
    fun beginNeedsEnd(): Boolean = drawing

    fun begun() {
        drawing = true
    }

    /** Closes an open gesture; false when there was nothing to close. */
    fun ended(): Boolean {
        val was = drawing
        drawing = false
        return was
    }
}
