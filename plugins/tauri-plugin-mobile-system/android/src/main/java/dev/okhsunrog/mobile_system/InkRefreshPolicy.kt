package dev.okhsunrog.mobile_system

/** Own only our transient request; a new pen-down postpones the quiet-period cleanup. */
internal class InkRefreshPolicy(private val enter: () -> Unit, private val leave: () -> Unit) {
    var fastRequested = false
        private set
    private var deadline: Long? = null

    companion object { const val QUIET_MS = 5000L }

    fun begin(softwarePreview: Boolean) {
        deadline = null
        if (softwarePreview && !fastRequested) {
            fastRequested = true
            enter()
        }
    }

    fun end(now: Long) {
        if (fastRequested) deadline = now + QUIET_MS
    }

    fun settle(now: Long): Boolean {
        val due = deadline ?: return false
        if (now < due) return false
        reset()
        return true
    }

    fun reset() {
        deadline = null
        if (fastRequested) {
            fastRequested = false
            leave()
        }
    }
}
