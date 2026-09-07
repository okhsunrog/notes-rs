package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class InkOverlayRectsTest {
    private fun rect(left: Double, top: Double, width: Double, height: Double) =
        OnyxOverlayRect().apply {
            this.left = left; this.top = top; this.width = width; this.height = height
        }

    private val sheet = InkRect(0, 0, 800, 1200)

    @Test fun overlaysScaleWithTheDeviceRatioAndRoundOutwards() {
        val overlays = inkOverlayRects(listOf(rect(10.5, 20.25, 100.0, 40.0)), 2.0, sheet)
        // Rounded away from the overlay so no pen pixel survives along an edge.
        assertEquals(listOf(InkRect(21, 40, 221, 121)), overlays)
    }

    @Test fun overlaysAreClippedToTheDrawableLimit() {
        val overlays = inkOverlayRects(listOf(rect(-50.0, -50.0, 1000.0, 5000.0)), 1.0, sheet)
        assertEquals(listOf(sheet), overlays)
    }

    @Test fun overlaysOutsideTheLimitOrWithoutAreaAreDropped() {
        val outside = rect(900.0, 0.0, 100.0, 100.0)
        val flat = rect(10.0, 200.0, 100.0, 0.0)
        val broken = rect(Double.NaN, 200.0, 100.0, 100.0)
        val kept = rect(10.0, 200.0, 100.0, 100.0)
        assertEquals(
            listOf(InkRect(10, 200, 110, 300)),
            inkOverlayRects(listOf(outside, flat, broken, kept), 1.0, sheet),
        )
    }

    @Test fun nothingIsMappedWithoutADrawableRegion() {
        val overlay = listOf(rect(10.0, 200.0, 100.0, 100.0))
        assertEquals(emptyList<InkRect>(), inkOverlayRects(overlay, 1.0, InkRect(0, 0, 0, 0)))
        assertEquals(emptyList<InkRect>(), inkOverlayRects(overlay, 0.0, sheet))
        assertEquals(emptyList<InkRect>(), inkOverlayRects(emptyList(), 1.0, sheet))
    }

    @Test fun aRunawayOverlayListNeverReachesThePenCallbacks() {
        val many = (0 until 40).map { rect(it * 10.0, 200.0, 5.0, 5.0) }
        assertEquals(8, inkOverlayRects(many, 1.0, sheet).size)
    }

    @Test fun aPenDownOnAnOverlayIsRecognisedAndOnlyThere() {
        val overlays = inkOverlayRects(listOf(rect(10.0, 200.0, 100.0, 40.0)), 1.0, sheet)
        assertTrue(inkOverlayHit(overlays, 10f, 200f))
        assertTrue(inkOverlayHit(overlays, 59.5f, 219f))
        // Exclusive on the far edges, so two abutting overlays never both claim a point.
        assertFalse(inkOverlayHit(overlays, 110f, 220f))
        assertFalse(inkOverlayHit(overlays, 60f, 240f))
        assertFalse(inkOverlayHit(overlays, 9.9f, 220f))
        assertFalse(inkOverlayHit(overlays, Float.NaN, 220f))
        // With no overlay the sheet takes every point, which is what keeps a lasso trace whole.
        assertFalse(inkOverlayHit(emptyList(), 30f, 210f))
    }
}
