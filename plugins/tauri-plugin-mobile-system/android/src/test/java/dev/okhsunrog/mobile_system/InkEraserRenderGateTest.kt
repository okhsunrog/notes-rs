package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class InkEraserRenderGateTest {
    @Test fun flippedPenStaysPausedAcrossEraseGesturesUntilTheFrameIsPresented() {
        val events = mutableListOf<String>()
        val gate = InkEraserRenderGate({ events.add("pause") }, { events.add("resume") })
        gate.begin(false)
        gate.begin(true)
        gate.begin(true)
        assertEquals(listOf("pause"), events)
        assertTrue(gate.active)
        gate.framePresented()
        gate.framePresented()
        assertEquals(listOf("pause", "resume"), events)
    }

    @Test fun newPenDownResumesImmediatelyButLifecycleShutdownDoesNot() {
        val events = mutableListOf<String>()
        val gate = InkEraserRenderGate({ events.add("pause") }, { events.add("resume") })
        gate.begin(true)
        gate.begin(false)
        assertEquals(listOf("pause", "resume"), events)
        gate.begin(true)
        gate.reset()
        gate.framePresented()
        assertFalse(gate.active)
        assertEquals(listOf("pause", "resume", "pause"), events)
    }
}
