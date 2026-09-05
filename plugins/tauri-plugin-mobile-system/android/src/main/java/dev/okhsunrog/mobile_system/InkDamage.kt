package dev.okhsunrog.mobile_system

internal data class InkBounds(val left: Double, val top: Double, val right: Double, val bottom: Double)

/** Damage survives superseded compositor acknowledgements; consume only after frame submission. */
internal class InkDamage {
    private var pending: InkBounds? = null

    fun add(left: Double, top: Double, right: Double, bottom: Double) {
        require(listOf(left, top, right, bottom).all { it.isFinite() })
        val l = left.coerceIn(0.0, 1000.0)
        val t = top.coerceIn(0.0, 1400.0)
        val r = right.coerceIn(0.0, 1000.0)
        val b = bottom.coerceIn(0.0, 1400.0)
        if (r <= l || b <= t) return
        val previous = pending
        pending = if (previous == null) InkBounds(l, t, r, b) else InkBounds(
            minOf(l, previous.left), minOf(t, previous.top), maxOf(r, previous.right), maxOf(b, previous.bottom))
    }

    fun take(): InkBounds? = pending.also { pending = null }
}
