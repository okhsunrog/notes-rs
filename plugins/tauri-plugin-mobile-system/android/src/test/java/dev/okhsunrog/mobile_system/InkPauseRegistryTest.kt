package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class InkPauseRegistryTest {
    @Test fun onlyTheFirstReasonPausesAndOnlyTheLastResumes() {
        val pauses = InkPauseRegistry()
        assertFalse(pauses.isPaused)
        assertTrue(pauses.pause("ime"))
        assertFalse(pauses.pause("overlay:1"))
        assertTrue(pauses.isPaused)
        // A dialog closing while the keyboard is still up must not bring the pen back.
        assertFalse(pauses.resume("overlay:1"))
        assertTrue(pauses.isPaused)
        assertTrue(pauses.resume("ime"))
        assertFalse(pauses.isPaused)
    }

    @Test fun repeatedPausesAndUnknownResumesAreNoOps() {
        val pauses = InkPauseRegistry()
        assertTrue(pauses.pause("text-focus"))
        // The same field reporting focus twice must still take one resume, not two.
        assertFalse(pauses.pause("text-focus"))
        assertFalse(pauses.resume("never-registered"))
        assertTrue(pauses.isPaused)
        assertTrue(pauses.resume("text-focus"))
        assertFalse(pauses.isPaused)
        assertFalse(pauses.resume("text-focus"))
    }

    @Test fun reasonsAreReportedInTheOrderTheyArrived() {
        val pauses = InkPauseRegistry()
        pauses.pause("ime")
        pauses.pause("overlay:1")
        assertEquals(listOf("ime", "overlay:1"), pauses.reasons())
        pauses.resume("ime")
        assertEquals(listOf("overlay:1"), pauses.reasons())
    }
}
