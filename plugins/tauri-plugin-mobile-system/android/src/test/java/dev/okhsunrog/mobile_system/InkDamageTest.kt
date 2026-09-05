package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class InkDamageTest {
    @Test fun supersededFramesRetainEveryDamagedPositionUntilPresentation() {
        val damage = InkDamage()
        damage.add(10.0, 20.0, 30.0, 40.0)
        damage.add(200.0, 300.0, 250.0, 350.0)
        damage.add(50.0, 50.0, 60.0, 60.0)
        assertEquals(InkBounds(10.0, 20.0, 250.0, 350.0), damage.take())
        assertNull(damage.take())
    }

    @Test fun boundsAreClippedAndEmptyUpdatesDoNotErasePendingDamage() {
        val damage = InkDamage()
        damage.add(-20.0, -10.0, 1100.0, 1500.0)
        damage.add(0.0, 0.0, 0.0, 0.0)
        assertEquals(InkBounds(0.0, 0.0, 1000.0, 1400.0), damage.take())
        damage.add(1200.0, 1500.0, 1300.0, 1600.0)
        assertNull(damage.take())
    }
}
