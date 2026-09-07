package dev.okhsunrog.mobile_system

import kotlin.math.ceil
import kotlin.math.floor

/** An integer rectangle in the WebView's own pixel space, free of the Android graphics types. */
internal data class InkRect(val left: Int, val top: Int, val right: Int, val bottom: Int) {
    val empty: Boolean get() = right <= left || bottom <= top
    fun holds(x: Float, y: Float): Boolean =
        !empty && x.isFinite() && y.isFinite() && x >= left && x < right && y >= top && y < bottom
}

/** More overlays than any sheet needs; a runaway list must not reach the pen callbacks. */
private const val MAX_OVERLAY_RECTS = 8

/**
 * DOM controls floating inside the sheet — today only the selection action menu. They arrive in the
 * same CSS pixel space as the sheet geometry, are rounded outwards so no pen pixel survives along an
 * edge, and are clipped to the drawable limit.
 *
 * These are deliberately **not** firmware exclude rects. Punching a hole in the handwriting region
 * also punches a hole in the lasso trace crossing it, which is exactly the broken dashed outline the
 * selection menu used to cause; stock Notes registers no exclude rect for its selection popup
 * either (see `docs/planning/handwriting-lasso-notes.md`). A pen landing on one of these rectangles
 * is instead swallowed for the length of that one gesture, so the tap reaches the DOM control while
 * the region — and every trace drawn through it — stays whole.
 */
internal fun inkOverlayRects(
    rects: List<OnyxOverlayRect>,
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
        .take(MAX_OVERLAY_RECTS)
        .toList()
}

/** Whether a pen-down at this view pixel belongs to a floating control rather than the sheet. */
internal fun inkOverlayHit(overlays: List<InkRect>, x: Float, y: Float): Boolean =
    overlays.any { it.holds(x, y) }
