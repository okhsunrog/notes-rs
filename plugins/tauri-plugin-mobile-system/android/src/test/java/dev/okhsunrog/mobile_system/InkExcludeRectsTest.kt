package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class InkExcludeRectsTest {
    private fun rect(left: Double, top: Double, width: Double, height: Double) =
        OnyxExcludeRect().apply {
            this.left = left; this.top = top; this.width = width; this.height = height
        }

    private val sheet = InkRect(0, 0, 800, 1200)

    @Test fun overlaysScaleWithTheDeviceRatioAndRoundOutwards() {
        val excludes = inkExcludeRects(listOf(rect(10.5, 20.25, 100.0, 40.0)), 2.0, sheet)
        // Rounded away from the overlay so no pen pixel survives along an edge.
        assertEquals(listOf(InkRect(21, 40, 221, 121)), excludes)
    }

    @Test fun overlaysAreClippedToTheDrawableLimit() {
        val excludes = inkExcludeRects(listOf(rect(-50.0, -50.0, 1000.0, 5000.0)), 1.0, sheet)
        assertEquals(listOf(sheet), excludes)
    }

    @Test fun overlaysOutsideTheLimitOrWithoutAreaAreDropped() {
        val outside = rect(900.0, 0.0, 100.0, 100.0)
        val flat = rect(10.0, 200.0, 100.0, 0.0)
        val broken = rect(Double.NaN, 200.0, 100.0, 100.0)
        val kept = rect(10.0, 200.0, 100.0, 100.0)
        assertEquals(
            listOf(InkRect(10, 200, 110, 300)),
            inkExcludeRects(listOf(outside, flat, broken, kept), 1.0, sheet),
        )
    }

    @Test fun nothingIsExcludedWithoutADrawableRegion() {
        val overlay = listOf(rect(10.0, 200.0, 100.0, 100.0))
        assertEquals(emptyList<InkRect>(), inkExcludeRects(overlay, 1.0, InkRect(0, 0, 0, 0)))
        assertEquals(emptyList<InkRect>(), inkExcludeRects(overlay, 0.0, sheet))
        assertEquals(emptyList<InkRect>(), inkExcludeRects(emptyList(), 1.0, sheet))
    }

    @Test fun aRunawayOverlayListNeverReachesTheFirmware() {
        val many = (0 until 40).map { rect(it * 10.0, 200.0, 5.0, 5.0) }
        assertEquals(8, inkExcludeRects(many, 1.0, sheet).size)
    }
}
