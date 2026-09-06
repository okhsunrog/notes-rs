package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

private class RecordingSwitches : RawDrawingSwitches {
    val calls = mutableListOf<String>()
    override fun render(enabled: Boolean) { calls.add("render=$enabled") }
    override fun input(enabled: Boolean) { calls.add("input=$enabled") }
    override fun pushRects() { calls.add("rects") }
    override fun resetDefaults() { calls.add("defaults") }
}

class RawDrawingGateTest {
    @Test fun pausingStopsRenderingBeforeTheReader() {
        val switches = RecordingSwitches()
        RawDrawingGate(switches).pause()
        assertEquals(listOf("render=false", "input=false"), switches.calls)
    }

    @Test fun resumingRePushesGeometryAndArmsInputBeforeRender() {
        val switches = RecordingSwitches()
        RawDrawingGate(switches).resume(render = true)
        assertEquals(listOf("rects", "defaults", "input=true", "render=true"), switches.calls)
    }

    @Test fun aToolThatOnlyWantsRawPointsResumesWithoutFirmwareInk() {
        val switches = RecordingSwitches()
        RawDrawingGate(switches).resume(render = false)
        assertEquals(listOf("rects", "defaults", "input=true", "render=false"), switches.calls)
    }
}
