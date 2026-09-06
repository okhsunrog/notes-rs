package dev.okhsunrog.mobile_system

/**
 * Named reasons the firmware pen layer is currently held down.
 *
 * Stock Notes keeps one predicate over its UI state instead of pausing the pen from every call
 * site, and its dialogs use exactly this shape: a set of class names, resumed only once the set is
 * empty. Named reasons make the pause idempotent — the keyboard may be reported twice, an overlay
 * may close while another is still open — and let `status()` say what is holding the pen.
 */
internal class InkPauseRegistry {
    private val held = linkedSetOf<String>()

    val isPaused: Boolean
        get() = held.isNotEmpty()

    fun reasons(): List<String> = held.toList()

    /** Adds a reason; true when this is the one that turned the pen off. */
    fun pause(reason: String): Boolean {
        val was = isPaused
        held.add(reason)
        return !was && isPaused
    }

    /** Drops a reason; true when this is the one that released the pen. */
    fun resume(reason: String): Boolean {
        if (!held.remove(reason)) return false
        return !isPaused
    }
}
