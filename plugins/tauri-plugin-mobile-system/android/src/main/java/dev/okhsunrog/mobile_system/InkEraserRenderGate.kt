package dev.okhsunrog.mobile_system

/** Keep firmware pen rendering paused until the erased canvas has been submitted. */
internal class InkEraserRenderGate(private val pause: () -> Unit, private val resume: () -> Unit) {
    var active = false
        private set

    fun begin(erasing: Boolean) {
        if (!erasing) {
            framePresented() // A new pen-down supersedes the pending erase frame.
        } else if (!active) {
            pause()
            active = true
        }
    }

    fun framePresented() {
        if (!active) return
        active = false
        resume()
    }

    fun reset() {
        active = false // Lifecycle shutdown must never re-enable pen rendering.
    }
}
