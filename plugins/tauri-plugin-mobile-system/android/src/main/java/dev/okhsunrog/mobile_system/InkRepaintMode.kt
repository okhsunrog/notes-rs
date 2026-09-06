package dev.okhsunrog.mobile_system

/** Hold the handwriting reconciliation mode through buffer submission, never across pen-down. */
internal class InkRepaintMode(private val enter: () -> Unit, private val leave: () -> Unit) {
    var active = false
        private set

    fun acquire() {
        if (active) return
        enter()
        active = true
    }

    fun release() {
        if (!active) return
        active = false
        leave()
    }
}
