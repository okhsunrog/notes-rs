package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class InkFrameFenceTest {
    @Test fun waitsForLatestCanvasAndTheEndOfThePenGesture() {
        val fence = InkFrameFence()
        val frame = fence.request()
        assertFalse(fence.canSubmit(1, false))
        assertTrue(fence.ready(frame, 1))
        assertFalse(fence.canSubmit(1, true))
        assertFalse(fence.canSubmit(2, false))
        assertTrue(fence.canSubmit(1, false))
    }

    @Test fun rejectsAnOldFrameWhenUndoChangesContentWithoutChangingStrokeSequence() {
        val fence = InkFrameFence()
        val first = fence.request()
        fence.ready(first, 3)
        val submitted = fence.submission()
        val undo = fence.request()
        assertFalse(fence.ready(first, 3))
        assertFalse(fence.present(submitted, 3, false))
        assertTrue(fence.ready(undo, 3))
        assertTrue(fence.present(fence.submission(), 3, false))
    }

    @Test fun aPenDownCancelsEvenAnAlreadyDispatchedFrameCallback() {
        val fence = InkFrameFence()
        fence.ready(fence.request(), 1)
        val old = fence.submission()
        fence.cancelSubmission()
        val next = fence.submission()
        assertFalse(fence.isCurrent(old))
        assertFalse(fence.present(old, 1, false))
        assertTrue(fence.isCurrent(next))
        assertTrue(fence.present(next, 1, false))
        assertFalse(fence.canSubmit(1, false))
    }

    @Test fun closingOrMovingTheSheetInvalidatesBothStages() {
        val fence = InkFrameFence()
        val beforeClose = fence.request()
        fence.ready(beforeClose, 4)
        val submitted = fence.submission()
        fence.request()
        assertFalse(fence.ready(beforeClose, 4))
        assertFalse(fence.present(submitted, 4, false))
        assertFalse(fence.canSubmit(4, false))
    }
}
