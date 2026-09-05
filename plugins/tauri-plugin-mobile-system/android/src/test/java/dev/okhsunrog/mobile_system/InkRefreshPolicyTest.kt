package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class InkRefreshPolicyTest {
    @Test fun fastPenAndFreehandLassoNeverRequestSoftwareAcceleration() {
        val calls = mutableListOf<String>()
        val policy = InkRefreshPolicy({ calls.add("enter") }, { calls.add("leave") })
        policy.begin(false)
        policy.end(100)
        assertFalse(policy.settle(6000))
        policy.reset()
        assertTrue(calls.isEmpty())
    }

    @Test fun aNewPenDownCancelsCleanupEvenIfTheOldTimerHasAlreadyFired() {
        val calls = mutableListOf<String>()
        val policy = InkRefreshPolicy({ calls.add("enter") }, { calls.add("leave") })
        policy.begin(true)
        policy.end(100)
        assertFalse(policy.settle(5099))
        policy.begin(false) // User started writing while the transform mode was settling.
        assertFalse(policy.settle(5100))
        policy.end(7000)
        assertFalse(policy.settle(11999))
        assertTrue(policy.settle(12000))
        assertFalse(policy.settle(13000))
        assertEquals(listOf("enter", "leave"), calls)
    }

    @Test fun consecutiveTransformsShareOneRequestButLifecycleExitClearsImmediately() {
        val calls = mutableListOf<String>()
        val policy = InkRefreshPolicy({ calls.add("enter") }, { calls.add("leave") })
        policy.begin(true)
        policy.end(100)
        policy.begin(true)
        policy.reset() // Close, lost focus, or cancelled geometry.
        policy.reset()
        assertFalse(policy.fastRequested)
        assertFalse(policy.settle(99999))
        policy.begin(true)
        policy.end(100000)
        assertTrue(policy.settle(105000))
        assertEquals(listOf("enter", "leave", "enter", "leave"), calls)
    }
}
