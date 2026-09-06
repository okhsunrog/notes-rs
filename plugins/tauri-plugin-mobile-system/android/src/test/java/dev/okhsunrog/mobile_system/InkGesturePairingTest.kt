package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class InkGesturePairingTest {
    @Test fun anUnpairedBeginHasToCloseThePreviousGestureFirst() {
        val gesture = InkGesturePairing()
        assertFalse(gesture.beginNeedsEnd())
        gesture.begun()
        assertTrue(gesture.drawing)
        // The firmware skipped the end callback; the next begin must not stack.
        assertTrue(gesture.beginNeedsEnd())
        assertTrue(gesture.ended())
        assertFalse(gesture.beginNeedsEnd())
        gesture.begun()
        assertTrue(gesture.drawing)
    }

    @Test fun endingTwiceReportsNothingLeftToClose() {
        val gesture = InkGesturePairing()
        assertFalse(gesture.ended())
        gesture.begun()
        assertTrue(gesture.ended())
        assertFalse(gesture.ended())
        assertFalse(gesture.drawing)
    }
}
