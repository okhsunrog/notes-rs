package dev.okhsunrog.mobile_system

import kotlin.math.ceil
import kotlin.math.floor

/** An integer rectangle in the WebView's own pixel space, free of the Android graphics types. */
internal data class InkRect(val left: Int, val top: Int, val right: Int, val bottom: Int) {
    val empty: Boolean get() = right <= left || bottom <= top
}

/** More overlays than any sheet needs; a runaway list must not reach the firmware. */
private const val MAX_EXCLUDE_RECTS = 8

/**
 * Rectangles the firmware must leave alone inside the drawable region, so a pen over a floating
 * menu taps it instead of inking through it. They arrive in the same CSS pixel space as the sheet
 * geometry, are rounded outwards so no pen pixel survives along an edge, and are clipped to the
 * limit rect: the firmware treats them as screen regions, so anything reaching past the sheet
 * would mask a part of the screen the sheet does not own.
 */
internal fun inkExcludeRects(
    rects: List<OnyxExcludeRect>,
    scale: Double,
    limit: InkRect,
): List<InkRect> {
    if (limit.empty || !scale.isFinite() || scale <= 0) return emptyList()
    return rects.asSequence()
        .filter { listOf(it.left, it.top, it.width, it.height).all { value -> value.isFinite() } }
        .filter { it.width > 0 && it.height > 0 }
        .map {
            InkRect(
                floor(it.left * scale).toInt().coerceAtLeast(limit.left),
                floor(it.top * scale).toInt().coerceAtLeast(limit.top),
                ceil((it.left + it.width) * scale).toInt().coerceAtMost(limit.right),
                ceil((it.top + it.height) * scale).toInt().coerceAtMost(limit.bottom),
            )
        }
        .filterNot { it.empty }
        .take(MAX_EXCLUDE_RECTS)
        .toList()
}
