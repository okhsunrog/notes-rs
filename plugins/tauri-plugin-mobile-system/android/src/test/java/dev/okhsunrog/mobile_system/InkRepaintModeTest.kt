package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class InkRepaintModeTest {
    @Test fun aSubmittedOrCancelledFrameRestoresItsModeExactlyOnce() {
        val events = mutableListOf<String>()
        val mode = InkRepaintMode({ events.add("repaint") }, { events.add("restore") })
        mode.acquire()
        mode.acquire()
        assertTrue(mode.active)
        mode.release()
        mode.release() // Focus loss following pen-down must not restore again.
        assertFalse(mode.active)
        assertEquals(listOf("repaint", "restore"), events)
        mode.acquire()
        mode.release()
        assertEquals(listOf("repaint", "restore", "repaint", "restore"), events)
    }
}
